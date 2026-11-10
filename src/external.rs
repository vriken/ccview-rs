use crate::{terminal::CommandSpec, ui_settings::TerminalApp};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const TERMINAL_APP_ENV: &str = "CCVIEW_TERMINAL_APP";
const BACKGROUND_ENV: &str = "CCVIEW_OPEN_BACKGROUND";
const TERM_PROGRAM_ENV: &str = "TERM_PROGRAM";
const DEFAULT_APP: &str = "Terminal";
const SCRIPT_PREFIX: &str = "open-";
const SCRIPT_SUFFIX: &str = ".command";
static SCRIPT_COUNTER: AtomicU64 = AtomicU64::new(0);
const STALE_SCRIPT_AGE: Duration = Duration::from_secs(60);

pub fn open(spec: &CommandSpec, preference: TerminalApp) -> Result<String, String> {
    open_with(spec, &terminal_app(preference), &open_in_terminal_app)
}

pub fn open_with(
    spec: &CommandSpec,
    app: &str,
    opener: &dyn Fn(&str, &Path) -> Result<(), String>,
) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err("opening an external terminal is macOS-only for now".to_owned());
    }
    let script = write_script(&script_dir()?, spec).map_err(|err| format!("could not write launcher: {err}"))?;
    match opener(app, &script) {
        Ok(()) => Ok(format!("opened in {app}")),
        Err(err) => {
            let _ = fs::remove_file(&script);
            Err(err)
        }
    }
}

fn open_in_terminal_app(app: &str, script: &Path) -> Result<(), String> {
    let mut open = Command::new("open");
    if std::env::var_os(BACKGROUND_ENV).is_some_and(|value| !value.is_empty()) {
        open.arg("-g");
    }
    let status = open
        .arg("-a")
        .arg(app)
        .arg(script)
        .status()
        .map_err(|err| format!("could not run open: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open -a {app} failed ({status})"))
    }
}

pub fn remove_stale_scripts() {
    let Ok(dir) = script_dir() else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let age = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        if age.is_some_and(|age| age > STALE_SCRIPT_AGE) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn app_override() -> Option<String> {
    std::env::var(TERMINAL_APP_ENV).ok().filter(|app| !app.is_empty())
}

fn terminal_app(preference: TerminalApp) -> String {
    resolve_app(
        preference,
        app_override().as_deref(),
        std::env::var(TERM_PROGRAM_ENV).ok().as_deref(),
    )
}

fn resolve_app(preference: TerminalApp, override_app: Option<&str>, term_program: Option<&str>) -> String {
    if let Some(app) = override_app {
        return app.to_owned();
    }
    if let Some(app) = preference.app_name() {
        return app.to_owned();
    }
    match term_program {
        Some("iTerm.app") => "iTerm",
        _ => DEFAULT_APP,
    }
    .to_owned()
}

fn script_dir() -> Result<PathBuf, String> {
    let owner = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("ccview-{owner}"));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    Ok(dir)
}

fn write_script(dir: &Path, spec: &CommandSpec) -> std::io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{SCRIPT_PREFIX}{}-{nanos}-{sequence}{SCRIPT_SUFFIX}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)?;
    file.write_all(render_script(spec).as_bytes())?;
    Ok(path)
}

pub fn render_script(spec: &CommandSpec) -> String {
    let mut script = String::from("#!/bin/sh\nrm -f -- \"$0\"\n");
    if let Some(cwd) = &spec.cwd {
        script.push_str(&format!("cd -- {} || exit 1\n", shell_quote(&cwd.to_string_lossy())));
    }
    for key in spec.env_remove.iter().filter(|key| is_env_name(key)) {
        script.push_str(&format!("unset {key}\n"));
    }
    for (key, value) in &spec.env {
        script.push_str(&format!("export {}={}\n", key, shell_quote(value)));
    }
    script.push_str("exec ");
    script.push_str(&shell_quote(&spec.program));
    for arg in &spec.args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script.push('\n');
    script
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, process::Stdio};

    const HOSTILE: [&str; 6] = [
        "plain",
        "with space",
        "it's \"quoted\"",
        "$(touch pwned) `touch pwned` ${HOME}",
        "line one\nline two",
        "; rm -rf / # \\ *",
    ];

    fn run_script(name: &str, spec: &CommandSpec) -> String {
        let dir = std::env::temp_dir().join(format!("ccview-ext-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let script = write_script(&dir, spec).unwrap();
        let output = Command::new("sh")
            .arg(&script)
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .output()
            .unwrap();
        assert!(!script.exists(), "script removes itself");
        assert!(!dir.join("pwned").exists());
        fs::remove_dir_all(&dir).unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn opener_receives_an_owner_only_script_that_runs_the_command() {
        let spec = CommandSpec::new("sh").args(["-c", "printf ran"]);
        let seen = std::cell::RefCell::new(None);
        let message = open_with(&spec, "Ghostty", &|app, script| {
            let mode = fs::metadata(script).unwrap().permissions().mode() & 0o777;
            let output = Command::new("sh").arg(script).output().unwrap();
            *seen.borrow_mut() = Some((app.to_owned(), mode, String::from_utf8(output.stdout).unwrap()));
            Ok(())
        });
        let (app, mode, output) = seen.into_inner().unwrap();
        assert_eq!((mode, output.as_str(), app.as_str()), (0o700, "ran", "Ghostty"));
        assert_eq!(message, Ok("opened in Ghostty".to_owned()));
    }

    #[test]
    fn the_environment_variable_beats_the_setting_which_beats_autodetection() {
        let resolve = resolve_app;
        assert_eq!(resolve(TerminalApp::Ghostty, Some("kitty"), Some("iTerm.app")), "kitty");
        assert_eq!(resolve(TerminalApp::WezTerm, None, Some("iTerm.app")), "WezTerm");
        assert_eq!(resolve(TerminalApp::Auto, None, Some("iTerm.app")), "iTerm");
        assert_eq!(resolve(TerminalApp::Auto, None, None), "Terminal");
    }

    #[test]
    fn failed_open_reports_the_error_and_removes_the_script() {
        let spec = CommandSpec::new("true");
        let path = std::cell::RefCell::new(PathBuf::new());
        let result = open_with(&spec, "Terminal", &|_, script| {
            *path.borrow_mut() = script.to_owned();
            Err("boom".to_owned())
        });
        assert_eq!(result, Err("boom".to_owned()));
        assert!(!path.borrow().exists());
    }

    #[test]
    fn hostile_arguments_reach_the_program_verbatim() {
        let mut args = vec!["-c".to_owned(), r#"for a in "$@"; do printf '[%s]' "$a"; done"#.to_owned(), "sh".to_owned()];
        args.extend(HOSTILE.iter().map(|arg| (*arg).to_owned()));
        let spec = CommandSpec::new("sh").args(args);
        let expected: String = HOSTILE.iter().map(|arg| format!("[{arg}]")).collect();
        assert_eq!(run_script("args", &spec), expected);
    }

    #[test]
    fn concurrent_launchers_never_collide_on_a_script_name() {
        let dir = std::env::temp_dir().join(format!("ccview-ext-unique-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let spec = CommandSpec::new("true");
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let (dir, spec) = (dir.clone(), spec.clone());
                std::thread::spawn(move || (0..200).all(|_| write_script(&dir, &spec).is_ok()))
            })
            .collect();
        for worker in workers {
            assert!(worker.join().unwrap(), "a launcher script name collided");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_valid_variable_names_are_unset() {
        let mut spec = CommandSpec::new("true");
        spec.env_remove = vec!["GOOD_1".into(), "bad name; touch pwned".into(), "1BAD".into(), String::new()];
        let script = render_script(&spec);
        assert!(script.contains("unset GOOD_1\n"));
        assert_eq!(script.matches("unset").count(), 1, "{script}");
    }

    #[test]
    fn hostile_working_directory_and_env_are_quoted() {
        let base = std::env::temp_dir().join(format!("ccview-cwd-{}", std::process::id()));
        let cwd = base.join("it's a $(touch pwned) dir\nsecond line");
        fs::create_dir_all(&cwd).unwrap();
        let spec = CommandSpec::new("sh")
            .args(["-c", r#"pwd -P; printf '%s' "$HOSTILE_VALUE""#])
            .cwd(&cwd)
            .env("HOSTILE_VALUE", "a'b $(x)");
        let output = run_script("cwd", &spec);
        let expected_cwd = fs::canonicalize(&cwd).unwrap();
        assert_eq!(output, format!("{}\na'b $(x)", expected_cwd.display()));
        assert!(!cwd.join("pwned").exists() && !base.join("pwned").exists());
        fs::remove_dir_all(&base).unwrap();
    }
}
