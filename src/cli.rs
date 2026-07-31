use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    version,
    about = "Run linters and formatters on staged git files",
    args_conflicts_with_subcommands = true
)]
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

/// Run linters and formatters on staged git files.
#[derive(Args)]
pub struct Opts {
    /// Run all tasks to completion even if one fails, then report all errors together.
    #[arg(long)]
    pub continue_on_error: bool,

    /// Limit how many tasks run at once: a number, true or 0 (unlimited), or false (sequential).
    #[arg(long, default_value = "true", value_parser = parse_concurrent)]
    pub concurrent: usize,

    /// Control stash scope; each value includes the previous.
    #[arg(long, value_enum, default_value_t)]
    pub stash: StashScope,

    /// Suppress warnings.
    #[arg(long)]
    pub quiet: bool,
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
        _ => s
            .parse::<usize>()
            .map_err(|_| format!("expected true, false, or a number, got '{s}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_concurrent;

    #[test]
    fn parse_concurrent_values() {
        assert_eq!(parse_concurrent("true"), Ok(0));
        assert_eq!(parse_concurrent("false"), Ok(1));
        assert_eq!(parse_concurrent("4"), Ok(4));
        assert_eq!(parse_concurrent("0"), Ok(0));
    }

    #[test]
    fn parse_concurrent_invalid() {
        let err = parse_concurrent("maybe").unwrap_err();
        assert_eq!(err, "expected true, false, or a number, got 'maybe'");
    }
}
