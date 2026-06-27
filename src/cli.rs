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
    /// Continue running all linters even if one fails, then report all errors together.
    #[arg(long)]
    pub continue_on_error: bool,

    /// Run tasks concurrently. Pass false to run sequentially, or a number to limit concurrency.
    #[arg(long, default_value = "true", value_parser = parse_concurrent)]
    pub concurrent: usize,

    /// Control stash scope. Without value: stash everything. Values: tracked, untracked, all.
    #[arg(long, value_enum, default_missing_value = "all", num_args = 0..=1, default_value_t)]
    pub stash: StashScope,

    /// Suppress warnings.
    #[arg(long)]
    pub quiet: bool,
}

#[derive(Clone, Default, ValueEnum)]
pub enum StashScope {
    /// Only stash partially-staged files (default).
    #[default]
    Partial,
    /// Also stash dirty tracked files.
    Tracked,
    /// Also stash untracked files.
    Untracked,
    /// Stash everything.
    All,
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
