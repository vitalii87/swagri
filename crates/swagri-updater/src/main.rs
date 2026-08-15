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
    about = "Apply a verified Swagri component update"
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
    /// Optional version marker written after a successful component restart.
    #[arg(long, requires = "replacement_version")]
    version_marker: Option<PathBuf>,
    /// Version stored in --version-marker after successful activation.
    #[arg(long, requires = "version_marker")]
    replacement_version: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    apply_update(
        &args.target,
        &args.replacement,
        &args.backup,
        &args.restart_args,
        args.no_restart,
        args.version_marker.as_deref(),
        args.replacement_version.as_deref(),
    )
}

fn apply_update(
    target: &Path,
    replacement: &Path,
    backup: &Path,
    restart_args: &Path,
    no_restart: bool,
    version_marker: Option<&Path>,
    replacement_version: Option<&str>,
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

    let previous_marker = version_marker.map(|marker| fs::read(marker).ok());
    if let (Some(marker), Some(version)) = (version_marker, replacement_version)
        && let Err(error) = fs::write(marker, format!("{version}\n"))
    {
        rollback(target, backup);
        restore_marker(
            marker,
            previous_marker.as_ref().and_then(|value| value.as_deref()),
        );
        return Err(error).context("could not update component version marker; rollback completed");
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
            if let Some(marker) = version_marker {
                restore_marker(
                    marker,
                    previous_marker.as_ref().and_then(|value| value.as_deref()),
                );
            }
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
            if let Some(marker) = version_marker {
                restore_marker(
                    marker,
                    previous_marker.as_ref().and_then(|value| value.as_deref()),
                );
            }
            return Err(error).context("new agent could not start; previous version restored");
        }
    };

    thread::sleep(Duration::from_secs(2));
    if let Some(status) = child.try_wait().context("could not check updated agent")? {
        rollback(target, backup);
        if let Some(marker) = version_marker {
            restore_marker(
                marker,
                previous_marker.as_ref().and_then(|value| value.as_deref()),
            );
        }
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

fn restore_marker(marker: &Path, previous: Option<&[u8]>) {
    if let Some(previous) = previous {
        let _ = fs::write(marker, previous);
    } else {
        let _ = fs::remove_file(marker);
    }
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
            None,
            None,
        );
        assert!(result.is_err());
    }
}
