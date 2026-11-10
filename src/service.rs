//! Running the service from login. On macOS that is a launchd agent of the user; Linux and Windows
//! follow with their own builds.
//!
//! The agent restarts the service if it crashes, not if it is stopped on purpose (Settings > Stop the
//! service ends it until the next login or until the app is opened).

use crate::paths::{self, Paths};
use std::path::{Path, PathBuf};

pub const LABEL: &str = "app.kr8.eightgent.service";

pub fn plist_path() -> PathBuf {
    paths::user_home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

/// The service is set to start at login.
pub fn installed() -> bool {
    cfg!(target_os = "macos") && plist_path().is_file()
}

/// This process was started by the login agent.
pub fn started_at_login() -> bool {
    std::env::var_os("EIGHTGENT_LAUNCHD").is_some()
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The agent's definition. `ProcessType Interactive` matters: launchd throttles the disk reads of a
/// background job, and an engine started by one took minutes to load a model.
pub fn plist(program: &Path, paths: &Paths) -> String {
    let log = paths.logs().join("service.log");
    let mut env = String::from("    <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>\n    <key>EIGHTGENT_LAUNCHD</key><string>1</string>\n");
    // a service set up for another data folder keeps using it
    if let Some(home) = std::env::var_os("EIGHTGENT_HOME").filter(|v| !v.is_empty()) {
        env.push_str(&format!("    <key>EIGHTGENT_HOME</key><string>{}</string>\n", xml(&home.to_string_lossy())));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{program}</string><string>serve</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>AssociatedBundleIdentifiers</key><string>app.kr8.eightgent</string>
  <key>EnvironmentVariables</key>
  <dict>
{env}  </dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        program = xml(&program.to_string_lossy()),
        log = xml(&log.to_string_lossy()),
    )
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("/bin/launchctl").args(args).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text.trim().to_string()) }
}

#[cfg(target_os = "macos")]
fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Write the agent's definition, so the service starts at the next login. Nothing is started now.
pub fn write(program: &Path, paths: &Paths) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Starting at login is not set up for this system yet.".into());
    }
    paths.ensure().map_err(|e| e.to_string())?;
    let file = plist_path();
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&file, plist(program, paths)).map_err(|e| format!("{}: {e}", file.display()))
}

/// Hand the agent to launchd now: it starts the service (which must not be running already).
pub fn load() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let job = format!("{}/{LABEL}", domain());
        // a job from an earlier install has to be gone before the new one is taken; launchd lets go
        // of it a moment after being told to, and refuses the new one until then
        if launchctl(&["print", &job]).is_ok() {
            let _ = launchctl(&["bootout", &job]);
            for _ in 0..40 {
                if launchctl(&["print", &job]).is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        let mut last = String::new();
        for _ in 0..8 {
            match launchctl(&["bootstrap", &domain(), &plist_path().to_string_lossy()]) {
                Ok(_) => return Ok(()),
                Err(e) => last = e,
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Err(format!("macOS did not take the login item: {last}"))
    }
    #[cfg(not(target_os = "macos"))]
    Err("Starting at login is not set up for this system yet.".into())
}

/// Start the agent's service again after it was stopped. False when there is no agent.
pub fn kickstart() -> bool {
    #[cfg(target_os = "macos")]
    {
        if !installed() {
            return false;
        }
        if launchctl(&["kickstart", &format!("{}/{LABEL}", domain())]).is_ok() {
            return true;
        }
        // not loaded in this login session yet (it was installed by hand, or booted out)
        load().is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Stop starting at login. With `stop`, the running service is ended too.
pub fn remove(stop: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = std::fs::remove_file(plist_path());
        if stop {
            let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain())]);
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = stop;
        Err("Starting at login is not set up for this system yet.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_restarts_a_crash_but_not_a_stop_and_reads_disks_at_full_speed() {
        let text = plist(Path::new("/Applications/8gent.app/Contents/Resources/bin/8gent"), &Paths::at("/tmp/8gent & co"));
        assert!(text.contains("<string>/Applications/8gent.app/Contents/Resources/bin/8gent</string><string>serve</string>"));
        assert!(text.contains("<key>SuccessfulExit</key><false/>"));
        assert!(text.contains("<key>ProcessType</key><string>Interactive</string>"));
        // (the separator differs by system; the agent itself is macOS only)
        assert!(text.contains("8gent &amp; co") && text.contains("service.log") && !text.contains("8gent & co"), "paths are escaped for XML");
        assert!(text.contains("/opt/homebrew/bin"), "engines installed with Homebrew are found");
    }
}
