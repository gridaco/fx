//! File kinds (identity.md §4) and the output-file suffix of a kind (store.md "Run folders").

/// identity.md §4: each suffix and its kind, in the table's order (a kind's first suffix is the
/// one a run folder writes).
const SUFFIXES: [(&str, &str); 22] = [
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".webp", "image/webp"),
    (".gif", "image/gif"),
    (".md", "text/markdown"),
    (".txt", "text/plain"),
    (".json", "json"),
    (".yaml", "text/yaml"),
    (".yml", "text/yaml"),
    (".toml", "text/toml"),
    (".html", "text/html"),
    (".wav", "audio/wav"),
    (".mp3", "audio/mpeg"),
    (".ogg", "audio/ogg"),
    (".mp4", "video/mp4"),
    (".webm", "video/webm"),
    (".mkv", "video/x-matroska"),
    (".glb", "model/gltf-binary"),
    (".gltf", "model/gltf+json"),
    (".fbx", "model/fbx"),
    (".zip", "file/zip"),
];

/// The suffix of a file name as Python's `PurePath.suffix` reads it: the last `.` and what
/// follows in the final path segment, when the dot is neither the segment's first nor its last
/// character.
fn suffix(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(at) if at > 0 && at + 1 < base.len() => &base[at..],
        _ => "",
    }
}

/// The kind of a file from its name's suffix, compared case-insensitively (identity.md §4 table);
/// `file` for anything else, including no suffix.
pub fn kind_of(name: &str) -> &'static str {
    let suffix = suffix(name).to_lowercase();
    SUFFIXES
        .iter()
        .find(|(s, _)| *s == suffix)
        .map_or("file", |(_, kind)| kind)
}

/// The suffix a run writes for an output of this kind (store.md "Run folders"): the first suffix
/// identity.md §4 lists for the kind (`image/png` → `.png`, `image/jpeg` → `.jpg`, `image/gif`
/// → `.gif`, `text/yaml` → `.yaml`), `.json` for `annotations`, and `""` for every other kind,
/// `file` and families such as `image` included. gnode's run folders had no suffix for
/// `image/gif`, `audio/ogg`, `text/yaml`, `text/toml`, `text/html` and `model/gltf+json`;
/// store.md §10 lists that change.
pub fn suffix_of_kind(kind: &str) -> &'static str {
    if kind == "annotations" {
        return ".json";
    }
    SUFFIXES
        .iter()
        .find(|(_, k)| *k == kind)
        .map_or("", |(suffix, _)| suffix)
}

/// The family of a kind: the part before `/` (`image/png` → `image`).
pub fn family(kind: &str) -> &str {
    kind.split('/').next().unwrap_or(kind)
}

/// Whether file content of this kind is read as text: kinds starting `text` (identity.md §5).
pub fn is_text(kind: &str) -> bool {
    kind.starts_with("text")
}

/// Whether file content of this kind is read as JSON: `json`, any `…+json`, and `annotations`.
pub fn is_json(kind: &str) -> bool {
    kind == "json" || kind == "annotations" || kind.ends_with("+json")
}

/// The kind of a workflow input file given for a declared kind: the suffix kind wins when it is
/// known (a declared kind is never enforced); the declared kind is used only for an unknown
/// suffix and a declaration other than `file` (`kind: image` and `brief` give `image`). As FX's
/// predecessor did, a declaration that `file` starts with keeps `file`.
pub fn effective_kind(name: &str, declared: &str) -> String {
    let kind = kind_of(name);
    if kind == "file" && !kind.starts_with(declared) {
        declared.to_string()
    } else {
        kind.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_suffix_table() {
        let cases = [
            ("a.png", "image/png"),
            ("a.PNG", "image/png"),
            ("dir/a.JpEg", "image/jpeg"),
            ("a.jpg", "image/jpeg"),
            ("a.webp", "image/webp"),
            ("a.gif", "image/gif"),
            ("a.md", "text/markdown"),
            ("a.txt", "text/plain"),
            ("a.json", "json"),
            ("a.yaml", "text/yaml"),
            ("a.YML", "text/yaml"),
            ("a.toml", "text/toml"),
            ("a.html", "text/html"),
            ("a.wav", "audio/wav"),
            ("a.mp3", "audio/mpeg"),
            ("a.ogg", "audio/ogg"),
            ("a.mp4", "video/mp4"),
            ("a.webm", "video/webm"),
            ("a.mkv", "video/x-matroska"),
            ("a.glb", "model/gltf-binary"),
            ("a.gltf", "model/gltf+json"),
            ("a.fbx", "model/fbx"),
            ("a.zip", "file/zip"),
            ("a.tar.gz", "file"),
            ("a", "file"),
            ("", "file"),
            (".png", "file"),
            ("dir/.png", "file"),
            ("a.", "file"),
            ("a.png.", "file"),
            ("x.png/a", "file"),
            ("..png", "image/png"),
            // Python lower-cases U+212A KELVIN SIGN to `k`.
            ("a.m\u{212A}v", "video/x-matroska"),
        ];
        for (name, kind) in cases {
            assert_eq!(kind_of(name), kind, "{name:?}");
        }
    }

    #[test]
    fn suffixes_of_kinds() {
        let cases = [
            ("image/png", ".png"),
            ("image/jpeg", ".jpg"),
            ("image/webp", ".webp"),
            ("image/gif", ".gif"),
            ("json", ".json"),
            ("annotations", ".json"),
            ("text/markdown", ".md"),
            ("text/plain", ".txt"),
            ("text/yaml", ".yaml"),
            ("text/toml", ".toml"),
            ("text/html", ".html"),
            ("audio/wav", ".wav"),
            ("audio/mpeg", ".mp3"),
            ("audio/ogg", ".ogg"),
            ("video/mp4", ".mp4"),
            ("video/webm", ".webm"),
            ("video/x-matroska", ".mkv"),
            ("model/gltf-binary", ".glb"),
            ("model/gltf+json", ".gltf"),
            ("model/fbx", ".fbx"),
            ("file/zip", ".zip"),
            ("file", ""),
            ("image", ""),
            ("text", ""),
            ("image/x-unknown", ""),
        ];
        for (kind, suffix) in cases {
            assert_eq!(suffix_of_kind(kind), suffix, "{kind:?}");
        }
    }

    #[test]
    fn effective_kinds() {
        // The suffix kind wins whenever the suffix is known.
        assert_eq!(effective_kind("a.txt", "image"), "text/plain");
        assert_eq!(effective_kind("a.png", "image"), "image/png");
        assert_eq!(effective_kind("a.png", "file"), "image/png");
        assert_eq!(effective_kind("a.png", "text/plain"), "image/png");
        // An unknown suffix takes a declared kind other than `file`.
        assert_eq!(effective_kind("a", "image"), "image");
        assert_eq!(effective_kind("a.bin", "image/png"), "image/png");
        assert_eq!(effective_kind("a.bin", "file"), "file");
        // gnode's prefix rule: `file` starts with `fi`.
        assert_eq!(effective_kind("a.bin", "fi"), "file");
    }

    #[test]
    fn families_text_and_json() {
        assert_eq!(family("image/png"), "image");
        assert_eq!(family("json"), "json");
        assert!(is_text("text/plain"));
        assert!(!is_text("json"));
        assert!(is_json("json"));
        assert!(is_json("annotations"));
        assert!(is_json("model/gltf+json"));
        assert!(!is_json("text/plain"));
    }
}
