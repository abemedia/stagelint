use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;

pub type Config = IndexMap<String, Commands>;

#[derive(Debug, Deserialize)]
#[serde(from = "RawEntry")]
pub struct Commands(Vec<CommandObject>);

impl std::ops::Deref for Commands {
    type Target = [CommandObject];

    fn deref(&self) -> &[CommandObject] {
        &self.0
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawEntry {
    Single(CommandObject),
    List(Vec<CommandObject>),
}

impl From<RawEntry> for Commands {
    fn from(raw: RawEntry) -> Self {
        match raw {
            RawEntry::Single(obj) => Commands(vec![obj]),
            RawEntry::List(items) => Commands(items),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(from = "RawCommand")]
pub struct CommandObject {
    pub command: String,
    pub pass_filenames: bool,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum RawCommand {
    Simple(String),
    Object {
        command: String,
        #[serde(default = "default_true")]
        pass_filenames: bool,
    },
}

impl From<RawCommand> for CommandObject {
    fn from(raw: RawCommand) -> Self {
        match raw {
            RawCommand::Simple(command) => CommandObject {
                command,
                pass_filenames: true,
            },
            RawCommand::Object {
                command,
                pass_filenames,
            } => CommandObject {
                command,
                pass_filenames,
            },
        }
    }
}

fn default_true() -> bool {
    true
}

const CONFIG_FILES: &[&str] = &[
    ".stagelint.yml",
    ".stagelint.yaml",
    ".stagelint.json",
    ".stagelint.jsonc",
    ".stagelint.json5",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no config file found (looked for {})", CONFIG_FILES.join(", "))]
    NotFound,
    #[error("failed to read {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}")]
    ParseYaml {
        path: PathBuf,
        #[source]
        source: yaml_serde::Error,
    },
    #[error("failed to parse {path}")]
    ParseJson {
        path: PathBuf,
        #[source]
        source: json5::Error,
    },
    #[error("unsupported config file extension: {}", path.display())]
    UnsupportedExtension { path: PathBuf },
}

/// Search for a config file starting from `start` and walking up to `root` (inclusive).
/// Caches results for all visited directories so repeated lookups in the same subtree are free.
pub fn find(
    start: &Path,
    root: &Path,
    cache: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> Option<PathBuf> {
    let mut visited = Vec::new();
    let mut dir = start;
    let result = 'walk: loop {
        if let Some(cached) = cache.get(dir) {
            break cached.clone();
        }
        for name in CONFIG_FILES {
            let path = dir.join(name);
            if path.is_file() {
                visited.push(dir.to_path_buf());
                break 'walk Some(path);
            }
        }
        visited.push(dir.to_path_buf());
        if dir == root {
            break None;
        }
        dir = match dir.parent() {
            Some(p) => p,
            None => break None,
        };
    };

    for dir in visited {
        cache.insert(dir, result.clone());
    }

    result
}

pub fn load_file(path: &Path) -> Result<Config, Error> {
    let content = fs::read_to_string(path).map_err(|e| Error::Read {
        path: path.to_owned(),
        source: e,
    })?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "yml" | "yaml" => yaml_serde::from_str(&content).map_err(|e| Error::ParseYaml {
            path: path.to_owned(),
            source: e,
        }),
        "json" | "jsonc" | "json5" => json5::from_str(&content).map_err(|e| Error::ParseJson {
            path: path.to_owned(),
            source: e,
        }),
        _ => Err(Error::UnsupportedExtension {
            path: path.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str, content: &str) -> Result<Config, Error> {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        fs::write(&path, content).expect("write");
        load_file(&path)
    }

    // Each shape is parsed on a different extension, covering both in one pass.

    #[test]
    fn simple_command() {
        let cfg = load(".stagelint.yml", r#""*.go": "gofmt -w""#).expect("load");
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "gofmt -w");
        assert!(cmds[0].pass_filenames);
    }

    #[test]
    fn command_list() {
        let cfg = load(
            ".stagelint.yaml",
            r#"
"*.md":
  - prettier --write
  - markdownlint
"#,
        )
        .expect("load");
        let cmds = &cfg["*.md"];
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "prettier --write");
        assert_eq!(cmds[1].command, "markdownlint");
    }

    #[test]
    fn command_object() {
        let cfg = load(
            ".stagelint.json",
            r#"{
                "*.go": {"command": "go vet ./...", "pass_filenames": false},
                "*.md": {"command": "markdownlint"}
            }"#,
        )
        .expect("load");
        assert_eq!(cfg["*.go"][0].command, "go vet ./...");
        assert!(!cfg["*.go"][0].pass_filenames);
        assert!(
            cfg["*.md"][0].pass_filenames,
            "pass_filenames defaults to true"
        );
    }

    #[test]
    fn mixed_list() {
        let cfg = load(
            ".stagelint.jsonc",
            r#"{
                "*.go": [
                    "goimports -w",
                    {"command": "golangci-lint run --fix", "pass_filenames": false}
                ]
            }"#,
        )
        .expect("load");
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "goimports -w");
        assert!(cmds[0].pass_filenames);
        assert_eq!(cmds[1].command, "golangci-lint run --fix");
        assert!(!cmds[1].pass_filenames);
    }

    #[test]
    fn json5_syntax() {
        let cfg = load(
            ".stagelint.json5",
            r#"{
                // Format Go files
                "*.go": "gofmt -w",
                /* Lint markdown */
                "*.md": "markdownlint",
            }"#,
        )
        .expect("load");
        assert_eq!(cfg["*.go"][0].command, "gofmt -w");
        assert_eq!(cfg["*.md"][0].command, "markdownlint");
    }

    #[test]
    fn unknown_field_rejected() {
        let result = load(
            ".stagelint.json",
            r#"{"*.go": {"command": "gofmt -w", "pass_filename": false}}"#,
        );
        assert!(
            result.is_err(),
            "misspelled key must fail, not silently default"
        );
    }

    #[test]
    fn unsupported_extension_rejected() {
        let result = load("stagelint.toml", r#""*.go" = "gofmt -w""#);
        assert!(matches!(result, Err(Error::UnsupportedExtension { .. })));
    }

    #[test]
    fn declaration_order_preserved() {
        let cfg = load(
            ".stagelint.yml",
            "\"z.txt\": \"third\"\n\"a.txt\": \"first\"\n\"m.txt\": \"second\"\n",
        )
        .expect("load");
        let keys: Vec<&str> = cfg.keys().map(String::as_str).collect();
        assert_eq!(keys, ["z.txt", "a.txt", "m.txt"]);
    }

    #[test]
    fn find_not_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(find(dir.path(), dir.path(), &mut HashMap::new()).is_none());
    }

    #[test]
    fn find_walks_up_to_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "gofmt -w""#).expect("write");
        let child = dir.path().join("packages").join("foo");
        fs::create_dir_all(&child).expect("mkdir");
        let found = find(&child, dir.path(), &mut HashMap::new()).expect("find");
        assert_eq!(found, dir.path().join(".stagelint.yml"));
    }

    #[test]
    fn find_prefers_closest() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "root""#).expect("write");
        let child = dir.path().join("packages").join("foo");
        fs::create_dir_all(&child).expect("mkdir");
        fs::write(child.join(".stagelint.yml"), r#""*.go": "child""#).expect("write");
        let found = find(&child, dir.path(), &mut HashMap::new()).expect("find");
        assert_eq!(found, child.join(".stagelint.yml"));
    }

    #[test]
    fn find_stops_at_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("repo");
        let parent = root.join("packages");
        let child = parent.join("foo");
        fs::create_dir_all(&child).expect("mkdir");
        // Config exists above root - should not be found
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "above""#).expect("write");
        assert!(find(&child, &root, &mut HashMap::new()).is_none());
    }

    #[test]
    fn find_cache_keeps_lookups_independent() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "root""#).expect("write");
        let a = dir.path().join("packages").join("a");
        let b = dir.path().join("packages").join("b");
        fs::create_dir_all(&a).expect("mkdir");
        fs::create_dir_all(&b).expect("mkdir");
        fs::write(a.join(".stagelint.yml"), r#""*.go": "a""#).expect("write");

        // a's walk must not poison the shared dirs with a's config.
        let mut cache = HashMap::new();
        assert_eq!(
            find(&a, dir.path(), &mut cache).expect("find a"),
            a.join(".stagelint.yml")
        );
        assert_eq!(
            find(&b, dir.path(), &mut cache).expect("find b"),
            dir.path().join(".stagelint.yml")
        );
    }

    #[test]
    fn load_priority_yml_over_json() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "from-yml""#).expect("write yml");
        fs::write(
            dir.path().join(".stagelint.json"),
            r#"{"*.go": "from-json"}"#,
        )
        .expect("write json");
        let found = find(dir.path(), dir.path(), &mut HashMap::new()).expect("find");
        let cfg = load_file(&found).expect("load");
        assert_eq!(cfg["*.go"][0].command, "from-yml");
    }
}
