use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "swagri-updater",
    version,
    about = "Apply a verified Swagri agent update"
)]
struct Args {
    #[arg(long)]
    target: PathBuf,
    #[arg(long)]
    replacement: PathBuf,
    #[arg(long)]
    backup: PathBuf,
    #[arg(long)]
    restart_args: PathBuf,
    #[arg(long)]
    no_restart: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    apply_update(
        &args.target,
        &args.replacement,
        &args.backup,
        &args.restart_args,
        args.no_restart,
    )
}

fn apply_update(
    target: &Path,
    replacement: &Path,
    backup: &Path,
    restart_args: &Path,
    no_restart: bool,
) -> Result<()> {
    if !target.is_file() {
        bail!("target does not exist: {}", target.display());
    }
    if !replacement.is_file() {
        bail!(
            "verified replacement does not exist: {}",
            replacement.display()
        );
    }

    let arguments: Vec<String> = if no_restart {
        Vec::new()
    } else {
        serde_json::from_slice(
            &fs::read(restart_args)
                .with_context(|| format!("could not read {}", restart_args.display()))?,
        )
        .context("restart arguments are invalid")?
    };
    let prepared = target.with_extension("swagri-new");
    if prepared.exists() {
        fs::remove_file(&prepared)
            .with_context(|| format!("could not remove stale {}", prepared.display()))?;
    }
    fs::copy(replacement, &prepared).context("could not stage replacement beside target")?;

    for _ in 0..120 {
        if backup.exists() {
            let _ = fs::remove_file(backup);
        }
        match fs::rename(target, backup) {
            Ok(()) => break,
            Err(_) => thread::sleep(Duration::from_millis(250)),
        }
    }
    if target.exists() {
        let _ = fs::remove_file(&prepared);
        bail!("agent did not stop before the update timeout");
    }

    if let Err(error) = fs::rename(&prepared, target) {
        let _ = fs::rename(backup, target);
        return Err(error).context("could not activate replacement; previous version restored");
    }

    if no_restart {
        let health = Command::new(target)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(health, Ok(status) if status.success()) {
            rollback(target, backup);
            bail!("updated agent failed its version health check; previous version restored");
        }
        let _ = fs::remove_file(replacement);
        let _ = fs::remove_file(restart_args);
        return Ok(());
    }

    let mut child = match Command::new(target)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            rollback(target, backup);
            return Err(error).context("new agent could not start; previous version restored");
        }
    };

    thread::sleep(Duration::from_secs(2));
    if let Some(status) = child.try_wait().context("could not check updated agent")? {
        rollback(target, backup);
        bail!("updated agent exited during health check ({status}); previous version restored");
    }

    let _ = fs::remove_file(replacement);
    let _ = fs::remove_file(restart_args);
    Ok(())
}

fn rollback(target: &Path, backup: &Path) {
    let _ = fs::remove_file(target);
    let _ = fs::rename(backup, target);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_target() {
        let root = std::env::temp_dir().join(format!("swagri-updater-test-{}", std::process::id()));
        let result = apply_update(
            &root.join("missing"),
            &root.join("replacement"),
            &root.join("backup"),
            &root.join("args"),
            false,
        );
        assert!(result.is_err());
    }
}
