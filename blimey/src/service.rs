//! The launchd service that keeps `blimey run` going on macOS (plan §9): started at login,
//! restarted within a second whenever it exits.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL: &str = "app.blimey.engine";
/// launchd's default is 10 s between restarts; the target is under 1 s.
const THROTTLE_SECONDS: u32 = 1;

fn plist_path() -> io::Result<PathBuf> {
    let home = std::env::home_dir().ok_or_else(|| io::Error::other("HOME is not set"))?;
    Ok(home.join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

/// A launch agent running `<exe> run <run_args>` from `working_dir`, with stdout and stderr
/// appended to `log`.
pub fn plist(exe: &Path, run_args: &[String], working_dir: &Path, log: &Path) -> String {
    let string = |text: &str| format!("<string>{}</string>", escape(text));
    let arguments: Vec<String> = [exe.to_string_lossy().as_ref(), "run"]
        .into_iter()
        .chain(run_args.iter().map(String::as_str))
        .map(|argument| format!("    {}", string(argument)))
        .collect();
    let log = string(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  {label}
  <key>ProgramArguments</key>
  <array>
{arguments}
  </array>
  <key>WorkingDirectory</key>
  {working_dir}
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>{THROTTLE_SECONDS}</integer>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  {log}
  <key>StandardErrorPath</key>
  {log}
</dict>
</plist>
"#,
        label = string(LABEL),
        arguments = arguments.join("\n"),
        working_dir = string(&working_dir.to_string_lossy()),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Writes the launch agent and (re)starts it. Running it again replaces the previous one.
pub fn install(plist: &str) -> io::Result<()> {
    let path = plist_path()?;
    fs::create_dir_all(path.parent().expect("the plist is in LaunchAgents"))?;
    // Not loaded yet is fine: this only stops a previous install.
    let _ = launchctl(&["unload", &path.to_string_lossy()]);
    fs::write(&path, plist).map_err(|error| in_file(&path, error))?;
    launchctl(&["load", "-w", &path.to_string_lossy()])?;
    println!("installed and started {LABEL}: {}", path.display());
    Ok(())
}

/// Stops the launch agent and removes it.
pub fn uninstall() -> io::Result<()> {
    let path = plist_path()?;
    if !path.exists() {
        println!("{LABEL} is not installed: no {}", path.display());
        return Ok(());
    }
    // An agent that isn't loaded (e.g. after a failed install) fails to unload; the plist must
    // go regardless, or launchd loads it again at the next login.
    if let Err(error) = launchctl(&["unload", "-w", &path.to_string_lossy()]) {
        eprintln!("service: {error}; removing the agent anyway");
    }
    fs::remove_file(&path).map_err(|error| in_file(&path, error))?;
    println!("stopped and removed {LABEL}");
    Ok(())
}

fn launchctl(arguments: &[&str]) -> io::Result<()> {
    let output = Command::new("launchctl").args(arguments).output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    // launchctl's legacy subcommands report some failures on stderr with exit status 0.
    if output.status.success() && stderr.trim().is_empty() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "launchctl {}: {}",
        arguments.join(" "),
        stderr.trim()
    )))
}

fn in_file(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_runs_the_engine_from_the_working_directory_and_restarts_it_within_a_second() {
        let plist = plist(
            Path::new("/opt/blimey"),
            &[
                "tests/fixtures/shows/demo".into(),
                "--dmx-port".into(),
                "/dev/cu.a&b".into(),
            ],
            Path::new("/Users/me/blimey"),
            Path::new("/Users/me/Library/Application Support/Blimey/engine.log"),
        );

        assert!(plist.contains(
            "    <string>/opt/blimey</string>\n    <string>run</string>\n    <string>tests/fixtures/shows/demo</string>\n    <string>--dmx-port</string>\n    <string>/dev/cu.a&amp;b</string>\n"
        ), "{plist}");
        assert!(plist.contains("<key>WorkingDirectory</key>\n  <string>/Users/me/blimey</string>"));
        assert!(plist.contains("<key>KeepAlive</key>\n  <true/>"));
        assert!(plist.contains("<key>ThrottleInterval</key>\n  <integer>1</integer>"));
    }
}
