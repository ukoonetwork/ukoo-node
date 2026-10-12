//! Runs `ukoo-node start` in the background: a launchd agent on macOS, a systemd user service on
//! Linux, a logon task on Windows.

use std::path::PathBuf;
use std::process::Command;

use crate::error::NodeError;
use crate::paths;

#[cfg(target_os = "macos")]
const LABEL: &str = "network.ukoo.node";

pub enum State {
    NotInstalled,
    Stopped,
    Running,
}

pub fn log_file() -> PathBuf {
    paths::data_dir().join("logs").join("node.log")
}

fn run(program: &str, args: &[&str]) -> Result<bool, NodeError> {
    Ok(Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?
        .success())
}

fn exe() -> Result<String, NodeError> {
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn unit_file() -> PathBuf {
    directories::BaseDirs::new()
        .expect("no home directory for this user")
        .home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn domain() -> String {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    format!("gui/{uid}")
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<(), NodeError> {
    let log = log_file();
    std::fs::create_dir_all(log.parent().expect("log has a parent"))?;
    let home = paths::data_dir();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string><string>start</string></array>
  <key>EnvironmentVariables</key><dict><key>UKOO_NODE_HOME</key><string>{}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        exe()?,
        home.display(),
        log.display(),
        log.display()
    );
    let file = unit_file();
    std::fs::create_dir_all(file.parent().expect("plist has a parent"))?;
    std::fs::write(&file, plist)?;
    let _ = run(
        "launchctl",
        &["bootout", &domain(), &file.to_string_lossy()],
    );
    if !run(
        "launchctl",
        &["bootstrap", &domain(), &file.to_string_lossy()],
    )? {
        return Err(NodeError::ServiceFailed(
            "launchctl couldn't load it".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<(), NodeError> {
    let file = unit_file();
    let _ = run(
        "launchctl",
        &["bootout", &domain(), &file.to_string_lossy()],
    );
    let _ = std::fs::remove_file(file);
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn state() -> Result<State, NodeError> {
    if !unit_file().exists() {
        return Ok(State::NotInstalled);
    }
    let out = Command::new("launchctl")
        .args(["print", &format!("{}/{LABEL}", domain())])
        .output()?;
    let running = String::from_utf8_lossy(&out.stdout).contains("state = running");
    Ok(if running {
        State::Running
    } else {
        State::Stopped
    })
}

#[cfg(target_os = "linux")]
fn unit_file() -> PathBuf {
    directories::BaseDirs::new()
        .expect("no home directory for this user")
        .config_dir()
        .join("systemd/user/ukoo-node.service")
}

#[cfg(target_os = "linux")]
pub fn install() -> Result<(), NodeError> {
    let unit = format!(
        "[Unit]\nDescription=Ukoo node\nAfter=network-online.target\n\n[Service]\nExecStart={} start\nEnvironment=UKOO_NODE_HOME={}\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        exe()?,
        paths::data_dir().display()
    );
    let file = unit_file();
    std::fs::create_dir_all(file.parent().expect("unit has a parent"))?;
    std::fs::write(&file, unit)?;
    if !run("systemctl", &["--user", "daemon-reload"])?
        || !run("systemctl", &["--user", "enable", "--now", "ukoo-node"])?
    {
        return Err(NodeError::ServiceFailed(
            "systemctl --user couldn't start it".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> Result<(), NodeError> {
    let _ = run("systemctl", &["--user", "disable", "--now", "ukoo-node"]);
    let _ = std::fs::remove_file(unit_file());
    let _ = run("systemctl", &["--user", "daemon-reload"]);
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn state() -> Result<State, NodeError> {
    if !unit_file().exists() {
        return Ok(State::NotInstalled);
    }
    Ok(
        if run(
            "systemctl",
            &["--user", "is-active", "--quiet", "ukoo-node"],
        )? {
            State::Running
        } else {
            State::Stopped
        },
    )
}

#[cfg(windows)]
const TASK: &str = "UkooNode";

#[cfg(windows)]
pub fn install() -> Result<(), NodeError> {
    let log = log_file();
    std::fs::create_dir_all(log.parent().expect("log has a parent"))?;
    // The task runs through cmd so its output lands in the log file.
    let action = format!(
        "cmd /c set UKOO_NODE_HOME={} && \"{}\" start >> \"{}\" 2>&1",
        paths::data_dir().display(),
        exe()?,
        log.display()
    );
    if !run(
        "schtasks",
        &[
            "/Create", "/TN", TASK, "/TR", &action, "/SC", "ONLOGON", "/RL", "LIMITED", "/F",
        ],
    )? {
        return Err(NodeError::ServiceFailed(
            "schtasks couldn't create the task".into(),
        ));
    }
    let _ = run("schtasks", &["/Run", "/TN", TASK]);
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> Result<(), NodeError> {
    let _ = run("schtasks", &["/End", "/TN", TASK]);
    let _ = run("schtasks", &["/Delete", "/TN", TASK, "/F"]);
    Ok(())
}

#[cfg(windows)]
pub fn state() -> Result<State, NodeError> {
    let out = Command::new("schtasks")
        .args(["/Query", "/TN", TASK, "/FO", "LIST"])
        .output()?;
    if !out.status.success() {
        return Ok(State::NotInstalled);
    }
    Ok(
        if String::from_utf8_lossy(&out.stdout).contains("Running") {
            State::Running
        } else {
            State::Stopped
        },
    )
}
