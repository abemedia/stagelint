use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid glob pattern '{pattern}'")]
    InvalidGlob {
        pattern: String,
        #[source]
        source: globset::Error,
    },
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
    #[error("invalid config {path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

/// One runner task per config pattern with matching files.
pub struct Task {
    pub commands: Vec<CommandObject>,
    pub files: Vec<OsString>,
    pub cwd: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandObject {
    pub command: String,
    #[serde(default = "default_true")]
    pub pass_filenames: bool,
}

impl From<String> for CommandObject {
    fn from(command: String) -> Self {
        CommandObject {
            command,
            pass_filenames: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Group `paths` by their nearest config file, load each, and match its patterns.
/// Tasks are ordered by config path, then by pattern declaration order.
pub fn resolve<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    workdir: &Path,
) -> Result<Vec<Task>, Error> {
    let mut cache = HashMap::new();
    let mut by_config: BTreeMap<PathBuf, Vec<&Path>> = BTreeMap::new();
    for path in paths {
        let dir = path.parent().unwrap_or(Path::new(""));
        if let Some(config_path) = find(workdir, dir, &mut cache) {
            by_config.entry(config_path).or_default().push(path);
        }
    }

    let mut tasks = Vec::new();
    for (config_path, paths) in &by_config {
        let cfg = load_file(&workdir.join(config_path))?;
        let prefix = config_path.parent().unwrap_or(Path::new(""));

        for (pattern, entry) in cfg {
            // Bare patterns match basenames at any depth.
            let glob = if pattern.contains('/') {
                pattern.clone()
            } else {
                format!("**/{pattern}")
            };
            let matcher = globset::GlobBuilder::new(&glob)
                .literal_separator(true)
                .build()
                .map_err(|source| Error::InvalidGlob {
                    pattern: pattern.clone(),
                    source,
                })?
                .compile_matcher();

            let files: Vec<OsString> = paths
                .iter()
                .filter(|path| {
                    let relative = path.strip_prefix(prefix).unwrap_or(path);
                    matcher.is_match(relative)
                })
                .map(|path| workdir.join(path).into_os_string())
                .collect();

            if files.is_empty() {
                continue;
            }

            tasks.push(Task {
                commands: entry,
                files,
                cwd: workdir.join(prefix),
            });
        }
    }
    Ok(tasks)
}

const CONFIG_FILES: &[&str] = &[
    ".stagelint.yml",
    ".stagelint.yaml",
    ".stagelint.json",
    ".stagelint.jsonc",
    ".stagelint.json5",
];

/// Search for a config file starting from the repo-relative `start`, walking up to the repo root.
/// Caches results for all visited directories so repeated lookups in the same subtree are free.
fn find(
    workdir: &Path,
    start: &Path,
    cache: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> Option<PathBuf> {
    if let Some(cached) = cache.get(start) {
        return cached.clone();
    }

    let mut visited = Vec::new();
    let mut probe = workdir.join(start);
    let mut dir = start;
    let result = 'walk: loop {
        if let Some(cached) = cache.get(dir) {
            break cached.clone();
        }
        visited.push(dir.to_path_buf());
        for name in CONFIG_FILES {
            probe.push(name);
            if probe.is_file() {
                break 'walk Some(dir.join(name));
            }
            probe.pop();
        }
        if dir.as_os_str().is_empty() {
            break None;
        }
        dir = dir.parent().unwrap_or(Path::new(""));
        probe.pop();
    };

    for dir in visited {
        cache.insert(dir, result.clone());
    }

    result
}

type Config = IndexMap<String, Vec<CommandObject>>;
type ConfigAs = serde_with::MapPreventDuplicates<
    serde_with::Same,
    serde_with::OneOrMany<serde_with::PickFirst<(serde_with::Same, serde_with::FromInto<String>)>>,
>;
type ConfigWrap = serde_with::de::DeserializeAsWrap<Config, ConfigAs>;

fn load_file(path: &Path) -> Result<Config, Error> {
    let content = fs::read_to_string(path).map_err(|e| Error::Read {
        path: path.to_owned(),
        source: e,
    })?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let cfg: Config = match ext {
        "yml" | "yaml" => yaml_serde::from_str::<ConfigWrap>(&content)
            .map_err(|e| Error::ParseYaml {
                path: path.to_owned(),
                source: e,
            })?
            .into_inner(),
        "json" | "jsonc" | "json5" => json5::from_str::<ConfigWrap>(&content)
            .map_err(|e| Error::ParseJson {
                path: path.to_owned(),
                source: e,
            })?
            .into_inner(),
        _ => unreachable!("no parser for {}", path.display()),
    };

    for (pattern, commands) in &cfg {
        let message = if commands.is_empty() {
            format!("pattern \"{pattern}\" has no commands")
        } else if commands.iter().any(|obj| obj.command.trim().is_empty()) {
            format!("empty command in pattern \"{pattern}\"")
        } else {
            continue;
        };
        return Err(Error::Invalid {
            path: path.to_owned(),
            message,
        });
    }

    Ok(cfg)
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
        let Err(err) = result else {
            panic!("misspelled key must fail, not silently default");
        };
        let source = std::error::Error::source(&err).expect("source").to_string();
        assert!(
            source.contains("pass_filename"),
            "the error should name the unknown field, got: {source}"
        );
    }

    #[test]
    fn duplicate_pattern_rejected() {
        let result = load(".stagelint.yml", "\"*.js\": eslint\n\"*.js\": prettier\n");
        assert!(result.is_err(), "a duplicated key must fail, not last-wins");
    }

    #[test]
    fn empty_commands_rejected() {
        assert!(load(".stagelint.yml", "\"*.js\": []\n").is_err());
        assert!(load(".stagelint.yml", "\"*.js\": \"\"\n").is_err());
    }

    #[test]
    fn every_config_file_name_has_a_parser() {
        for name in CONFIG_FILES {
            let _ = load(name, "");
        }
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

    /// Bare patterns match basenames at any depth; patterns with `/` do not cross directories.
    #[test]
    fn glob_pattern_matching() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join(".stagelint.yml"),
            "\"*.txt\": \"a\"\n\"cmd*.go\": \"b\"\n\"sub/*.md\": \"c\"\n",
        )
        .expect("write");

        let paths: Vec<PathBuf> = [
            "root.txt",
            "sub/nested.txt",
            "readme.md",
            "sub/notes.md",
            "sub/deep/notes.md",
            "pkg/cmdfoo.go",
            "cmdutil/main.go",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();

        let tasks = resolve(paths.iter().map(PathBuf::as_path), dir.path()).expect("resolve");
        let matched = |task: &Task| -> Vec<String> {
            task.files
                .iter()
                .map(|f| {
                    Path::new(f)
                        .strip_prefix(dir.path())
                        .expect("under temp dir")
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect()
        };

        assert_eq!(tasks.len(), 3);
        assert_eq!(matched(&tasks[0]), ["root.txt", "sub/nested.txt"]);
        assert_eq!(matched(&tasks[1]), ["pkg/cmdfoo.go"]);
        assert_eq!(matched(&tasks[2]), ["sub/notes.md"]);
    }

    /// An anchored pattern is relative to its own config, not the repo root.
    #[test]
    fn nested_config_anchors_to_its_own_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let web = dir.path().join("packages/web");
        fs::create_dir_all(&web).expect("mkdir");
        fs::write(web.join(".stagelint.yml"), "\"src/*.ts\": \"a\"\n").expect("write");

        let paths: Vec<PathBuf> = ["packages/web/src/a.ts", "packages/web/lib/b.ts"]
            .iter()
            .map(PathBuf::from)
            .collect();

        let tasks = resolve(paths.iter().map(PathBuf::as_path), dir.path()).expect("resolve");

        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].files,
            [dir.path().join("packages/web/src/a.ts").into_os_string()]
        );
    }

    #[test]
    fn find_not_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(find(dir.path(), Path::new(""), &mut HashMap::new()).is_none());
    }

    #[test]
    fn find_walks_up_to_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "gofmt -w""#).expect("write");
        let child = dir.path().join("packages").join("foo");
        fs::create_dir_all(&child).expect("mkdir");
        let found = find(dir.path(), Path::new("packages/foo"), &mut HashMap::new()).expect("find");
        assert_eq!(found, Path::new(".stagelint.yml"));
    }

    #[test]
    fn find_prefers_closest() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "root""#).expect("write");
        let child = dir.path().join("packages").join("foo");
        fs::create_dir_all(&child).expect("mkdir");
        fs::write(child.join(".stagelint.yml"), r#""*.go": "child""#).expect("write");
        let found = find(dir.path(), Path::new("packages/foo"), &mut HashMap::new()).expect("find");
        assert_eq!(found, Path::new("packages/foo/.stagelint.yml"));
    }

    #[test]
    fn find_stops_at_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("repo");
        let parent = root.join("packages");
        let child = parent.join("foo");
        fs::create_dir_all(&child).expect("mkdir");
        fs::write(dir.path().join(".stagelint.yml"), r#""*.go": "above""#).expect("write");
        assert!(find(&root, Path::new("packages/foo"), &mut HashMap::new()).is_none());
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
            find(dir.path(), Path::new("packages/a"), &mut cache).expect("find a"),
            Path::new("packages/a/.stagelint.yml")
        );
        assert_eq!(
            find(dir.path(), Path::new("packages/b"), &mut cache).expect("find b"),
            Path::new(".stagelint.yml")
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
        let found = find(dir.path(), Path::new(""), &mut HashMap::new()).expect("find");
        let cfg = load_file(&dir.path().join(found)).expect("load");
        assert_eq!(cfg["*.go"][0].command, "from-yml");
    }
}
