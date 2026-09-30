#[cfg(windows)]
mod relay_windows;

#[cfg(windows)]
fn main() {
    relay_windows::run();
}

#[cfg(not(windows))]
fn main() {
    // Linux talks to Codex directly; the Claude named-pipe relay is Windows-only.
    eprintln!("coucou-hook is the Windows Claude Code relay; Linux uses Codex in Coucou.");
    std::process::exit(1);
}
