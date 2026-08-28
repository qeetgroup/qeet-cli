//! `qeet doctor` — can this machine actually use qeet?
//!
//! Each check is a fact, not a guess, and each failure says what to do. The SSH identity
//! check exists because of a real incident: `git@github.com` can authenticate as a different
//! GitHub account than the one that can see an organization's private repositories, and the
//! only symptom is every private repo reporting "not found".

use std::io::Write;
use std::process::Stdio;

use super::context;
use crate::error::Error;
use crate::git::Git;
use crate::manifest::source;
use crate::output::style::{bad, dim, heading, ok, symbol, warn};

enum Verdict {
    Pass(String),
    Warn(String, String),
    Fail(String, String),
}

pub async fn run(args: &crate::cli::ManifestArgs) -> Result<bool, Error> {
    let mut checks: Vec<(&str, Verdict)> = Vec::new();

    // 1. git, and its version.
    checks.push((
        "git",
        match git_version().await {
            Some(version) => Verdict::Pass(version),
            None => Verdict::Fail(
                "not found".into(),
                "Install git and make sure it is on your PATH.".into(),
            ),
        },
    ));

    // 2. the manifest actually in effect, and whether it validates.
    checks.push((
        "manifest",
        match context::manifest(args.manifest.as_deref()) {
            Ok(loaded) => {
                let where_from = loaded.note.unwrap_or_else(|| "built-in registry".to_string());
                Verdict::Pass(format!(
                    "{where_from} — {} products, {} repositories",
                    loaded.manifest.products.len(),
                    loaded.manifest.products.values().map(|p| p.repositories.len()).sum::<usize>()
                ))
            }
            Err(err) => Verdict::Fail(
                "invalid".into(),
                err.to_string().lines().next().unwrap_or("see `qeet products`").to_string(),
            ),
        },
    ));

    // 3. a user-level override, which is the usual cause of "why is it cloning that?".
    checks.push((
        "user config",
        match source::user_config_path() {
            // Informational, not a warning: an override is a legitimate setup -- it is how
            // you point qeet at an SSH host alias. The `manifest` line above already says
            // which file won, so this only needs to confirm the file is intentional.
            Some(path) if path.exists() => Verdict::Pass(format!(
                "{} in effect (delete it to fall back to the built-in registry)",
                path.display()
            )),
            Some(path) => Verdict::Pass(format!("none ({} would be used)", path.display())),
            None => Verdict::Pass("none".into()),
        },
    ));

    // 4. can we write where we are standing?
    checks.push((
        "workspace",
        match std::env::current_dir() {
            Ok(dir) => match writable(&dir) {
                true => Verdict::Pass(format!("{} is writable", dir.display())),
                false => Verdict::Fail(
                    format!("{} is not writable", dir.display()),
                    "Change to a directory you own before cloning.".into(),
                ),
            },
            Err(err) => Verdict::Fail(err.to_string(), "Change to a valid directory.".into()),
        },
    ));

    // 5 and 6 depend on the manifest, because the host to test is whatever the manifest
    // says -- checking github.com when the manifest points at an SSH alias would test the
    // wrong thing and report a problem that does not exist.
    if let Ok(loaded) = context::manifest(args.manifest.as_deref()) {
        let host = loaded.manifest.remote.host.clone();

        // Reported as a fact, not a verdict: qeet cannot know which account *should* answer.
        checks.push(("ssh identity", ssh_identity(&format!("git@{host}")).await));

        // The check that earns doctor its place. A wrong SSH identity looks exactly like
        // "repository not found", and nothing else in the CLI can tell the difference.
        checks.push(("remote access", remote_access(&loaded.manifest).await));
    }

    let mut out = std::io::stdout().lock();
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", heading("qeet doctor"));
    let _ = writeln!(out);

    let width = checks.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    let mut failures = 0;
    let mut warnings = 0;
    let mut advice: Vec<String> = Vec::new();

    for (label, verdict) in &checks {
        let line = match verdict {
            Verdict::Pass(detail) => format!("{} {}", ok(symbol::OK), detail),
            Verdict::Warn(detail, hint) => {
                warnings += 1;
                advice.push(hint.clone());
                format!("{} {}", warn(symbol::WARN), warn(detail))
            }
            Verdict::Fail(detail, hint) => {
                failures += 1;
                advice.push(hint.clone());
                format!("{} {}", bad(symbol::BAD), bad(detail))
            }
        };
        let _ = writeln!(out, "  {:<width$}  {line}", dim(label));
    }

    if !advice.is_empty() {
        let _ = writeln!(out);
        for hint in &advice {
            let _ = writeln!(out, "  {} {hint}", dim("→"));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}",
        match (failures, warnings) {
            (0, 0) => ok("Ready.").to_string(),
            (0, w) => warn(format!(
                "Usable, with {w} thing{} worth looking at.",
                if w == 1 { "" } else { "s" }
            ))
            .to_string(),
            (f, _) => bad(format!(
                "{f} problem{} to fix before qeet will work.",
                if f == 1 { "" } else { "s" }
            ))
            .to_string(),
        }
    );
    let _ = out.flush();

    Ok(failures == 0)
}

/// Try `git ls-remote` against one real repository from the manifest.
///
/// Picks a private repository when the manifest has one, because a public repository clones
/// for *any* identity and would prove nothing about organization access.
async fn remote_access(manifest: &crate::manifest::Manifest) -> Verdict {
    let protocol = manifest.remote.protocol;
    // `*-files` repositories are the organization's private specification repos, so they are
    // the sharpest test of whether this identity has real access.
    let Some((repo, url)) = manifest
        .products
        .values()
        .flat_map(|product| &product.repositories)
        .map(|entry| (entry.name.clone(), manifest.url_for(entry, protocol)))
        .min_by_key(|(name, _)| (!name.ends_with("-files"), name.clone()))
    else {
        return Verdict::Warn(
            "no repositories in the manifest".into(),
            "Check the manifest.".into(),
        );
    };

    let output = tokio::process::Command::new("git")
        .args(["ls-remote", "--exit-code", "--heads", &url])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            Verdict::Pass(format!("can reach {repo} over {protocol}"))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let kind = crate::git::classify::classify(&stderr);
            Verdict::Warn(
                format!("cannot reach {repo} over {protocol} — {}", kind.summary()),
                format!(
                    "This identity cannot see {repo}. Either use `--protocol https`, or point \
                     the manifest at an SSH host alias that has access (see the README)."
                ),
            )
        }
        Err(err) => Verdict::Warn(
            format!("could not run git ls-remote: {err}"),
            "Check that git is on your PATH.".into(),
        ),
    }
}

async fn git_version() -> Option<String> {
    Git::discover().await.ok()?;
    let out = tokio::process::Command::new("git")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".qeet-doctor-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Ask a git host over SSH who we are. GitHub answers `Hi <user>!` and exits non-zero, which
/// is success for our purposes -- we only want the name.
async fn ssh_identity(target: &str) -> Verdict {
    let output = tokio::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-T", target])
        .stdin(Stdio::null())
        .output()
        .await;

    let Ok(output) = output else {
        return Verdict::Warn(
            "could not run ssh".into(),
            "Not fatal — HTTPS works too: `qeet clone <product> --protocol https`.".into(),
        );
    };

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    match text.split("Hi ").nth(1).and_then(|rest| rest.split('!').next()) {
        Some(user) => Verdict::Pass(format!("authenticates as {user}")),
        None if text.contains("Permission denied") => Verdict::Warn(
            "key rejected".into(),
            "SSH will not reach private repositories. Use --protocol https, or add a key.".into(),
        ),
        None => Verdict::Warn(
            "identity unclear".into(),
            "Run `ssh -T git@github.com` yourself to see which account answers.".into(),
        ),
    }
}
