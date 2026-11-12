//! `models import`: explicit public discovery, verified conversion, then atomic publication of
//! ordinary project routes ([model-selection.md](../../../../spec/model-selection.md) §3).
//! No credentials are loaded and no generation request is made. All validation precedes writes.

use crate::cli::{ModelImportArgs, ModelsArgs, ModelsVerb};
use crate::print::print_line;
use grida_fx_core::docs::project::Project;
use grida_fx_core::docs::{Schema, validate};
use grida_fx_core::money::Usd;
use grida_fx_core::registry::relative_inside;
use grida_fx_core::value::sha256_hex;
use grida_fx_core::{Error, ErrorKind, yaml};
use grida_fx_providers::models::{self, ASSETS, MAX_ASSET_BYTES};
use grida_fx_providers::transport::http::{HttpConfig, HttpTransport};
use grida_fx_providers::transport::{HttpRequest, Lane, Method, Transport};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub fn run(args: &ModelsArgs) -> Result<u8, Error> {
    match &args.verb {
        ModelsVerb::Import(args) => import(args),
    }
}

fn import(args: &ModelImportArgs) -> Result<u8, Error> {
    let cap = Usd::parse(&args.cap_usd).map_err(Error::usage)?;
    if cap <= Usd::ZERO {
        return Err(Error::usage(
            "models import needs a positive per-call --cap-usd allowance",
        ));
    }
    if args.provider != "gg" {
        return Err(Error::usage(
            "models import currently supports GG image interfaces only (--provider gg)",
        ));
    }
    if let Some(tag) = &args.release {
        models::check_release(tag).map_err(Error::usage)?;
    }
    let cwd = super::planning::working_directory()?;
    let project = Project::find(&cwd)?;
    let config_path = project.root.join("fx.yaml");
    confined(&project.root, &config_path, "fx.yaml")?;
    let config = if project.has_file {
        Some(read_bounded(&config_path, "fx.yaml")?)
    } else {
        None
    };
    let (tag, files) = match &args.from {
        Some(folder) => {
            let folder = cwd.join(folder);
            let mut files = BTreeMap::new();
            for name in ASSETS.into_iter().chain(["gg.operations.json"]) {
                files.insert(name.into(), read_bounded(&folder.join(name), name)?);
            }
            let manifest = models::json_file(&files["manifest.json"], "manifest.json")
                .map_err(Error::document)?;
            let tag = args
                .release
                .clone()
                .or_else(|| manifest["version"].as_str().map(str::to_string))
                .ok_or_else(|| Error::document("manifest.json has no Models release version"))?;
            (tag, files)
        }
        None => fetch_assets(args.release.as_deref())?,
    };
    let release = models::verify_release(&tag, &files).map_err(Error::document)?;
    let table = models::gg_image_table(&release, &files["gg.operations.json"], &args.model, cap)
        .map_err(Error::document)?;
    validate(Schema::Routes, &table, "the imported route table")?;
    grida_fx_core::routes::RouteTable::from_document(&table, "the imported route table")?;
    let content = yaml::write_yaml(&table);
    let relative = format!("routes/models/{}.yaml", sha256_hex(content.as_bytes()));
    let target = project.root.join(&relative);
    confined(&project.root, &target, &relative)?;
    let original = config.as_deref().unwrap_or(b"fx: project/v1\n");
    let original =
        std::str::from_utf8(original).map_err(|_| Error::document("fx.yaml is not UTF-8"))?;
    let updated = register_table(original, &relative)?;
    // Validate every edit before either file is published, including preserving all other values.
    let written =
        yaml::load(updated.as_bytes(), "fx.yaml").map_err(|e| Error::document(e.to_string()))?;
    validate(Schema::Project, &written, "fx.yaml")?;
    publish_new(&target, content.as_bytes(), &relative)?;
    let config_changed = config.as_deref() != Some(updated.as_bytes());
    if config_changed {
        let saved = (|| {
            let current = if config_path.exists() {
                Some(read_bounded(&config_path, "fx.yaml")?)
            } else {
                None
            };
            if current.as_deref() == Some(updated.as_bytes()) {
                Ok(())
            } else if current != config {
                Err(Error::usage(
                    "fx.yaml changed during model import; repeat the import",
                ))
            } else if config.is_none() {
                publish_new(&config_path, updated.as_bytes(), "fx.yaml").map(|_| ())
            } else {
                grida_fx_runtime::store::atomic_write(&config_path, updated.as_bytes(), false)
                    .map_err(|e| Error::io("fx.yaml", &e))
            }
        })();
        // Immutable route data may already have been registered by another identical import.
        // Leave it intact on a configuration race; an unreferenced file is safe to reuse.
        saved?;
    }
    print_line(&format!("imported {} on gg from Models {tag}", args.model));
    print_line(&format!("registered {relative}"));
    print_line(
        "the cap is a planning allowance; importing generates no media and approves no spending",
    );
    Ok(0)
}

fn read_bounded(path: &Path, label: &str) -> Result<Vec<u8>, Error> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(label, &e))?;
    if !file.metadata().map_err(|e| Error::io(label, &e))?.is_file() {
        return Err(Error::usage(format!("{label} is not a regular file")));
    }
    let mut bytes = Vec::new();
    file.take(MAX_ASSET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io(label, &e))?;
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(Error::usage(format!("{label} is larger than 32 MiB")));
    }
    Ok(bytes)
}

/// Refuse output symlinks and any parent which resolves outside the project.
fn confined(root: &Path, target: &Path, label: &str) -> Result<(), Error> {
    if relative_inside(root, target).is_none()
        || std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(Error::usage(format!(
            "{label}: model import writes regular files inside the project"
        )));
    }
    Ok(())
}

/// Publishes one immutable file without replacing anything. Identical content is idempotent.
fn publish_new(path: &Path, bytes: &[u8], label: &str) -> Result<bool, Error> {
    if path.exists() {
        return if read_bounded(path, label)? == bytes {
            Ok(false)
        } else {
            Err(Error::usage(format!(
                "{label} already exists with different content; it was not overwritten"
            )))
        };
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::internal("an import file has no parent"))?;
    std::fs::create_dir_all(parent).map_err(|e| Error::io(label, &e))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|e| Error::io(label, &e))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|e| Error::io(label, &e))?;
    temporary
        .persist_noclobber(path)
        .map_err(|e| Error::io(label, &e.error))?;
    Ok(true)
}

fn fetch_assets(release: Option<&str>) -> Result<(String, BTreeMap<String, Vec<u8>>), Error> {
    if grida_fx_providers::live::Network::from_env(&|n| std::env::var(n).ok())
        == grida_fx_providers::live::Network::Off
    {
        return Err(Error::usage(
            "models import needs public catalogue downloads, and the network is off (GRIDA_FX_NETWORK=off); use --from DIRECTORY",
        ));
    }
    let transport = HttpTransport::new(HttpConfig::default()).map_err(Error::internal)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::io("the catalogue download", &e))?;
    runtime.block_on(async {
        let tag = match release {
            Some(tag) => tag.to_string(),
            None => {
                let bytes = download(
                    &transport,
                    "https://api.github.com/repos/gridaco/models/releases/latest",
                )
                .await?;
                let meta = models::json_file(&bytes, "the Models release metadata")
                    .map_err(Error::document)?;
                meta["tag_name"]
                    .as_str()
                    .ok_or_else(|| Error::document("the Models release metadata has no tag_name"))?
                    .to_string()
            }
        };
        models::check_release(&tag).map_err(Error::document)?;
        let mut files = BTreeMap::new();
        for name in ASSETS {
            files.insert(
                name.into(),
                download(
                    &transport,
                    &format!("{}/releases/download/{tag}/{name}", models::REPOSITORY),
                )
                .await?,
            );
        }
        files.insert(
            "gg.operations.json".into(),
            download(&transport, models::OPERATIONS_URL).await?,
        );
        Ok((tag, files))
    })
}

/// Explicit bounded public release redirects, never a credential or arbitrary schema fetch.
async fn download(transport: &dyn Transport, initial: &str) -> Result<Vec<u8>, Error> {
    let mut url =
        url::Url::parse(initial).map_err(|_| Error::internal("the catalogue URL is invalid"))?;
    let github = url
        .host_str()
        .is_some_and(|h| h == "github.com" || h == "api.github.com");
    for redirects in 0..=3 {
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some_and(|p| p != 443)
            || url.fragment().is_some()
            || !match url.host_str() {
                Some("github.com" | "api.github.com" | "release-assets.githubusercontent.com") => {
                    github
                }
                Some("gateway.grida.ai") => {
                    !github && url.path() == "/schemas/operations.json" && url.query().is_none()
                }
                _ => false,
            }
        {
            return Err(Error::usage(
                "the public catalogue redirect is outside its admitted HTTPS origins",
            ));
        }
        let mut request = HttpRequest::new(Method::Get, url.as_str(), Lane::Download)
            .timeout(Duration::from_secs(60))
            .max_response_bytes(MAX_ASSET_BYTES as u64);
        request.headers.push((
            "user-agent".into(),
            format!("grida-fx/{}", grida_fx_core::ENGINE_VERSION),
        ));
        let response = transport.send(request).await.map_err(|_| {
            Error::usage("the public catalogue download failed; no project files were changed")
        })?;
        if response.status == 200 {
            return Ok(response.body);
        }
        if github && redirects < 3 && matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v)
                .ok_or_else(|| Error::usage("the catalogue redirect has no location"))?;
            url = url
                .join(location)
                .map_err(|_| Error::usage("the catalogue redirect is malformed"))?;
        } else {
            return Err(Error::new(
                ErrorKind::Io,
                format!(
                    "the public catalogue download returned HTTP {}; no project files were changed",
                    response.status
                ),
            ));
        }
    }
    unreachable!("the redirect loop always returns at its bound")
}

/// Preserve the project's text and comments. Exotic YAML is refused instead of rewritten.
fn register_table(text: &str, relative: &str) -> Result<String, Error> {
    let mut expected =
        yaml::load(text.as_bytes(), "fx.yaml").map_err(|e| Error::document(e.to_string()))?;
    let mut tables = expected["route_tables"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if tables.last().is_some_and(|v| v == relative) {
        return Ok(text.to_string());
    }
    let moving = tables.iter().position(|v| v == relative);
    if let Some(index) = moving {
        tables.remove(index);
    }
    tables.push(relative.into());
    let had_tables = expected.get("route_tables").is_some();
    expected["route_tables"] = Value::Array(tables);
    let quoted = serde_json::to_string(relative).expect("a string is JSON");
    let refused = || {
        Error::usage(format!(
            "cannot safely edit fx.yaml's route_tables layout; add {quoted} to route_tables manually, then repeat the import"
        ))
    };
    let mut updated = text.to_string();
    if !had_tables {
        if text
            .trim_start_matches(|c: char| c.is_whitespace())
            .starts_with('{')
        {
            return Err(refused());
        }
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&format!("route_tables:\n  - {quoted}\n"));
    } else {
        let mut offset = 0;
        let mut field = None;
        for line in text.split_inclusive('\n') {
            if line.starts_with("route_tables:") {
                field = Some((offset, line));
                break;
            }
            offset += line.len();
        }
        let (start, line) = field.ok_or_else(refused)?;
        let value_start = start + "route_tables:".len();
        let value = line["route_tables:".len()..].trim_start();
        if value.starts_with('[') {
            let open = value_start + text[value_start..].find('[').ok_or_else(refused)?;
            let mut sequence = flow_sequence(text, open).ok_or_else(refused)?;
            if let Some(index) = moving {
                let (range, comma) = sequence.items.get(index).ok_or_else(refused)?;
                // A moved entry was not last, so its following comma exists. Retain every
                // comment and whitespace outside the scalar while removing that one entry.
                updated.remove(comma.ok_or_else(refused)?);
                updated.replace_range(range.clone(), "");
                sequence = flow_sequence(&updated, open).ok_or_else(refused)?;
            }
            let separator = if sequence.last == Some(b'[') || sequence.last == Some(b',') {
                " "
            } else {
                ", "
            };
            updated.insert_str(sequence.close, &format!("{separator}{quoted}"));
        } else if value.trim().is_empty() || value.starts_with('#') {
            let after = start + line.len();
            let mut end = after;
            let mut indent = None;
            let mut entries = Vec::new();
            for following in text[after..].split_inclusive('\n') {
                let plain = following.trim_start();
                if !plain.trim().is_empty() && !plain.starts_with('#') {
                    if plain.starts_with("- ") {
                        indent.get_or_insert(following.len() - plain.len());
                        entries.push(end..end + following.len());
                    } else if !following.starts_with(char::is_whitespace) {
                        break;
                    } else {
                        return Err(refused());
                    }
                }
                end += following.len();
            }
            let entry = if let Some(index) = moving {
                let range = entries.get(index).ok_or_else(refused)?;
                let entry = text[range.clone()].to_string();
                updated.replace_range(range.clone(), "");
                end -= range.len();
                entry
            } else {
                format!("{}- {quoted}\n", " ".repeat(indent.unwrap_or(2)))
            };
            let lead = if end > 0 && !updated[..end].ends_with('\n') {
                "\n"
            } else {
                ""
            };
            updated.insert_str(end, &format!("{lead}{entry}"));
        } else {
            return Err(refused());
        }
    }
    let actual = yaml::load(updated.as_bytes(), "fx.yaml").map_err(|_| refused())?;
    if actual != expected {
        return Err(refused());
    }
    Ok(updated)
}

struct FlowSequence {
    close: usize,
    last: Option<u8>,
    items: Vec<(std::ops::Range<usize>, Option<usize>)>,
}

/// Flow scalar spans and separators, respecting quoted strings and comments. Nested values
/// are refused because project route_tables contains strings only.
fn flow_sequence(text: &str, open: usize) -> Option<FlowSequence> {
    let bytes = text.as_bytes();
    let mut quote = None;
    let mut comment = false;
    let mut last = Some(b'[');
    let mut first = None;
    let mut value_end = None;
    let mut items = Vec::new();
    let mut index = open + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
            }
        } else if let Some(q) = quote {
            if (q == b'"' && byte == b'\\')
                || (q == b'\'' && byte == b'\'' && bytes.get(index + 1) == Some(&b'\''))
            {
                index += 1;
            } else if byte == q {
                quote = None;
                last = Some(q);
                value_end = Some(index + 1);
            }
        } else {
            match byte {
                b'#' => comment = true,
                b'\'' | b'"' => {
                    first.get_or_insert(index);
                    quote = Some(byte);
                }
                b',' => {
                    items.push((first.take()?..value_end.take()?, Some(index)));
                    last = Some(byte);
                }
                b']' => {
                    if let Some(start) = first {
                        items.push((start..value_end?, None));
                    }
                    return Some(FlowSequence {
                        close: index,
                        last,
                        items,
                    });
                }
                b'[' | b'{' | b'}' => return None,
                b if !b.is_ascii_whitespace() => {
                    first.get_or_insert(index);
                    value_end = Some(index + 1);
                    last = Some(b);
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_registration_preserves_comments_and_every_other_setting() {
        for original in [
            "# project\nfx: project/v1\n# keep\n",
            "fx: project/v1\nroute_tables: [] # keep\nroutes: {}\n",
            "fx: project/v1\nroute_tables: [old.yaml, # keep\n]\nroutes: {}\n",
            "fx: project/v1\nroute_tables:\n  - old.yaml # keep\nroutes: {}\n",
            "fx: project/v1\nroute_tables:\n- old.yaml # keep\n# end\n",
            "fx: project/v1\nroute_tables: ['path]a.yaml', \"path\\\"b.yaml\"] # keep\n",
        ] {
            let updated = register_table(original, "routes/models/acme.yaml").unwrap();
            assert!(updated.contains("# keep"));
            let document = yaml::load(updated.as_bytes(), "fx.yaml").unwrap();
            assert_eq!(
                document["route_tables"].as_array().unwrap().last(),
                Some(&Value::from("routes/models/acme.yaml"))
            );
            assert_eq!(
                register_table(&updated, "routes/models/acme.yaml").unwrap(),
                updated
            );
        }
        assert!(
            register_table("{fx: project/v1}\n", "routes/models/acme.yaml")
                .unwrap_err()
                .message
                .contains("manually")
        );
    }

    #[test]
    fn immutable_route_publication_never_replaces_existing_bytes() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("routes/model.yaml");
        assert!(publish_new(&target, b"original", "routes/model.yaml").unwrap());
        assert!(!publish_new(&target, b"original", "routes/model.yaml").unwrap());
        assert!(publish_new(&target, b"other", "routes/model.yaml").is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"original");
    }

    #[test]
    fn reimport_moves_its_table_last_and_preserves_other_order_and_comments() {
        for original in [
            "fx: project/v1\nroute_tables: [first.yaml, old.yaml, # old note\n latest.yaml, last.yaml] # keep\nroutes: {}\n",
            "fx: project/v1\nroute_tables:\n  - first.yaml\n  - old.yaml # old note\n  - latest.yaml\n  - last.yaml\n# keep\nroutes: {}\n",
            "fx: project/v1\nroute_tables:\n- old.yaml # old note\n- latest.yaml # keep",
        ] {
            let updated = register_table(original, "old.yaml").unwrap();
            let before = yaml::load(original.as_bytes(), "fx.yaml").unwrap();
            let after = yaml::load(updated.as_bytes(), "fx.yaml").unwrap();
            let mut expected = before["route_tables"].as_array().unwrap().clone();
            expected.retain(|value| value != "old.yaml");
            expected.push("old.yaml".into());
            assert_eq!(after["route_tables"], Value::Array(expected));
            assert!(updated.contains("# old note"));
            assert!(updated.contains("# keep"));
            assert_eq!(register_table(&updated, "old.yaml").unwrap(), updated);
        }
    }
}
