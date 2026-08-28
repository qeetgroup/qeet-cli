//! `qeet self-update` — update the CLI itself.
//!
//! Deliberately does **not** implement its own updater. Instead it works out how qeet was
//! installed and hands over to whatever installed it. That means:
//!
//! - no HTTP client, no TLS stack and no JSON parser added to a local-first CLI;
//! - no fighting Homebrew, which owns its own prefix and would be left inconsistent by a
//!   binary that overwrote itself behind brew's back;
//! - the upgrade path is always the one the machine's package manager expects.
//!
//! The cost is honest: qeet cannot tell you whether a newer version exists without asking
//! the network, so it does not claim to. It tells you the one command that will find out.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::output::style::{dim, heading, name, ok, symbol, warn};

/// How this binary got here.
#[derive(Debug, PartialEq, Eq)]
enum Installed {
    Homebrew,
    /// The shell or PowerShell installer, which leaves a receipt behind.
    Installer,
    /// `cargo install`.
    Cargo,
    Unknown,
}

pub async fn run() -> Result<(), Error> {
    let exe = std::env::current_exe().ok();
    let method = detect(exe.as_deref(), receipt_path().as_deref());

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", heading("qeet self-update"));
    let _ = writeln!(out, "  {} {}", dim("installed version"), name(env!("CARGO_PKG_VERSION")));
    if let Some(exe) = &exe {
        let _ = writeln!(out, "  {} {}", dim("binary"), dim(exe.display()));
    }
    let _ = writeln!(out);

    let (label, command) = match method {
        Installed::Homebrew => (
            "Homebrew",
            Some("brew update && brew upgrade qeet".to_string()),
        ),
        Installed::Installer => (
            "install script",
            Some(
                "curl --proto '=https' --tlsv1.2 -fsSL \\\n    https://github.com/qeetgroup/qeet-cli/releases/latest/download/qeet-cli-installer.sh | sh"
                    .to_string(),
            ),
        ),
        Installed::Cargo => (
            "cargo",
            Some("cargo install --git https://github.com/qeetgroup/qeet-cli --locked".to_string()),
        ),
        Installed::Unknown => ("unrecognised", None),
    };

    let _ = writeln!(out, "  {} {label}", dim("installed via"));
    let _ = writeln!(out);

    match command {
        Some(command) => {
            let _ = writeln!(out, "  {} Run:", ok(symbol::OK));
            let _ = writeln!(out);
            for line in command.lines() {
                let _ = writeln!(out, "      {}", name(line));
            }
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  {}",
                dim("qeet does not overwrite itself — that would leave your package manager")
            );
            let _ = writeln!(
                out,
                "  {}",
                dim("inconsistent. Running the command above keeps it in charge.")
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  {} Could not tell how this binary was installed.",
                warn(symbol::WARN)
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "  Reinstall by whichever route you prefer:");
            let _ = writeln!(out, "      {}", name("brew install qeetgroup/tap/qeet"));
            let _ = writeln!(
                out,
                "      {}",
                name(
                    "curl -fsSL https://github.com/qeetgroup/qeet-cli/releases/latest/download/qeet-cli-installer.sh | sh"
                )
            );
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", dim("Releases: https://github.com/qeetgroup/qeet-cli/releases"));
    let _ = out.flush();
    Ok(())
}

fn receipt_path() -> Option<PathBuf> {
    use etcetera::BaseStrategy as _;
    let strategy = etcetera::base_strategy::choose_native_strategy().ok()?;
    Some(strategy.config_dir().join("qeet-cli").join("qeet-cli-receipt.json"))
}

/// Work out the install method from the binary's location and the installer's receipt.
///
/// Separated from I/O so every branch is testable.
fn detect(exe: Option<&Path>, receipt: Option<&Path>) -> Installed {
    if let Some(exe) = exe {
        let path = exe.to_string_lossy();
        // Covers both Apple-silicon (/opt/homebrew) and Intel (/usr/local) prefixes, and
        // Linuxbrew. Cellar appears in the resolved path of a brew-installed binary.
        if path.contains("/Cellar/")
            || path.starts_with("/opt/homebrew/")
            || path.contains("/linuxbrew/")
        {
            return Installed::Homebrew;
        }
        if path.contains("/.cargo/bin/") {
            return Installed::Cargo;
        }
    }
    if receipt.is_some_and(Path::exists) {
        return Installed::Installer;
    }
    Installed::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_homebrew_layouts() {
        for path in [
            "/opt/homebrew/bin/qeet",
            "/opt/homebrew/Cellar/qeet/0.1.3/bin/qeet",
            "/usr/local/Cellar/qeet/0.1.3/bin/qeet",
            "/home/linuxbrew/.linuxbrew/bin/qeet",
        ] {
            assert_eq!(detect(Some(Path::new(path)), None), Installed::Homebrew, "{path}");
        }
    }

    #[test]
    fn recognises_cargo_install() {
        assert_eq!(detect(Some(Path::new("/Users/x/.cargo/bin/qeet")), None), Installed::Cargo);
    }

    #[test]
    fn recognises_the_install_script_by_its_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let receipt = dir.path().join("qeet-cli-receipt.json");
        std::fs::write(&receipt, "{}").expect("write");
        assert_eq!(
            detect(Some(Path::new("/Users/x/.local/bin/qeet")), Some(&receipt)),
            Installed::Installer
        );
    }

    #[test]
    fn admits_when_it_cannot_tell() {
        assert_eq!(detect(Some(Path::new("/opt/custom/qeet")), None), Installed::Unknown);
        assert_eq!(detect(None, None), Installed::Unknown);
    }

    /// Homebrew wins over a stray receipt: brew owns the binary, so brew must do the upgrade.
    #[test]
    fn homebrew_takes_priority_over_a_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let receipt = dir.path().join("r.json");
        std::fs::write(&receipt, "{}").expect("write");
        assert_eq!(
            detect(Some(Path::new("/opt/homebrew/bin/qeet")), Some(&receipt)),
            Installed::Homebrew
        );
    }
}
