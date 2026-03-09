use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;

static HELP_TEXT: LazyLock<String> = LazyLock::new(get_tool_validation_help);

fn get_tool_validation_help() -> String {
    let mut help = String::new();

    let dashify_status = check_tool("dashify", "--version");
    help.push_str("OPTIONAL TOOLS:\n");
    help.push_str(&format!(
        "  {} {:<10} {:>12}  (file renaming)\n",
        dashify_status.status_icon, "dashify", dashify_status.version
    ));

    help.push('\n');
    let cron_status = check_cron_status();
    help.push_str(&format!("CRON STATUS:\n  {}\n", cron_status));

    help.push_str("\nLogs are written to: ~/.local/share/kondo/logs/kondo.log");
    help
}

pub fn check_cron_status() -> String {
    let marker = "# kondo-auto";
    match Command::new("crontab").arg("-l").output() {
        Ok(output) if output.status.success() => {
            let crontab = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = crontab.lines().find(|l| l.contains(marker)) {
                let schedule = line.trim().strip_suffix(marker).unwrap_or(line.trim()).trim();
                format!("✅ installed: {}", schedule)
            } else {
                "❌ not installed (use: kondo cron install)".to_string()
            }
        }
        _ => "❌ not installed (use: kondo cron install)".to_string(),
    }
}

struct ToolStatus {
    version: String,
    status_icon: String,
}

fn check_tool(tool: &str, version_arg: &str) -> ToolStatus {
    match Command::new(tool).arg(version_arg).output() {
        Ok(output) if output.status.success() => {
            let version_output = String::from_utf8_lossy(&output.stdout);
            let version = version_output
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().last())
                .unwrap_or("unknown")
                .to_string();
            ToolStatus {
                version,
                status_icon: "✅".to_string(),
            }
        }
        _ => ToolStatus {
            version: "not found".to_string(),
            status_icon: "❌".to_string(),
        },
    }
}

#[derive(Parser)]
#[command(
    name = "kondo",
    about = "Organize files by moving them to the right directories based on extension",
    version = env!("GIT_DESCRIBE"),
    after_help = HELP_TEXT.as_str()
)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Dry run: show what would be done without moving files
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage the kondo cron job
    Cron {
        /// Action to perform
        action: CronAction,

        /// Cron schedule expression (used with install/reinstall)
        #[arg(short, long, default_value = "*/15 * * * *")]
        schedule: String,
    },
}

#[derive(Clone, ValueEnum)]
pub enum CronAction {
    /// Install the cron job
    Install,
    /// Remove and re-install the cron job
    Reinstall,
    /// Remove the cron job
    Uninstall,
    /// Show current cron job status
    Status,
}
