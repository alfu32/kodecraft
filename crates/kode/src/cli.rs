use clap::{ArgAction, Parser, Subcommand};
use kode_core::{DomainError, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(
    name = "kode",
    version,
    about = "Kode terminal IDE",
    disable_version_flag = true,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Open a file or folder. A file takes precedence over restored session focus.
    pub target: Option<PathBuf>,
    /// Override the resource directory (styles, MIME and service catalogs).
    #[arg(long, value_name = "DIRECTORY")]
    pub resources: Option<PathBuf>,
    #[arg(short = 'v', long, action = ArgAction::Version)]
    pub version: Option<bool>,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Subcommand)]
pub enum Command {
    Version,
    /// Serve the terminal IDE through an installed or bundled ttyd executable.
    #[command(disable_help_flag = true)]
    Serve {
        /// Serve options, folder, and optional -- ttyd arguments. Use serve --help.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<std::ffi::OsString>,
    },
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version => f.write_str("Version"),
            Self::Serve { .. } => f.debug_struct("Serve").finish_non_exhaustive(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupTarget {
    pub project_root: PathBuf,
    pub file: Option<PathBuf>,
}
pub fn resolve_target(path: &Path, current_dir: &Path) -> Result<StartupTarget> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };
    let metadata = fs::metadata(&path).map_err(|source| DomainError::Io {
        operation: "open startup target",
        path: path.clone(),
        source,
    })?;
    let canonical = fs::canonicalize(&path).map_err(|source| DomainError::Io {
        operation: "resolve startup target",
        path: path.clone(),
        source,
    })?;
    if metadata.is_dir() {
        return Ok(StartupTarget {
            project_root: canonical,
            file: None,
        });
    }
    if metadata.is_file() {
        // The workspace is the requested file's parent, as in the source CLI.
        // The file itself gets canonical identity so aliases share one buffer.
        let parent = fs::canonicalize(path.parent().unwrap_or(current_dir)).map_err(|source| {
            DomainError::Io {
                operation: "resolve file workspace",
                path: path.clone(),
                source,
            }
        })?;
        return Ok(StartupTarget {
            project_root: parent,
            file: Some(canonical),
        });
    }
    Err(DomainError::Unsupported(format!(
        "path must be a regular file or directory: {}",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_help_no_target_version_and_single_target_are_unambiguous() {
        let cli = Cli::try_parse_from(["kode"]).unwrap();
        assert!(cli.target.is_none());
        assert!(cli.command.is_none());
        assert_eq!(
            Cli::try_parse_from(["kode", "a file.rs"]).unwrap().target,
            Some("a file.rs".into())
        );
        assert!(matches!(
            Cli::try_parse_from(["kode", "version"]).unwrap().command,
            Some(Command::Version)
        ));
        for flag in ["--help", "-h"] {
            assert_eq!(
                Cli::try_parse_from(["kode", flag]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
        for flag in ["--version", "-v"] {
            assert_eq!(
                Cli::try_parse_from(["kode", flag]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayVersion
            );
        }
        assert!(Cli::try_parse_from(["kode", "one", "two"]).is_err());
        assert!(Cli::try_parse_from(["kode", "--unknown"]).is_err());
        for args in [
            vec!["kode", "serve"],
            vec!["kode", "serve", "--help"],
            vec!["kode", "serve", "--", "--public"],
            vec!["kode", "serve", "--credential", "fixture:private"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(cli.command, Some(Command::Serve { .. })));
            assert!(!format!("{cli:?}").contains("private"));
        }
    }
    #[test]
    fn startup_folder_file_spaces_relative_missing_and_special_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let file = root.join("a file.rs");
        fs::write(&file, "text").unwrap();
        assert_eq!(
            resolve_target(Path::new("."), root).unwrap().project_root,
            fs::canonicalize(root).unwrap()
        );
        let target = resolve_target(Path::new("a file.rs"), root).unwrap();
        assert_eq!(target.file, Some(file.clone()));
        assert_eq!(target.project_root, root);
        assert!(resolve_target(Path::new("missing"), root).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&file, root.join("alias.rs")).unwrap();
            assert_eq!(
                resolve_target(Path::new("alias.rs"), root).unwrap().file,
                Some(file)
            );
        }
    }
}
