//! The project file, `fx.yaml` (`fx: project/v1`; spec/schemas/fx-project-v1.schema.json).
//!
//! `Project::find(start)` walks up from `start` (a file's folder, or the folder itself) to the
//! first folder holding a regular file `fx.yaml`. With none, the project root is `start` and the
//! document is all defaults (`runs: runs`, `cache: .fx/cache`, `nodes: nodes`). An invalid
//! `fx.yaml` is an error (exit 2).
//!
//! The start is made absolute and its symbolic links are resolved (as far as it exists), as
//! Python's `Path.resolve()` does. Messages name the file `fx.yaml`. A `nodes:` folder with a
//! `..` segment reads `nodes is a folder inside the project`, gnode's sentence.

use super::workflow::Budget;
use super::{Schema, check_kind, validate};
use crate::error::{Error, Result};
use crate::money::Usd;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The project file's name.
const PROJECT_FILE: &str = "fx.yaml";

/// A capability's default route in fx.yaml `routes:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDefault {
    pub route: String,
    pub concurrency: Option<u32>,
}

/// The typed project file.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDoc {
    pub runs: String,
    pub cache: String,
    pub budget: Option<Budget>,
    pub routes: IndexMap<String, RouteDefault>,
    pub route_tables: Vec<String>,
    pub view_origins: Vec<String>,
    pub sources: Vec<String>,
    pub nodes: String,
}

impl Default for ProjectDoc {
    fn default() -> Self {
        Self {
            runs: "runs".into(),
            cache: ".fx/cache".into(),
            budget: None,
            routes: IndexMap::new(),
            route_tables: Vec::new(),
            view_origins: Vec::new(),
            sources: Vec::new(),
            nodes: "nodes".into(),
        }
    }
}

/// A project: its root (absolute, symlinks resolved) and its document.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    pub document: ProjectDoc,
    /// Whether an `fx.yaml` was found.
    pub has_file: bool,
}

impl Project {
    /// Finds the project of `start` (module doc).
    pub fn find(start: &Path) -> Result<Project> {
        let mut here = crate::inputs::bind::resolve(start);
        if here.is_file()
            && let Some(parent) = here.parent()
        {
            here = parent.to_path_buf();
        }
        for folder in here.ancestors() {
            let candidate = folder.join(PROJECT_FILE);
            if candidate.is_file() {
                return Ok(Project {
                    root: folder.to_path_buf(),
                    document: read_project(&candidate, PROJECT_FILE)?,
                    has_file: true,
                });
            }
        }
        Ok(Project {
            root: here,
            document: ProjectDoc::default(),
            has_file: false,
        })
    }

    /// The store root, `root / cache`.
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join(&self.document.cache)
    }

    /// The run folders' root, `root / runs`.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join(&self.document.runs)
    }

    /// `{capability: route id}` from `routes:`.
    pub fn route_defaults(&self) -> IndexMap<String, String> {
        self.document
            .routes
            .iter()
            .map(|(k, v)| (k.clone(), v.route.clone()))
            .collect()
    }
}

/// Reads and types a project file. `label` names it in messages.
fn read_project(path: &Path, label: &str) -> Result<ProjectDoc> {
    let value = crate::yaml::load_file(path, label)?;
    parse_project(&value, label)
}

/// Validates and types a project document (fx-project-v1).
pub(crate) fn parse_project(value: &Value, label: &str) -> Result<ProjectDoc> {
    check_kind(Schema::Project, value, label)?;
    if let Some(Value::String(nodes)) = value.get("nodes")
        && nodes.split('/').any(|segment| segment == "..")
    {
        return Err(Error::document(format!(
            "{label}: nodes: nodes is a folder inside the project"
        )));
    }
    validate(Schema::Project, value, label)?;
    let refuse =
        |where_: &str, message: &str| Error::document(format!("{label}: {where_}: {message}"));
    let text = |key: &str, default: &str| -> Result<String> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(default.to_string()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(refuse(key, "expected text")),
        }
    };
    let texts = |key: &str| -> Result<Vec<String>> {
        match value.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    item.as_str()
                        .map(String::from)
                        .ok_or_else(|| refuse(&format!("{key}.{i}"), "expected text"))
                })
                .collect(),
            Some(_) => Err(refuse(key, "expected a list")),
        }
    };
    let budget = match value.get("budget") {
        None | Some(Value::Null) => None,
        Some(budget) => {
            let amount = budget
                .get("max_usd")
                .ok_or_else(|| refuse("budget.max_usd", "a budget names max_usd"))?;
            let max_usd =
                Usd::from_value(amount).map_err(|message| refuse("budget.max_usd", &message))?;
            Some(Budget { max_usd })
        }
    };
    let mut routes = IndexMap::new();
    if let Some(Value::Object(entries)) = value.get("routes") {
        for (capability, entry) in entries {
            let where_ = format!("routes.{capability}");
            let default = match entry {
                Value::String(route) => RouteDefault {
                    route: route.clone(),
                    concurrency: None,
                },
                Value::Object(map) => RouteDefault {
                    route: map
                        .get("route")
                        .and_then(Value::as_str)
                        .ok_or_else(|| refuse(&where_, "a route default names its route"))?
                        .to_string(),
                    concurrency: match map.get("concurrency") {
                        None | Some(Value::Null) => None,
                        Some(Value::Number(n)) => {
                            let x = crate::value::as_f64(n);
                            if x.fract() != 0.0 || !(1.0..=1024.0).contains(&x) {
                                return Err(refuse(
                                    &format!("{where_}.concurrency"),
                                    "out of range",
                                ));
                            }
                            Some(x as u32)
                        }
                        Some(_) => {
                            return Err(refuse(
                                &format!("{where_}.concurrency"),
                                "expected a number",
                            ));
                        }
                    },
                },
                _ => return Err(refuse(&where_, "a route default is a route or a mapping")),
            };
            routes.insert(capability.clone(), default);
        }
    }
    Ok(ProjectDoc {
        runs: text("runs", "runs")?,
        cache: text("cache", ".fx/cache")?,
        budget,
        routes,
        route_tables: texts("route_tables")?,
        view_origins: texts("view_origins")?,
        sources: texts("sources")?,
        nodes: text("nodes", "nodes")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_project_document_is_typed() {
        let document = json!({
            "fx": "project/v1",
            "runs": "out/runs",
            "routes": {
                "image.generate": "img-a@acme",
                "vision.review": {"route": "vlm-c@other", "concurrency": 2},
            },
            "route_tables": ["routes.yaml"],
            "view_origins": ["https://example.test"],
            "sources": ["acme_nodes"],
            "nodes": "src/nodes",
        });
        let project = parse_project(&document, "fx.yaml").unwrap();
        assert_eq!(project.runs, "out/runs");
        assert_eq!(project.cache, ".fx/cache");
        assert_eq!(project.nodes, "src/nodes");
        assert_eq!(project.route_tables, ["routes.yaml"]);
        assert_eq!(project.sources, ["acme_nodes"]);
        assert_eq!(project.view_origins, ["https://example.test"]);
        assert_eq!(
            project.routes["vision.review"],
            RouteDefault {
                route: "vlm-c@other".into(),
                concurrency: Some(2)
            }
        );
        assert_eq!(
            project.routes["image.generate"],
            RouteDefault {
                route: "img-a@acme".into(),
                concurrency: None
            }
        );
        let defaults = parse_project(&json!({"fx": "project/v1"}), "fx.yaml").unwrap();
        assert_eq!(defaults, ProjectDoc::default());
    }

    #[test]
    fn project_refusals() {
        let message = |document: Value| parse_project(&document, "fx.yaml").unwrap_err().message;
        assert_eq!(
            message(json!({"fx": "workflow/v1"})),
            "fx.yaml: a project file starts with fx: project/v1"
        );
        assert_eq!(
            message(json!({"fx": "project/v1", "nodes": "a/../b"})),
            "fx.yaml: nodes: nodes is a folder inside the project"
        );
        assert!(
            message(json!({"fx": "project/v1", "routes": {"image.generate": "nope"}}))
                .starts_with("fx.yaml: routes.image.generate: ")
        );
        assert!(
            message(json!({"fx": "project/v1", "colour": 1})).starts_with("fx.yaml: (document): ")
        );
        assert!(
            message(json!({"fx": "project/v1", "route_tables": ["a", "a"]}))
                .starts_with("fx.yaml: route_tables: ")
        );
    }

    /// Budgets are read as money (they need [`crate::money`]).
    mod with_money {
        use super::*;

        #[test]
        fn a_project_budget_is_micro_dollars() {
            let document = json!({"fx": "project/v1", "budget": {"max_usd": 2}});
            let project = parse_project(&document, "fx.yaml").unwrap();
            assert_eq!(
                project.budget,
                Some(Budget {
                    max_usd: Usd(2_000_000)
                })
            );
        }
    }

    #[test]
    fn find_walks_up_and_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // No fx.yaml anywhere in a fresh temporary folder (its ancestors are system folders).
        let bare = Project::find(&root).unwrap();
        assert_eq!(bare.root, root);
        assert!(!bare.has_file);
        assert_eq!(bare.document, ProjectDoc::default());
        // A folder named fx.yaml is skipped; a file under it starts at its folder.
        std::fs::create_dir_all(root.join("a/fx.yaml")).unwrap();
        std::fs::write(root.join("a/w.yaml"), "x").unwrap();
        let skipped = Project::find(&root.join("a/w.yaml")).unwrap();
        assert_eq!(skipped.root, root.join("a"));
        assert!(!skipped.has_file);
        // A start that does not exist is still walked from.
        let missing = Project::find(&root.join("a/missing/deeper")).unwrap();
        assert_eq!(missing.root, root.join("a/missing/deeper"));
        assert_eq!(missing.cache_dir(), root.join("a/missing/deeper/.fx/cache"));
    }
}
