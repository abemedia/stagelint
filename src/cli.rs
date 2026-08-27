use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[command(flatten)]
    pub opts: Opts,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Install the pre-commit hook.
    Init {
        /// Overwrite an existing pre-commit hook.
        #[arg(long)]
        force: bool,
    },
}

/// Run commands like linters and formatters on staged git files.
#[derive(Args)]
#[expect(clippy::struct_excessive_bools, reason = "one field per CLI flag")]
pub struct Opts {
    /// Run all tasks to completion even if one fails, then report all errors together.
    #[arg(long)]
    pub continue_on_error: bool,

    /// Run at most this many tasks at once, or false to run them sequentially.
    #[arg(long, default_value = "true", value_name = "NUM|BOOL", value_parser = parse_concurrent)]
    pub concurrent: usize,

    /// Lint the files changed in a revision range instead of the staged files.
    ///
    /// Takes any range `git diff` does - `main...HEAD` for everything on the branch since it
    /// diverged, or a single revision such as `HEAD~3` to compare against HEAD. Results are staged,
    /// as in the default mode.
    #[arg(long, value_name = "REVSPEC", group = "source")]
    pub diff: Option<String>,

    /// Lint the files modified in the working tree, including untracked ones.
    ///
    /// Nothing is stashed and nothing is staged: the commands see the working tree as it is, and
    /// their changes are left there.
    #[arg(long, group = "source")]
    pub unstaged: bool,

    /// Lint the given paths instead of the staged files.
    ///
    /// A path is skipped rather than failing the run if it does not exist, is a directory or a
    /// symlink, or lies outside the repository. Ignored files are linted like any other. Nothing
    /// is stashed and nothing is staged, as with `--unstaged`.
    #[arg(long, value_name = "PATHS", num_args = 1.., group = "source")]
    pub files: Vec<PathBuf>,

    /// Control stash scope; each value includes the previous.
    #[arg(long, value_enum, default_value_t, conflicts_with_all = ["unstaged", "files"])]
    pub stash: StashScope,

    /// Print only the output of failed commands and errors.
    #[arg(long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Show the output of every command, not only of those that failed.
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Clone, Default, ValueEnum)]
pub enum StashScope {
    /// Only stash partially-staged files.
    #[default]
    Partial,
    /// Also stash dirty tracked files.
    Tracked,
    /// Also stash untracked files.
    Untracked,
}

fn parse_concurrent(s: &str) -> Result<usize, String> {
    match s {
        "true" => Ok(0), // 0 means unlimited.
        "false" => Ok(1),
        _ => match s.parse::<usize>() {
            Ok(0) | Err(_) => Err("expected true, false, or a positive number".into()),
            Ok(n) => Ok(n),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, parse_concurrent};
    use clap::Parser;

    /// `args_conflicts_with_subcommands` means run flags cannot be combined with `init`.
    #[test]
    fn init_rejects_run_flags() {
        assert!(Cli::try_parse_from(["stagelint", "--quiet", "init"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "--concurrent", "false", "init"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "init", "--force"]).is_ok());
    }

    #[test]
    fn sources_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["stagelint", "--diff", "HEAD~1", "--unstaged"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "--diff", "HEAD~1", "--files", "a"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "--unstaged", "--files", "a"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "--unstaged"]).is_ok());
    }

    /// Nothing is stashed for the sources that stage nothing, so asking is a mistake.
    #[test]
    fn stash_rejects_sources_that_stage_nothing() {
        assert!(Cli::try_parse_from(["stagelint", "--unstaged", "--stash", "tracked"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "--files", "a", "--stash", "tracked"]).is_err());
        assert!(Cli::try_parse_from(["stagelint", "--stash", "tracked"]).is_ok());
    }

    #[test]
    fn parse_concurrent_values() {
        assert_eq!(parse_concurrent("true"), Ok(0));
        assert_eq!(parse_concurrent("false"), Ok(1));
        assert_eq!(parse_concurrent("4"), Ok(4));
    }

    #[test]
    fn parse_concurrent_invalid() {
        assert!(parse_concurrent("maybe").is_err());
        assert!(parse_concurrent("0").is_err());
    }
}
