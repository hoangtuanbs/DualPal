//! Open the system terminal in a local folder.
//!
//! Windows prefers Windows Terminal (`wt.exe`) and falls back to PowerShell,
//! then `cmd.exe`, each in a new console window. macOS uses Terminal.app.
//! Linux honours `$TERMINAL`, then tries the common terminal emulators.
//!
//! When `DUALPAL_TERMINAL_LOG` is set the folder is recorded there instead of
//! starting anything, so end-to-end tests can check the action.

use std::path::{Path, PathBuf};

use crate::difftools::find_program;
use crate::{Error, Result};

/// The program to start, its arguments, and the folder it starts in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Windows only: start it in a console window of its own.
    pub new_console: bool,
}

/// Which OS to build the command for; [`Os::current`] in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// Linux terminals tried in order after `$TERMINAL`, with the flag that sets
/// the start folder (empty when the terminal starts in its working folder).
const LINUX_TERMINALS: &[(&str, &str)] = &[
    ("x-terminal-emulator", ""),
    ("gnome-terminal", "--working-directory="),
    ("konsole", "--workdir="),
    ("xfce4-terminal", "--working-directory="),
    ("kitty", "--directory="),
    ("alacritty", "--working-directory="),
    ("wezterm", ""),
    ("tilix", "--working-directory="),
    ("xterm", ""),
];

/// Build the command for `dir`, using `find` to look programs up on PATH.
pub fn command_for(
    os: Os,
    dir: &Path,
    terminal_env: Option<&str>,
    find: impl Fn(&str) -> Option<PathBuf>,
) -> Result<TerminalCommand> {
    let cwd = dir.to_path_buf();
    let with = |program: PathBuf, args: Vec<String>, new_console: bool| TerminalCommand {
        program,
        args,
        cwd: cwd.clone(),
        new_console,
    };
    match os {
        Os::Windows => {
            if let Some(wt) = find("wt") {
                return Ok(with(
                    wt,
                    vec!["-d".into(), dir.display().to_string()],
                    false,
                ));
            }
            if let Some(ps) = find("pwsh").or_else(|| find("powershell")) {
                return Ok(with(ps, vec!["-NoExit".into()], true));
            }
            Ok(with(
                find("cmd").unwrap_or_else(|| "cmd.exe".into()),
                vec![],
                true,
            ))
        }
        Os::MacOs => Ok(with(
            "open".into(),
            vec!["-a".into(), "Terminal".into(), dir.display().to_string()],
            false,
        )),
        Os::Linux => {
            if let Some(t) = terminal_env.map(str::trim).filter(|t| !t.is_empty()) {
                if let Some(p) = find(t) {
                    return Ok(with(p, vec![], false));
                }
            }
            for (name, flag) in LINUX_TERMINALS {
                if let Some(p) = find(name) {
                    let args = if flag.is_empty() {
                        vec![]
                    } else {
                        vec![format!("{flag}{}", dir.display())]
                    };
                    return Ok(with(p, args, false));
                }
            }
            Err(Error::Unsupported(
                "opening a terminal: no terminal app was found. Set $TERMINAL to the one you use"
                    .into(),
            ))
        }
    }
}

/// Open the system terminal in the local folder `dir` without waiting for it.
pub fn open(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(Error::NotFound(format!("folder {}", dir.display())));
    }
    if crate::launch_log::record("DUALPAL_TERMINAL_LOG", &dir.display().to_string()) {
        return Ok(());
    }
    let terminal_env = std::env::var("TERMINAL").ok();
    let tc = command_for(Os::current(), dir, terminal_env.as_deref(), find_program)?;
    let mut cmd = std::process::Command::new(&tc.program);
    cmd.args(&tc.args)
        .current_dir(&tc.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    if tc.new_console {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        cmd.creation_flags(CREATE_NEW_CONSOLE)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
    }
    let mut child = cmd.spawn().map_err(|e| {
        Error::Unsupported(format!(
            "starting the terminal {} ({e})",
            tc.program.display()
        ))
    })?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> Option<PathBuf> + 'a {
        move |n| {
            names
                .contains(&n)
                .then(|| PathBuf::from(format!("/bin/{n}")))
        }
    }

    #[test]
    fn windows_prefers_windows_terminal_with_the_folder() {
        let c = command_for(
            Os::Windows,
            Path::new("C:\\work"),
            None,
            only(&["wt", "powershell"]),
        )
        .unwrap();
        assert_eq!(c.program, PathBuf::from("/bin/wt"));
        assert_eq!(c.args, vec!["-d", "C:\\work"]);
        assert!(!c.new_console);
    }

    #[test]
    fn windows_falls_back_to_powershell_then_cmd_in_a_new_console() {
        let ps = command_for(Os::Windows, Path::new("C:\\w"), None, only(&["powershell"])).unwrap();
        assert_eq!(ps.program, PathBuf::from("/bin/powershell"));
        assert_eq!(ps.cwd, PathBuf::from("C:\\w"));
        assert!(ps.new_console);
        let cmd = command_for(Os::Windows, Path::new("C:\\w"), None, only(&[])).unwrap();
        assert_eq!(cmd.program, PathBuf::from("cmd.exe"));
        assert!(cmd.new_console);
    }

    #[test]
    fn macos_opens_terminal_app_on_the_folder() {
        let c = command_for(Os::MacOs, Path::new("/Users/a"), None, only(&[])).unwrap();
        assert_eq!(c.program, PathBuf::from("open"));
        assert_eq!(c.args, vec!["-a", "Terminal", "/Users/a"]);
    }

    #[test]
    fn linux_uses_terminal_env_first_then_known_terminals() {
        let c = command_for(
            Os::Linux,
            Path::new("/home/a"),
            Some("kitty"),
            only(&["kitty", "gnome-terminal"]),
        )
        .unwrap();
        assert_eq!(c.program, PathBuf::from("/bin/kitty"));
        assert_eq!(c.cwd, PathBuf::from("/home/a"));
        let g = command_for(
            Os::Linux,
            Path::new("/home/a"),
            Some("missing"),
            only(&["gnome-terminal"]),
        )
        .unwrap();
        assert_eq!(g.args, vec!["--working-directory=/home/a"]);
    }

    #[test]
    fn linux_without_a_terminal_says_so() {
        let e = command_for(Os::Linux, Path::new("/"), None, only(&[])).unwrap_err();
        assert!(e.to_string().contains("TERMINAL"), "{e}");
    }

    #[test]
    fn open_rejects_a_missing_folder() {
        let e = open(Path::new("/definitely/not/here/dualpal")).unwrap_err();
        assert!(matches!(e, Error::NotFound(_)), "{e}");
    }
}
