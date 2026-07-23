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
#[serde(untagged)]
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

    fn parse_yaml(s: &str) -> Config {
        yaml_serde::from_str(s).expect("parse yaml")
    }

    fn parse_json5(s: &str) -> Config {
        json5::from_str(s).expect("parse json5")
    }

    #[test]
    fn yaml_simple_command() {
        let cfg = parse_yaml(r#""*.go": "gofmt -w""#);
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "gofmt -w");
        assert!(cmds[0].pass_filenames);
    }

    #[test]
    fn yaml_command_list() {
        let cfg = parse_yaml(
            r#"
"*.md":
  - prettier --write
  - markdownlint
"#,
        );
        let cmds = &cfg["*.md"];
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "prettier --write");
        assert!(cmds[0].pass_filenames);
        assert_eq!(cmds[1].command, "markdownlint");
        assert!(cmds[1].pass_filenames);
    }

    #[test]
    fn yaml_command_object() {
        let cfg = parse_yaml(
            r#"
"*.go":
  command: "go vet ./..."
  pass_filenames: false
"#,
        );
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "go vet ./...");
        assert!(!cmds[0].pass_filenames);
    }

    #[test]
    fn yaml_object_pass_filenames_defaults_true() {
        let cfg = parse_yaml(
            r#"
"*.go":
  command: "gofmt -w"
"#,
        );
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].pass_filenames);
    }

    #[test]
    fn yaml_mixed_list() {
        let cfg = parse_yaml(
            r#"
"*.go":
  - "goimports -w"
  - command: "golangci-lint run --fix"
    pass_filenames: false
"#,
        );
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "goimports -w");
        assert!(cmds[0].pass_filenames);
        assert_eq!(cmds[1].command, "golangci-lint run --fix");
        assert!(!cmds[1].pass_filenames);
    }

    #[test]
    fn json_simple_command() {
        let cfg = parse_json5(r#"{"*.go": "gofmt -w"}"#);
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "gofmt -w");
        assert!(cmds[0].pass_filenames);
    }

    #[test]
    fn json5_with_comments() {
        let cfg = parse_json5(
            r#"{
            // Format Go files
            "*.go": "gofmt -w",
            /* Lint markdown */
            "*.md": "markdownlint",
        }"#,
        );
        assert_eq!(cfg["*.go"][0].command, "gofmt -w");
        assert_eq!(cfg["*.md"][0].command, "markdownlint");
    }

    #[test]
    fn json5_trailing_commas() {
        let cfg = parse_json5(
            r#"{
            "*.go": "gofmt -w",
            "*.md": "markdownlint",
        }"#,
        );
        assert_eq!(cfg.len(), 2);
    }

    #[test]
    fn json_command_object_with_pass_filenames() {
        let cfg = parse_json5(r#"{"*.go": {"command": "go vet ./...", "pass_filenames": false}}"#);
        let cmds = &cfg["*.go"];
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "go vet ./...");
        assert!(!cmds[0].pass_filenames);
    }

    #[test]
    fn json_command_list() {
        let cfg = parse_json5(r#"{"*.go": ["goimports -w", "gofmt -w"]}"#);
        assert_eq!(cfg["*.go"].len(), 2);
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
    fn load_yaml_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".stagelint.yml");
        fs::write(&path, r#""*.go": "gofmt -w""#).expect("write");
        let cfg = load_file(&path).expect("load");
        assert!(cfg.contains_key("*.go"));
    }

    #[test]
    fn load_json_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".stagelint.json");
        fs::write(&path, r#"{"*.go": "gofmt -w"}"#).expect("write");
        let cfg = load_file(&path).expect("load");
        assert!(cfg.contains_key("*.go"));
    }

    #[test]
    fn load_json5_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".stagelint.json5");
        fs::write(
            &path,
            r#"{
                // Go formatting
                "*.go": "gofmt -w",
            }"#,
        )
        .expect("write");
        let cfg = load_file(&path).expect("load");
        assert!(cfg.contains_key("*.go"));
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
