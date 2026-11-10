use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::favorites::write_atomically;
use crate::terminal::durable::{host_log_file, login_path};
use crate::terminal::runtime_dir::effective_uid;

pub const LABEL: &str = "dev.ccview-rs.host";
const LAUNCH_AGENTS_DIR_ENV: &str = "CCVIEW_LAUNCH_AGENTS_DIR";
pub const MACOS_ONLY: &str = "autostart is macOS only";

pub trait Launchctl {
    fn run(&self, args: &[String]) -> io::Result<bool>;
}

pub struct SystemLaunchctl;

impl Launchctl for SystemLaunchctl {
    fn run(&self, args: &[String]) -> io::Result<bool> {
        Command::new("launchctl")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub installed: bool,
    pub loaded: bool,
}

pub struct Agent {
    pub dir: PathBuf,
    pub executable: PathBuf,
    pub log: PathBuf,
    pub path: String,
    pub uid: u32,
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Agent {
    pub fn from_env() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("HOME is not set"))?;
        let dir = std::env::var_os(LAUNCH_AGENTS_DIR_ENV)
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join("Library/LaunchAgents"), PathBuf::from);
        Ok(Self {
            dir,
            executable: std::env::current_exe()?.canonicalize()?,
            log: host_log_file(),
            path: login_path(&home, None),
            uid: effective_uid(),
        })
    }

    pub fn plist_path(&self) -> PathBuf {
        self.dir.join(format!("{LABEL}.plist"))
    }

    pub fn render(&self) -> String {
        let text = |value: &Path| xml_escape(&value.display().to_string());
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n  <string>{LABEL}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>host</string>\n    <string>--restore</string>\n  </array>\n\
  <key>RunAtLoad</key>\n  <true/>\n\
  <key>StandardOutPath</key>\n  <string>{}</string>\n\
  <key>StandardErrorPath</key>\n  <string>{}</string>\n\
  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{}</string>\n  </dict>\n\
</dict>\n\
</plist>\n",
            text(&self.executable),
            text(&self.log),
            text(&self.log),
            xml_escape(&self.path),
        )
    }

    fn target(&self) -> String {
        format!("gui/{}/{LABEL}", self.uid)
    }

    fn loaded(&self, launchctl: &dyn Launchctl) -> bool {
        launchctl.run(&["print".to_owned(), self.target()]).unwrap_or(false)
    }

    pub fn installed(&self) -> bool {
        self.plist_path().is_file()
    }

    pub fn status(&self, launchctl: &dyn Launchctl) -> Status {
        Status { installed: self.installed(), loaded: self.loaded(launchctl) }
    }

    pub fn on(&self, launchctl: &dyn Launchctl) -> io::Result<()> {
        if let Some(log_dir) = self.log.parent() {
            fs::create_dir_all(log_dir)?;
        }
        write_atomically(&self.plist_path(), self.render().as_bytes())?;
        if self.loaded(launchctl) {
            launchctl.run(&["bootout".to_owned(), self.target()])?;
        }
        let args = ["bootstrap".to_owned(), format!("gui/{}", self.uid), self.plist_path().display().to_string()];
        if launchctl.run(&args)? {
            Ok(())
        } else {
            Err(io::Error::other("launchctl bootstrap failed"))
        }
    }

    pub fn off(&self, launchctl: &dyn Launchctl) -> io::Result<()> {
        if self.loaded(launchctl) && !launchctl.run(&["bootout".to_owned(), self.target()])? {
            return Err(io::Error::other("launchctl bootout failed"));
        }
        match fs::remove_file(self.plist_path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

pub fn is_enabled() -> bool {
    Agent::from_env().is_ok_and(|agent| agent.installed())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(MACOS_ONLY.to_owned());
    }
    let agent = Agent::from_env().map_err(|error| error.to_string())?;
    let result = if enabled { agent.on(&SystemLaunchctl) } else { agent.off(&SystemLaunchctl) };
    result.map_err(|error| error.to_string())
}

pub fn run_command(args: &[String], launchctl: &dyn Launchctl) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err(MACOS_ONLY.to_owned());
    }
    let agent = Agent::from_env().map_err(|error| error.to_string())?;
    match args.first().map(String::as_str) {
        Some("on") => agent.on(launchctl).map(|()| "autostart on".to_owned()),
        Some("off") => agent.off(launchctl).map(|()| "autostart off".to_owned()),
        Some("status") => {
            let status = agent.status(launchctl);
            Ok(format!("plist: {}\nloaded: {}", status.installed, status.loaded))
        }
        _ => return Err("usage: ccview-rs autostart on|off|status".to_owned()),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Recording {
        calls: RefCell<Vec<Vec<String>>>,
        loaded: RefCell<bool>,
    }

    impl Recording {
        fn new() -> Self {
            Self { calls: RefCell::new(Vec::new()), loaded: RefCell::new(false) }
        }
    }

    impl Launchctl for Recording {
        fn run(&self, args: &[String]) -> io::Result<bool> {
            self.calls.borrow_mut().push(args.to_vec());
            match args[0].as_str() {
                "print" => Ok(*self.loaded.borrow()),
                "bootstrap" => {
                    *self.loaded.borrow_mut() = true;
                    Ok(true)
                }
                "bootout" => {
                    *self.loaded.borrow_mut() = false;
                    Ok(true)
                }
                _ => Ok(false),
            }
        }
    }

    fn agent(name: &str) -> Agent {
        let root = std::env::temp_dir().join(format!("ccview-autostart-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Agent {
            dir: root.join("LaunchAgents"),
            executable: PathBuf::from("/opt/ccview/bin/ccview-rs"),
            log: root.join("state/host.log"),
            path: "/opt/homebrew/bin:/usr/bin".to_owned(),
            uid: 501,
        }
    }

    fn cleanup(agent: &Agent) {
        let _ = fs::remove_dir_all(agent.dir.parent().unwrap());
    }

    #[test]
    fn on_writes_a_run_at_load_plist_that_restores_once_and_never_keeps_alive_then_loads_it() {
        let agent = agent("on");
        let launchctl = Recording::new();
        agent.on(&launchctl).unwrap();
        let plist = fs::read_to_string(agent.plist_path()).unwrap();
        assert!(plist.contains("<key>Label</key>\n  <string>dev.ccview-rs.host</string>"));
        assert!(plist.contains(
            "<array>\n    <string>/opt/ccview/bin/ccview-rs</string>\n    <string>host</string>\n    <string>--restore</string>\n  </array>"
        ));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(!plist.contains("KeepAlive"));
        assert!(plist.contains(&format!("<key>StandardOutPath</key>\n  <string>{}</string>", agent.log.display())));
        assert!(plist.contains("<key>PATH</key>\n    <string>/opt/homebrew/bin:/usr/bin</string>"));
        let calls = launchctl.calls.borrow();
        assert_eq!(
            calls.last().unwrap(),
            &vec!["bootstrap".to_owned(), "gui/501".to_owned(), agent.plist_path().display().to_string()]
        );
        cleanup(&agent);
    }

    #[test]
    fn off_boots_it_out_and_removes_the_plist_and_status_tracks_both() {
        let agent = agent("off");
        let launchctl = Recording::new();
        assert_eq!(agent.status(&launchctl), Status { installed: false, loaded: false });
        agent.on(&launchctl).unwrap();
        assert_eq!(agent.status(&launchctl), Status { installed: true, loaded: true });
        agent.off(&launchctl).unwrap();
        assert_eq!(agent.status(&launchctl), Status { installed: false, loaded: false });
        assert!(launchctl.calls.borrow().contains(&vec!["bootout".to_owned(), "gui/501/dev.ccview-rs.host".to_owned()]));
        cleanup(&agent);
    }

    #[test]
    fn turning_it_on_twice_boots_the_old_job_out_before_bootstrapping_again() {
        let agent = agent("twice");
        let launchctl = Recording::new();
        agent.on(&launchctl).unwrap();
        agent.on(&launchctl).unwrap();
        let verbs: Vec<String> = launchctl.calls.borrow().iter().map(|call| call[0].clone()).collect();
        assert_eq!(verbs, ["print", "bootstrap", "print", "bootout", "bootstrap"]);
        cleanup(&agent);
    }
}
