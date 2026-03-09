#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

use clap::Parser;
use colored::*;
use eyre::{Context, Result};
use log::info;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod cli;
mod config;
mod report;

use cli::{Cli, Commands, CronAction};
use config::Config;
use report::{Action, Report};

fn setup_logging() -> Result<()> {
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kondo")
        .join("logs");

    fs::create_dir_all(&log_dir).context("Failed to create log directory")?;

    let log_file = log_dir.join("kondo.log");

    let target = Box::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .context("Failed to open log file")?,
    );

    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Pipe(target))
        .init();

    info!("Logging initialized, writing to: {}", log_file.display());
    Ok(())
}

/// Run dashify on a file, returning the new path (may be renamed).
/// Uses --dry-run first to determine the new name, then renames.
fn dashify_file(path: &Path, dry_run: bool) -> Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    // Use --dry-run to discover the new name
    let output = match Command::new("dashify")
        .arg("--force-dash")
        .arg("--dry-run")
        .arg(path)
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            log::warn!("dashify not found, skipping rename for {}", path.display());
            return Ok(path.to_path_buf());
        }
    };

    if !output.status.success() {
        log::warn!("dashify returned non-zero for {}", path.display());
        return Ok(path.to_path_buf());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();

    // If no output, file already has a dashified name
    if line.is_empty() {
        return Ok(path.to_path_buf());
    }

    // Parse "old -> new" format
    let new_name = match line.split_once(" -> ") {
        Some((_, new)) => new.trim(),
        None => return Ok(path.to_path_buf()),
    };

    let new_path = parent.join(new_name);

    if dry_run {
        // Don't actually rename, but return the would-be path for display
        return Ok(path.to_path_buf());
    }

    // Actually run dashify (without --dry-run)
    let succeeded = Command::new("dashify")
        .arg("--force-dash")
        .arg(path)
        .status()
        .ok()
        .map(|s| s.success())
        .unwrap_or(false);

    if !succeeded {
        log::warn!("dashify rename failed for {}", path.display());
        return Ok(path.to_path_buf());
    }

    if new_path.exists() {
        Ok(new_path)
    } else if path.exists() {
        Ok(path.to_path_buf())
    } else {
        eyre::bail!(
            "dashify renamed {} but couldn't find result at {}",
            path.display(),
            new_path.display()
        )
    }
}

/// Move a file to the destination directory, preserving the filename.
/// Returns the Action taken and the destination path.
fn move_file(src: &Path, dest_dir: &Path, dry_run: bool) -> Result<(Action, PathBuf, Option<String>)> {
    let filename = match src.file_name() {
        Some(f) => f,
        None => {
            return Ok((Action::Skip, dest_dir.to_path_buf(), Some("no filename".to_string())));
        }
    };
    let dest = dest_dir.join(filename);

    // Don't overwrite existing files
    if dest.exists() {
        log::info!("Skipping {} -> {} (exists)", src.display(), dest.display());
        return Ok((
            Action::Skip,
            dest,
            Some(format!("already exists at {}", dest_dir.display())),
        ));
    }

    if dry_run {
        return Ok((Action::Move, dest, None));
    }

    // Ensure destination directory exists
    fs::create_dir_all(dest_dir).context(format!("Failed to create directory {}", dest_dir.display()))?;

    // Try rename first (same filesystem), fall back to copy+remove
    if fs::rename(src, &dest).is_err() {
        fs::copy(src, &dest).context(format!("Failed to copy {} -> {}", src.display(), dest.display()))?;
        fs::remove_file(src).context(format!("Failed to remove source {}", src.display()))?;
    }

    log::info!("Moved {} -> {}", src.display(), dest.display());
    Ok((Action::Move, dest, None))
}

/// Scan source directories and organize files according to rules
fn organize(config: &Config, ext_map: &HashMap<String, PathBuf>, dry_run: bool) -> Result<Report> {
    let mut report = Report::default();

    for source in config.source_paths() {
        if !source.exists() {
            report.push(
                Action::Skip,
                source.clone(),
                None,
                Some("source directory not found".to_string()),
            );
            continue;
        }

        let entries = fs::read_dir(&source).context(format!("Failed to read directory {}", source.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only process regular files (skip symlinks)
            if !path.is_file() || path.is_symlink() {
                continue;
            }

            // Get extension
            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            // Look up destination
            let dest_dir = match ext_map.get(&ext) {
                Some(d) => d.clone(),
                None => {
                    report.push(Action::Skip, path, None, Some("no matching rule".to_string()));
                    continue;
                }
            };

            // Optionally dashify the filename first
            let final_path = if config.dashify {
                match dashify_file(&path, dry_run) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("dashify failed for {}: {}", path.display(), e);
                        report.push(
                            Action::Error,
                            path.clone(),
                            None,
                            Some(format!("dashify failed: {}", e)),
                        );
                        path.clone()
                    }
                }
            } else {
                path.clone()
            };

            let (action, dest, reason) = move_file(&final_path, &dest_dir, dry_run)?;
            report.push(action, final_path, Some(dest), reason);
        }
    }

    Ok(report)
}

/// Get the path to the kondo binary
fn kondo_binary_path() -> Result<PathBuf> {
    std::env::current_exe().context("Failed to determine kondo binary path")
}

/// Install a user cron job
fn install_cron(schedule: &str, config_path: Option<&PathBuf>) -> Result<()> {
    let binary = kondo_binary_path()?;
    let mut cmd = format!("{}", binary.display());
    if let Some(cfg) = config_path {
        cmd = format!("{} --config {}", cmd, cfg.display());
    }

    let cron_line = format!("{} {}", schedule, cmd);
    let marker = "# kondo-auto";

    // Read existing crontab
    let existing = Command::new("crontab")
        .arg("-l")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    // Remove any existing kondo lines
    let filtered: Vec<&str> = existing.lines().filter(|line| !line.contains(marker)).collect();

    let mut new_crontab = filtered.join("\n");
    if !new_crontab.is_empty() && !new_crontab.ends_with('\n') {
        new_crontab.push('\n');
    }
    new_crontab.push_str(&format!("{} {}\n", cron_line, marker));

    // Write new crontab
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn crontab")?;

    use std::io::Write;
    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(new_crontab.as_bytes())
            .context("Failed to write crontab")?;
    }

    let status = child.wait().context("Failed to wait for crontab")?;
    if !status.success() {
        eyre::bail!("crontab command failed");
    }

    println!("{} Cron job installed: {}", "installed".green(), cron_line);
    Ok(())
}

/// Remove the kondo cron job
fn uninstall_cron() -> Result<()> {
    let marker = "# kondo-auto";

    let existing = Command::new("crontab")
        .arg("-l")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let filtered: Vec<&str> = existing.lines().filter(|line| !line.contains(marker)).collect();

    let new_crontab = format!("{}\n", filtered.join("\n"));

    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn crontab")?;

    use std::io::Write;
    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(new_crontab.as_bytes())
            .context("Failed to write crontab")?;
    }

    let status = child.wait().context("Failed to wait for crontab")?;
    if !status.success() {
        eyre::bail!("crontab command failed");
    }

    println!("{} Cron job removed", "uninstalled".green());
    Ok(())
}

fn main() -> Result<()> {
    setup_logging().context("Failed to setup logging")?;

    let cli = Cli::parse();
    let config = Config::load(cli.config.as_ref()).context("Failed to load configuration")?;

    info!("Starting with config from: {:?}", cli.config);

    // Handle subcommands
    if let Some(Commands::Cron { action, schedule }) = &cli.command {
        return match action {
            CronAction::Install => install_cron(schedule, cli.config.as_ref()),
            CronAction::Reinstall => {
                let _ = uninstall_cron();
                install_cron(schedule, cli.config.as_ref())
            }
            CronAction::Uninstall => uninstall_cron(),
            CronAction::Status => {
                println!("{}", cli::check_cron_status());
                Ok(())
            }
        };
    }

    // Run organization
    let ext_map = config.extension_map();

    if cli.verbose || cli.dry_run {
        println!(
            "{} Scanning {} source(s) with {} extension rule(s)",
            "kondo".cyan().bold(),
            config.sources.len(),
            ext_map.len()
        );
        if cli.dry_run {
            println!("{}", "  (dry run - no files will be moved)".yellow());
        }
        println!();
    }

    let report = organize(&config, &ext_map, cli.dry_run)?;
    report.print(cli.dry_run, cli.verbose);

    Ok(())
}
