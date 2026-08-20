//! A nested task logger: one handle per row, any depth, drawn as a live tree when stderr is a
//! terminal and as a log of status changes otherwise. Callers add rows and set their status.

use std::io::{self, Write};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use console::{StyledObject, style};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

struct Glyphs {
    tick: &'static str,
    cross: &'static str,
    warning: &'static str,
    square: &'static str,
    arrow: &'static str,
    ban: &'static str,
    dash: &'static str,
    spinner: &'static [&'static str],
}

const UNICODE: Glyphs = Glyphs {
    tick: "✔",
    cross: "✖",
    warning: "⚠",
    square: "◼",
    arrow: "❯",
    ban: "⊘",
    dash: "—",
    spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
};

/// What the legacy Windows console can draw.
const ASCII: Glyphs = Glyphs {
    tick: "√",
    cross: "×",
    warning: "‼",
    square: "■",
    arrow: ">",
    ban: "-",
    dash: "-",
    spinner: &["-", "\\", "|", "/"],
};

/// Unicode unless the Linux console, or a Windows terminal not known to render it.
fn glyphs() -> &'static Glyphs {
    static GLYPHS: std::sync::OnceLock<&Glyphs> = std::sync::OnceLock::new();
    GLYPHS.get_or_init(|| {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        let unicode = if cfg!(windows) {
            !var("CI").is_empty()
                || !var("WT_SESSION").is_empty()
                || !var("TERMINUS_SUBLIME").is_empty()
                || var("ConEmuTask") == "{cmd::Cmder}"
                || matches!(var("TERM_PROGRAM").as_str(), "Terminus-Sublime" | "vscode")
                || matches!(
                    var("TERM").as_str(),
                    "xterm-256color" | "alacritty" | "rxvt-unicode" | "rxvt-unicode-256color"
                )
                || var("TERMINAL_EMULATOR") == "JetBrains-JediTerm"
        } else {
            var("TERM") != "linux"
        };
        if unicode { &UNICODE } else { &ASCII }
    })
}

const TICK_RATE: Duration = Duration::from_millis(80);
/// The root row, never drawn.
const ROOT: usize = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Only failed leaves and their output.
    Quiet,
    Normal,
    /// Every row's output, no collapsing.
    Verbose,
}

/// Ordered by severity, so the worst of several is their `max`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    #[default]
    Pending,
    Running,
    Done,
    Warn,
    Cancelled,
    Failed,
}

impl Status {
    fn settled(self) -> bool {
        !matches!(self, Status::Pending | Status::Running)
    }

    /// Whether the row's output is worth reading on its own.
    fn notable(self) -> bool {
        matches!(self, Status::Warn | Status::Failed)
    }

    fn tag(self) -> Option<&'static str> {
        Some(match self {
            Status::Running => "STARTED",
            Status::Warn => "WARN",
            Status::Done => "COMPLETED",
            Status::Cancelled => "CANCELLED",
            Status::Failed => "FAILED",
            Status::Pending => return None,
        })
    }
}

/// A handle to one row; every clone is the same row.
#[derive(Clone)]
pub struct Reporter {
    shared: Arc<Shared>,
    id: usize,
}

struct Shared {
    /// Shared with the ticker thread; not `Shared` itself, or `Drop` could run there and join itself.
    state: Arc<Mutex<State>>,
    /// The redraw thread; `None` when not drawing a tree.
    ticker: Option<(mpsc::Sender<()>, thread::JoinHandle<()>)>,
}

struct State {
    level: Level,
    mode: Mode,
    rows: Vec<Row>,
}

enum Mode {
    /// A live tree drawn by indicatif.
    Tree {
        progress: MultiProgress,
        out: Box<dyn Write + Send>,
    },
    /// One line per status change.
    Plain {
        out: Box<dyn Write + Send>,
        /// Glyphs instead of tags, which only `Quiet` reaches.
        terminal: bool,
    },
}

#[derive(Default)]
struct Row {
    title: String,
    note: String,
    status: Status,
    parent: Option<usize>,
    children: Vec<usize>,
    depth: usize,
    bar: Option<ProgressBar>,
    /// Printed when the row settles, or below the finished tree.
    output: Vec<u8>,
}

impl Reporter {
    /// The root handle, drawing to stderr.
    pub fn new(level: Level) -> Reporter {
        Reporter::custom(
            level,
            ProgressDrawTarget::stderr(),
            Box::new(io::stderr()),
            Some(TICK_RATE),
        )
    }

    /// A reporter drawing to `target`, or printing to `plain` when it is hidden or `Quiet`.
    /// Without a `tick` interval nothing advances the spinners; call `tick` instead.
    pub fn custom(
        level: Level,
        target: ProgressDrawTarget,
        plain: Box<dyn Write + Send>,
        tick: Option<Duration>,
    ) -> Reporter {
        let progress = MultiProgress::with_draw_target(target);
        let terminal = !progress.is_hidden();
        let tree = terminal && level != Level::Quiet;
        let mode = if tree {
            Mode::Tree {
                progress,
                out: plain,
            }
        } else {
            Mode::Plain {
                out: plain,
                terminal,
            }
        };
        let state = Arc::new(Mutex::new(State {
            level,
            mode,
            rows: vec![Row::default()],
        }));
        let ticker = tick.filter(|_| tree).map(|tick| {
            let (tx, rx) = mpsc::channel::<()>();
            let state = state.clone();
            let handle = thread::spawn(move || {
                while rx.recv_timeout(tick) == Err(mpsc::RecvTimeoutError::Timeout) {
                    state.lock().unwrap_or_else(PoisonError::into_inner).tick();
                }
            });
            (tx, handle)
        });
        Reporter {
            shared: Arc::new(Shared { state, ticker }),
            id: ROOT,
        }
    }

    /// Add a child row after the existing ones.
    pub fn add(&self, title: impl Into<String>) -> Reporter {
        let mut state = self.state();
        let depth = state.rows[self.id].depth + 1;
        state.rows.push(Row {
            title: title.into(),
            parent: Some(self.id),
            depth,
            ..Row::default()
        });
        let id = state.rows.len() - 1;
        state.rows[self.id].children.push(id);
        Reporter {
            shared: self.shared.clone(),
            id,
        }
    }

    pub fn status(&self, status: Status) -> Reporter {
        self.state().set_status(self.id, status);
        self.clone()
    }

    pub fn title(&self, title: impl Into<String>) -> Reporter {
        let mut state = self.state();
        state.rows[self.id].title = title.into();
        state.draw(self.id);
        self.clone()
    }

    /// Dim text after the title, behind a dash.
    pub fn note(&self, note: impl Into<String>) -> Reporter {
        let mut state = self.state();
        state.rows[self.id].note = note.into();
        state.draw(self.id);
        self.clone()
    }

    /// Append to the row's output, printed for failed rows or, under `Verbose`, every row.
    pub fn output(&self, bytes: impl AsRef<[u8]>) -> Reporter {
        let mut state = self.state();
        state.rows[self.id].output.extend_from_slice(bytes.as_ref());
        // The settle line went out without it, so this needs its own header.
        if state.rows[self.id].status.settled() {
            state.flush_output(self.id, true);
        }
        self.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some((tx, handle)) = self.ticker.take() {
            drop(tx);
            handle.join().ok();
        }
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .finish();
    }
}

impl State {
    /// Change a row's status and draw or print the transition.
    fn set_status(&mut self, id: usize, status: Status) {
        if self.rows[id].status == status {
            return;
        }
        self.rows[id].status = status;
        match &mut self.mode {
            Mode::Tree { .. } => {
                self.reconcile();
                self.draw(id);
            }
            Mode::Plain { .. } => {
                if status.tag().is_some() && !self.muted(id) {
                    self.print_line(id);
                }
            }
        }
        if status.settled() {
            self.flush_output(id, false);
        }
    }

    /// Quiet says only what broke, not the groups whose children already said it.
    fn muted(&self, id: usize) -> bool {
        let row = &self.rows[id];
        self.level == Level::Quiet
            && (row.status != Status::Failed
                || row
                    .children
                    .iter()
                    .any(|&c| self.rows[c].status == Status::Failed))
    }

    /// The row's line: `[TAG] path note`, or glyph and path on a terminal.
    fn print_line(&mut self, id: usize) {
        let term = matches!(self.mode, Mode::Plain { terminal: true, .. });
        let line = if term {
            let sep = format!(" {} ", glyphs().arrow);
            format!(
                "{} {}{}",
                glyph(&self.rows, id),
                path(&self.rows, id, &sep),
                trailer(&self.rows, id)
            )
        } else {
            let tag = self.rows[id].status.tag().unwrap_or_default();
            format!(
                "[{tag}] {}{}",
                path(&self.rows, id, " > "),
                trailer(&self.rows, id)
            )
        };
        if let Mode::Plain { out, .. } = &mut self.mode {
            writeln!(out, "{line}").ok();
            out.flush().ok();
        }
    }

    /// Print and release a row's output, under its own header when asked. A tree holds output it
    /// will print until `finish`.
    fn flush_output(&mut self, id: usize, header: bool) {
        let show = self.rows[id].status.notable() || self.level == Level::Verbose;
        if !show || self.muted(id) {
            self.rows[id].output = Vec::new();
            return;
        }
        if self.rows[id].output.is_empty() || matches!(self.mode, Mode::Tree { .. }) {
            return;
        }
        if header {
            self.print_line(id);
        }
        if let Mode::Plain { out, .. } = &mut self.mode {
            let row = &mut self.rows[id];
            write_block(out, &row.output);
            writeln!(out).ok();
            out.flush().ok();
            row.output = Vec::new();
        }
    }

    fn collapsed(&self, id: usize) -> bool {
        self.level == Level::Normal && !self.rows[id].children.is_empty() && self.passed(id)
    }

    /// Whether `id` and everything below it are done.
    fn passed(&self, id: usize) -> bool {
        self.rows[id].status == Status::Done
            && self.rows[id].children.iter().all(|&c| self.passed(c))
    }

    /// Rows that get a line, in tree order.
    fn visible(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.rows[ROOT].children.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            out.push(id);
            if !self.collapsed(id) {
                stack.extend(self.rows[id].children.iter().rev());
            }
        }
        out
    }

    /// Match bars to the visible rows.
    fn reconcile(&mut self) {
        let Mode::Tree { progress, .. } = &self.mode else {
            return;
        };
        let visible = self.visible();
        let mut shown = vec![false; self.rows.len()];
        for &id in &visible {
            shown[id] = true;
        }
        for (row, shown) in self.rows.iter_mut().zip(shown) {
            if !shown && let Some(bar) = row.bar.take() {
                progress.remove(&bar);
            }
        }
        for (index, &id) in visible.iter().enumerate() {
            if self.rows[id].bar.is_none() {
                // Styled before insertion so no frame shows a default bar, ticked so it takes a line.
                let (style, message) = self.line(id);
                let bar = ProgressBar::no_length()
                    .with_style(style)
                    .with_prefix("  ".repeat(self.rows[id].depth - 1))
                    .with_message(message);
                let bar = progress.insert(index, bar);
                bar.tick();
                self.rows[id].bar = Some(bar);
            }
        }
    }

    /// Redraw `id` and its ancestors, whose glyphs depend on it.
    fn draw(&self, id: usize) {
        for id in std::iter::once(id).chain(ancestors(&self.rows, id)) {
            if let Some(bar) = &self.rows[id].bar {
                let (style, message) = self.line(id);
                bar.set_style(style);
                bar.set_message(message);
            }
        }
    }

    /// A row's line and style; a failed leaf's title is red so the trail of carets ends in red.
    fn line(&self, id: usize) -> (ProgressStyle, String) {
        let row = &self.rows[id];
        let leaf = row.children.is_empty();
        if row.status == Status::Running && leaf {
            (
                running_style(),
                format!("{}{}", row.title, trailer(&self.rows, id)),
            )
        } else {
            let title = if row.status == Status::Failed && leaf {
                style(&row.title).red().for_stderr().to_string()
            } else {
                row.title.clone()
            };
            (
                static_style(),
                format!(
                    "{} {title}{}",
                    glyph(&self.rows, id),
                    trailer(&self.rows, id)
                ),
            )
        }
    }

    /// Advance the spinners.
    fn tick(&self) {
        for row in &self.rows {
            if row.status == Status::Running
                && let Some(bar) = &row.bar
            {
                bar.tick();
            }
        }
    }

    /// Cancel whatever never finished, freeze the tree and print the held output.
    fn finish(&mut self) {
        // Children before parents, so a group's line follows its children's in plain mode.
        for id in (ROOT + 1..self.rows.len()).rev() {
            if matches!(self.rows[id].status, Status::Pending | Status::Running) {
                self.set_status(id, Status::Cancelled);
            }
        }
        if !matches!(self.mode, Mode::Tree { .. }) {
            return;
        }
        for row in &self.rows {
            if let Some(bar) = &row.bar {
                bar.finish();
            }
        }
        let show: Vec<usize> = self
            .visible()
            .into_iter()
            .filter(|&id| {
                let row = &self.rows[id];
                !row.output.is_empty() && (row.status.notable() || self.level == Level::Verbose)
            })
            .collect();
        if show.is_empty() {
            return;
        }
        let sep = format!(" {} ", glyphs().arrow);
        let Mode::Tree { out, .. } = &mut self.mode else {
            return;
        };
        // The last drawn row has no newline of its own.
        writeln!(out).ok();
        for id in show {
            writeln!(
                out,
                "\n{} {}",
                glyph(&self.rows, id),
                path(&self.rows, id, &sep)
            )
            .ok();
            write_block(out, &self.rows[id].output);
        }
        out.flush().ok();
    }
}

/// Ancestors of `id`, nearest first, root excluded.
fn ancestors(rows: &[Row], id: usize) -> impl Iterator<Item = usize> + '_ {
    std::iter::successors(rows[id].parent, |&p| rows[p].parent).filter(|&p| p != ROOT)
}

/// Titles from below the top-level row down to `id`.
fn path(rows: &[Row], id: usize, sep: &str) -> String {
    // Ancestors at depth 1 are sections (the container, a phase), not part of the path.
    let mut titles: Vec<&str> = std::iter::once(id)
        .chain(ancestors(rows, id).take_while(|&a| rows[a].depth > 1))
        .map(|i| rows[i].title.as_str())
        .collect();
    titles.reverse();
    titles.join(sep)
}

/// The dim note after a title, with its leading space and dash.
fn trailer(rows: &[Row], id: usize) -> String {
    let note = &rows[id].note;
    if note.is_empty() {
        String::new()
    } else {
        let dashed = format_args!("{} {note}", glyphs().dash);
        format!(" {}", style(dashed).dim().for_stderr())
    }
}

/// The glyph for a row's status.
fn glyph(rows: &[Row], id: usize) -> StyledObject<&'static str> {
    let row = &rows[id];
    let g = glyphs();
    let group = !row.children.is_empty();
    match row.status {
        Status::Running if group => style(g.arrow).yellow(),
        Status::Failed | Status::Cancelled if group => style(g.arrow).red(),
        Status::Done => style(g.tick).green(),
        Status::Warn => style(g.warning).yellow(),
        Status::Failed => style(g.cross).red(),
        Status::Cancelled => style(g.ban).dim(),
        Status::Pending | Status::Running => style(g.square).dim(),
    }
    .for_stderr()
}

/// Write output, ending it with a newline.
fn write_block(out: &mut dyn Write, bytes: &[u8]) {
    out.write_all(bytes).ok();
    if !bytes.ends_with(b"\n") {
        out.write_all(b"\n").ok();
    }
}

fn static_style() -> ProgressStyle {
    static STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
        ProgressStyle::with_template("{prefix}{msg}")
            .expect("valid template")
            // The template has no spinner, so drop the 29 tick strings every style is born with.
            .tick_strings(&["", ""])
    });
    STYLE.clone()
}

fn running_style() -> ProgressStyle {
    static STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
        ProgressStyle::with_template("{prefix}{spinner:.yellow} {msg}")
            .expect("valid template")
            .tick_strings(glyphs().spinner)
    });
    STYLE.clone()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use indicatif::{InMemoryTerm, ProgressDrawTarget};

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn plain(level: Level) -> (Reporter, Buf) {
        console::set_colors_enabled_stderr(false);
        let buf = Buf::default();
        let root = Reporter::custom(
            level,
            ProgressDrawTarget::hidden(),
            Box::new(buf.clone()),
            None,
        );
        (root, buf)
    }

    /// `term_like` sets no refresh rate, so every draw lands before the next statement runs.
    fn tree(level: Level) -> (Reporter, InMemoryTerm, Buf) {
        console::set_colors_enabled_stderr(false);
        let term = InMemoryTerm::new(20, 60);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let buf = Buf::default();
        let root = Reporter::custom(level, target, Box::new(buf.clone()), None);
        (root, term, buf)
    }

    #[test]
    fn plain_logs_transitions_with_full_paths() {
        let (root, buf) = plain(Level::Normal);
        let tasks = root.add("Running tasks");
        let config = tasks.add("cfg.json");
        let glob = config.add("*.md").note("2 files");
        let a = glob.add("a");
        let b = glob.add("b");
        assert_eq!(buf.text(), "", "pending prints nothing");
        tasks.status(Status::Running);
        config.status(Status::Running);
        glob.status(Status::Running);
        a.status(Status::Running);
        a.output("hello\n").status(Status::Done);
        b.status(Status::Running).status(Status::Failed);
        glob.status(Status::Failed);
        config.status(Status::Failed);
        tasks.status(Status::Failed);
        assert_eq!(
            buf.text(),
            "[STARTED] Running tasks\n\
             [STARTED] cfg.json\n\
             [STARTED] cfg.json > *.md — 2 files\n\
             [STARTED] cfg.json > *.md > a\n\
             [COMPLETED] cfg.json > *.md > a\n\
             [STARTED] cfg.json > *.md > b\n\
             [FAILED] cfg.json > *.md > b\n\
             [FAILED] cfg.json > *.md — 2 files\n\
             [FAILED] cfg.json\n\
             [FAILED] Running tasks\n"
        );
    }

    #[test]
    fn plain_prints_failed_output_under_its_line_and_late_output_under_a_header() {
        let (root, buf) = plain(Level::Normal);
        let a = root.add("a").status(Status::Running);
        a.output("boom\n").status(Status::Failed);
        let b = root.add("b").status(Status::Running).status(Status::Failed);
        b.output("late").output("more\n");
        assert_eq!(
            buf.text(),
            "[STARTED] a\n[FAILED] a\nboom\n\n[STARTED] b\n[FAILED] b\n\
             [FAILED] b\nlate\n\n[FAILED] b\nmore\n\n"
        );
    }

    #[test]
    fn warnings_print_their_output_without_failing_their_phase() {
        let (root, buf) = plain(Level::Normal);
        let phase = root.add("Restoring").status(Status::Running);
        phase
            .add("Could not apply x")
            .output("a.md: conflicts\n")
            .status(Status::Warn);
        phase.status(Status::Done);
        drop((root, phase));
        assert_eq!(
            buf.text(),
            "[STARTED] Restoring\n[WARN] Could not apply x\na.md: conflicts\n\n[COMPLETED] Restoring\n"
        );
    }

    #[test]
    fn verbose_prints_every_output() {
        let (root, buf) = plain(Level::Verbose);
        let a = root.add("a").status(Status::Running);
        a.output("ok\n").status(Status::Done);
        assert_eq!(buf.text(), "[STARTED] a\n[COMPLETED] a\nok\n\n");
    }

    #[test]
    fn quiet_prints_only_failed_output() {
        let (root, buf) = plain(Level::Quiet);
        root.add("note").status(Status::Warn).output("detail\n");
        let a = root.add("a").status(Status::Running);
        a.output("fine\n").status(Status::Done);
        let group = root.add("group").status(Status::Running);
        let b = group.add("b").status(Status::Running);
        b.output("boom\n").status(Status::Failed);
        group.status(Status::Failed);
        root.add("Error").output("bad\n").status(Status::Failed);
        assert_eq!(buf.text(), "[FAILED] b\nboom\n\n[FAILED] Error\nbad\n\n");
    }

    /// On a terminal, quiet mode heads output with the glyph and path the tree would show.
    #[test]
    fn quiet_on_a_terminal_uses_glyphs() {
        console::set_colors_enabled_stderr(false);
        let buf = Buf::default();
        let term = InMemoryTerm::new(20, 60);
        let target = ProgressDrawTarget::term_like(Box::new(term.clone()));
        let root = Reporter::custom(Level::Quiet, target, Box::new(buf.clone()), None);
        let cmd = root
            .add("cfg")
            .add("*.md")
            .add("lint")
            .status(Status::Running);
        cmd.output("boom\n").status(Status::Failed);
        assert_eq!(buf.text(), "✖ *.md ❯ lint\nboom\n\n");
        assert_eq!(term.contents(), "", "quiet draws no tree");
    }

    #[test]
    fn dropping_the_last_handle_cancels_what_never_finished() {
        let (root, buf) = plain(Level::Normal);
        let tasks = root.add("Running tasks").status(Status::Running);
        let glob = tasks.add("*.md").status(Status::Running);
        let a = glob.add("a").status(Status::Running);
        let b = glob.add("b");
        drop((root, tasks, glob, a, b));
        assert_eq!(
            buf.text(),
            "[STARTED] Running tasks\n[STARTED] *.md\n[STARTED] *.md > a\n\
             [CANCELLED] *.md > b\n[CANCELLED] *.md > a\n[CANCELLED] *.md\n\
             [CANCELLED] Running tasks\n"
        );
    }

    #[test]
    fn tree_glyphs_follow_each_rows_own_status() {
        let (root, term, _) = tree(Level::Normal);
        let tasks = root.add("Running tasks");
        let glob = tasks.add("*.md").note("1 file");
        let a = glob.add("a");
        let b = glob.add("b");
        tasks.status(Status::Running);
        assert_eq!(
            term.contents(),
            "❯ Running tasks\n  ◼ *.md — 1 file\n    ◼ a\n    ◼ b",
            "a running group shows a caret, pending rows squares"
        );
        glob.status(Status::Running);
        a.status(Status::Running);
        assert_eq!(
            term.contents(),
            "❯ Running tasks\n  ❯ *.md — 1 file\n    ⠙ a\n    ◼ b",
            "a running leaf spins"
        );
        a.status(Status::Done);
        b.status(Status::Running).status(Status::Failed);
        glob.status(Status::Failed);
        tasks.status(Status::Failed);
        assert_eq!(
            term.contents(),
            "❯ Running tasks\n  ❯ *.md — 1 file\n    ✔ a\n    ✖ b",
            "failed groups show red carets, failed leaves a cross"
        );
        drop((root, tasks, glob, a, b));
    }

    #[test]
    fn tree_collapses_a_passed_group_at_normal_but_not_at_verbose() {
        for (level, expected) in [
            (Level::Normal, "✔ *.md"),
            (Level::Verbose, "✔ *.md\n  ✔ a\n  ✔ b"),
        ] {
            let (root, term, _) = tree(level);
            let glob = root.add("*.md").status(Status::Running);
            let a = glob.add("a").status(Status::Running);
            let b = glob.add("b").status(Status::Running);
            a.status(Status::Done);
            b.status(Status::Done);
            glob.status(Status::Done);
            assert_eq!(term.contents(), expected);
            drop((root, glob, a, b));
        }
    }

    #[test]
    fn a_warning_anywhere_below_a_passed_group_stops_it_collapsing() {
        let (root, term, _) = tree(Level::Normal);
        let config = root.add("cfg").status(Status::Running);
        let glob = config.add("*.md").status(Status::Running);
        let cmd = glob.add("lint").status(Status::Warn);
        glob.status(Status::Done);
        config.status(Status::Done);
        assert_eq!(term.contents(), "✔ cfg\n  ✔ *.md\n    ⚠ lint");
        drop((root, config, glob, cmd));
    }

    #[test]
    fn the_finished_tree_is_followed_by_the_output_it_held() {
        let (root, _term, buf) = tree(Level::Normal);
        let glob = root.add("cfg").add("*.md").status(Status::Running);
        let a = glob.add("a").status(Status::Running);
        a.output("boom\n").status(Status::Failed);
        glob.status(Status::Failed);
        assert_eq!(buf.text(), "", "nothing until the tree is done");
        drop((root, glob, a));
        assert_eq!(buf.text(), "\n\n✖ *.md ❯ a\nboom\n");
    }
}
