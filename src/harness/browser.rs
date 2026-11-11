use std::process::Command;

const SCHEMES: [&str; 2] = ["http://", "https://"];
const MAX_URL_CHARS: usize = 2048;

pub(crate) type Opener = dyn FnMut(&str) -> Result<(), String>;

pub(crate) fn is_openable(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    url.chars().count() <= MAX_URL_CHARS
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
        && SCHEMES.iter().any(|scheme| {
            lowered
                .strip_prefix(scheme)
                .is_some_and(|rest| !rest.is_empty())
        })
}

pub(crate) fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    match host_port.strip_prefix('[') {
        Some(_) => host_port
            .split_inclusive(']')
            .next()
            .unwrap_or_default()
            .to_owned(),
        None => host_port.split(':').next().unwrap_or_default().to_owned(),
    }
}

pub(crate) fn open_with<F>(url: &str, opener: &mut F) -> Result<String, String>
where
    F: FnMut(&str) -> Result<(), String> + ?Sized,
{
    if !is_openable(url) {
        return Err("only http and https links can be opened".to_owned());
    }
    opener(url)?;
    Ok(format!("opened {}", host(url)))
}

pub(crate) fn system_open(url: &str) -> Result<(), String> {
    if cfg!(test) {
        return Err("tests must inject an opener".to_owned());
    }
    if !cfg!(target_os = "macos") {
        return Err("opening links is macOS-only for now".to_owned());
    }
    let status = Command::new("open")
        .arg(url)
        .status()
        .map_err(|err| format!("could not run open: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open failed ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_and_https_ever_reach_the_opener() {
        let mut opened = Vec::new();
        let mut record = |url: &str| {
            opened.push(url.to_owned());
            Ok(())
        };
        for hostile in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "ftp://host/file",
            "-a Calculator",
            "https://",
            "https://a.example/x y",
        ] {
            assert!(open_with(hostile, &mut record).is_err(), "{hostile}");
        }
        let notice = open_with(
            "https://user:secret@Docs.example:8443/p?token=abc",
            &mut record,
        );
        assert_eq!(notice.as_deref(), Ok("opened Docs.example"));
        assert_eq!(
            opened,
            ["https://user:secret@Docs.example:8443/p?token=abc"]
        );
    }
}
