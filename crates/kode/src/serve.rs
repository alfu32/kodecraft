//! Literal argv adapter for the existing ttyd browser terminal frontend.
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

const HELP: &str = "Usage: kode serve [options] [folder] [-- <ttyd args>]

  --port, -p <number>       HTTP port (default 11045)
  --host <address>          Bind address (default 127.0.0.1)
  --public                  Bind 0.0.0.0
  --credential, -c <u:p>    Basic authentication
  --resources <directory>  Resource override for the child IDE
  -- <args...>              Forward remaining arguments literally to ttyd

Install ttyd on PATH, place it beside kode, or set KODE_TTYD to its executable.
SSH: ssh -L 11045:127.0.0.1:11045 <host>
Chrome app window: google-chrome --app=http://127.0.0.1:11045
Browser tabs reserve Ctrl+T/Ctrl+W; an app window forwards these terminal keys.";

struct Options {
    port: u16,
    host: OsString,
    credential: Option<OsString>,
    folder: PathBuf,
    resources: Option<PathBuf>,
    forwarded: Vec<OsString>,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn parse(args: &[OsString], cwd: &Path) -> io::Result<Options> {
    let mut options = Options {
        port: 11045,
        host: "127.0.0.1".into(),
        credential: None,
        folder: cwd.into(),
        resources: None,
        forwarded: Vec::new(),
    };
    let mut folder = false;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let text = arg.to_str().unwrap_or("");
        if text == "--" {
            options.forwarded.extend_from_slice(&args[i + 1..]);
            break;
        }
        let (flag, inline) = text
            .split_once('=')
            .map_or((text, None), |(a, b)| (a, Some(b)));
        match flag {
            "--port" | "--post" | "-p" | "--host" | "--credential" | "-c" | "--resources" => {
                let value = if let Some(value) = inline {
                    OsString::from(value)
                } else {
                    i += 1;
                    args.get(i).cloned().ok_or_else(|| {
                        invalid("Missing serve option value; see kode serve --help")
                    })?
                };
                match flag {
                    "--port" | "--post" | "-p" => {
                        options.port = value
                            .to_str()
                            .and_then(|v| v.parse::<u16>().ok())
                            .filter(|v| *v != 0)
                            .ok_or_else(|| {
                                invalid("Serve port must be a number between 1 and 65535")
                            })?;
                    }
                    "--host" => {
                        if value.to_string_lossy().trim().is_empty() {
                            // The source's empty --host= leaves the current host.
                            if inline.is_none() {
                                return Err(invalid("Missing serve host"));
                            }
                        } else {
                            options.host = value;
                        }
                    }
                    "--credential" | "-c" => {
                        if !value.to_string_lossy().contains(':') {
                            return Err(invalid(
                                "Invalid serve credential; expected user:password",
                            ));
                        }
                        options.credential = Some(value);
                    }
                    "--resources" => {
                        if value.is_empty() {
                            return Err(invalid("Missing resource directory"));
                        }
                        let value = PathBuf::from(value);
                        options.resources = Some(if value.is_absolute() {
                            value
                        } else {
                            cwd.join(value)
                        });
                    }
                    _ => unreachable!(),
                }
            }
            "--public" if inline.is_none() => options.host = "0.0.0.0".into(),
            _ if text.starts_with('-') || folder => {
                options.forwarded.extend_from_slice(&args[i..]);
                break;
            }
            _ => {
                let path = PathBuf::from(arg);
                options.folder = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                folder = true;
            }
        }
        i += 1;
    }
    if !options.folder.is_dir() {
        return Err(invalid("Serve folder must be an existing directory"));
    }
    options.folder = fs::canonicalize(options.folder)?;
    Ok(options)
}
fn arguments(options: &Options, executable: &Path) -> Vec<OsString> {
    let mut args = vec![
        "-p".into(),
        options.port.to_string().into(),
        "-i".into(),
        options.host.clone(),
        "-W".into(),
        "-T".into(),
        "xterm-256color".into(),
        "-w".into(),
        options.folder.clone().into(),
        "-t".into(),
        "titleFixed=Kode".into(),
    ];
    if let Some(credential) = &options.credential {
        // ttyd notice logging includes a reversible Basic auth token. Suppress
        // notices by default; explicitly forwarded debug flags remain ttyd's.
        args.extend(["-d".into(), "3".into(), "-c".into(), credential.clone()]);
    }
    args.extend(options.forwarded.iter().cloned());
    // Terminate ttyd options even when the executable's path starts with '-'.
    args.push("--".into());
    args.push(executable.as_os_str().into());
    if let Some(resources) = &options.resources {
        args.extend(["--resources".into(), resources.as_os_str().into()]);
    }
    args.push(options.folder.clone().into());
    args
}
fn usable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn resolve_ttyd(
    executable: &Path,
    override_path: Option<OsString>,
    search_path: Option<OsString>,
) -> io::Result<PathBuf> {
    if let Some(path) = override_path {
        let path = PathBuf::from(path);
        return if usable(&path) {
            fs::canonicalize(path)
        } else {
            Err(invalid("KODE_TTYD must name an executable ttyd file"))
        };
    }
    let name = if cfg!(windows) { "ttyd.exe" } else { "ttyd" };
    let adjacent = executable.parent().unwrap_or(Path::new(".")).join(name);
    if usable(&adjacent) {
        return Ok(adjacent);
    }
    if let Some(paths) = search_path {
        for directory in std::env::split_paths(&paths) {
            let path = directory.join(name);
            if usable(&path) {
                return fs::canonicalize(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "kode serve needs a ttyd executable for {}/{}; install ttyd on PATH, put it beside kode, or set KODE_TTYD",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    ))
}
pub async fn run(args: Vec<OsString>) -> io::Result<i32> {
    if args.first().is_some_and(|v| v == "--help" || v == "-h") {
        println!("{HELP}");
        return Ok(0);
    }
    let options = parse(&args, &std::env::current_dir()?)?;
    let executable = std::env::current_exe()?;
    let ttyd = resolve_ttyd(
        &executable,
        std::env::var_os("KODE_TTYD"),
        std::env::var_os("PATH"),
    )?;
    println!(
        "Serving Kode at http://{}:{}",
        options.host.to_string_lossy(),
        options.port
    );
    if [
        OsStr::new("127.0.0.1"),
        OsStr::new("localhost"),
        OsStr::new("::1"),
    ]
    .contains(&options.host.as_os_str())
    {
        println!(
            "SSH tunnel: ssh -L {}:127.0.0.1:{} <host>",
            options.port, options.port
        );
    } else if options.credential.is_none() {
        eprintln!("Warning: public writable terminal without --credential.");
    }
    let mut command = std::process::Command::new(ttyd);
    command
        .args(arguments(&options, &executable))
        .current_dir(&options.folder);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // ttyd owns its PTYs and signal handling directly; no orphan wrapper or
        // second process group sits between SSH/systemd and the actual gateway.
        use std::io::Write;
        std::io::stdout().flush()?;
        Err(command.exec())
    }
    #[cfg(not(unix))]
    {
        Ok(command.status()?.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn os(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn defaults_aliases_validation_and_literal_forwarding() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path();
        let defaults = parse(&[], cwd).unwrap();
        assert_eq!(defaults.port, 11045);
        assert_eq!(defaults.host, "127.0.0.1");
        for flag in ["--port", "--post", "-p"] {
            assert_eq!(parse(&os(&[flag, "3012"]), cwd).unwrap().port, 3012);
        }
        for flag in ["--port=3012", "--post=3012"] {
            assert_eq!(parse(&os(&[flag]), cwd).unwrap().port, 3012);
        }
        for bad in [
            &["--port", "0"][..],
            &["--port=65536"],
            &["--port"],
            &["-c", "secret"],
            &["--host", " "],
        ] {
            let error = parse(&os(bad), cwd).err().unwrap().to_string();
            assert!(!error.contains("secret"));
        }
        let options = parse(
            &os(&[
                "--public",
                "--host=localhost",
                "--",
                "--public",
                "a b",
                "$(literal)",
            ]),
            cwd,
        )
        .unwrap();
        assert_eq!(options.host, "localhost");
        assert_eq!(options.forwarded, os(&["--public", "a b", "$(literal)"]));
        let options = parse(&os(&["--unknown", "--public"]), cwd).unwrap();
        assert_eq!(options.host, "127.0.0.1");
        assert_eq!(options.forwarded, os(&["--unknown", "--public"]));
    }
    #[test]
    fn spaces_credentials_resources_and_child_argv_have_distinct_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("workspace with spaces");
        fs::create_dir(&folder).unwrap();
        let options = parse(
            &[
                os(&["-c", "user:p a:ss", "--resources", "resource root"]),
                vec![folder.clone().into()],
                os(&["--", "-t", "fontFamily=a b"]),
            ]
            .concat(),
            temp.path(),
        )
        .unwrap();
        let executable = temp.path().join("bin with spaces/kode");
        let args = arguments(&options, &executable);
        assert!(args.windows(2).any(|v| v == os(&["-c", "user:p a:ss"])));
        assert!(args.windows(2).any(|v| v == os(&["-t", "fontFamily=a b"])));
        let end = args.iter().position(|v| v == "--").unwrap();
        assert_eq!(args[end + 1], executable);
        assert_eq!(args[end + 2], "--resources");
        assert_eq!(args[end + 3], temp.path().join("resource root"));
        assert_eq!(args[end + 4], folder);
    }
    #[test]
    fn missing_or_nonexecutable_launcher_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let kode = temp.path().join("kode");
        assert!(
            resolve_ttyd(&kode, None, None)
                .unwrap_err()
                .to_string()
                .contains("install ttyd")
        );
        assert!(resolve_ttyd(&kode, Some(temp.path().as_os_str().into()), None).is_err());
        let ttyd = temp
            .path()
            .join(if cfg!(windows) { "ttyd.exe" } else { "ttyd" });
        fs::write(&ttyd, b"fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(resolve_ttyd(&kode, None, None).is_err());
            fs::set_permissions(&ttyd, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(resolve_ttyd(&kode, None, None).unwrap(), ttyd);
    }
}
