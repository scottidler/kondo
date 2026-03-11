use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;

static HELP_TEXT: LazyLock<String> = LazyLock::new(get_tool_validation_help);

fn get_tool_validation_help() -> String {
    let mut help = String::new();

    let dashify_status = check_tool("dashify", "--version");
    let rkvr_status = check_tool("rkvr", "--version");
    help.push_str("OPTIONAL TOOLS:\n");
    help.push_str(&format!(
        "  {} {:<10} {:>12}  (file renaming)\n",
        dashify_status.status_icon, "dashify", dashify_status.version
    ));
    help.push_str(&format!(
        "  {} {:<10} {:>12}  (safe file removal with recovery)\n",
        rkvr_status.status_icon, "rkvr", rkvr_status.version
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
                let entry = line.trim().strip_suffix(marker).unwrap_or(line.trim()).trim();
                // Extract the 5 cron fields from the beginning of the entry
                let fields: Vec<&str> = entry.splitn(6, ' ').collect();
                let description = if fields.len() >= 5 {
                    let schedule = fields[..5].join(" ");
                    format!(" ({})", describe_cron(&schedule))
                } else {
                    String::new()
                };
                format!("✅ installed: {}{}", entry, description)
            } else {
                "❌ not installed (use: kondo cron install)".to_string()
            }
        }
        _ => "❌ not installed (use: kondo cron install)".to_string(),
    }
}

fn describe_cron(schedule: &str) -> String {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return schedule.to_string();
    }
    let (minute, hour, dom, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);

    let freq = describe_minute(minute);
    let mut parts = vec![freq];

    if hour != "*" {
        parts.push(describe_hour(hour));
    }

    if dow != "*" {
        parts.push(describe_dow(dow));
    }

    if dom != "*" {
        parts.push(format!("on day(s) {}", dom));
    }

    if month != "*" {
        parts.push(format!("in month(s) {}", month));
    }

    parts.join(", ")
}

fn describe_minute(minute: &str) -> String {
    if minute == "*" {
        return "every minute".to_string();
    }
    if let Some(interval) = minute.strip_prefix("*/") {
        return format!("every {} minutes", interval);
    }
    if minute == "0" {
        return "on the hour".to_string();
    }
    format!("at minute {}", minute)
}

fn describe_hour(hour: &str) -> String {
    if let Some(interval) = hour.strip_prefix("*/") {
        return format!("every {} hours", interval);
    }
    if hour.contains('-') {
        let parts: Vec<&str> = hour.split('-').collect();
        if parts.len() == 2 {
            return format!("{}–{}", format_hour(parts[0]), format_hour(parts[1]));
        }
    }
    if hour.contains(',') {
        let hours: Vec<String> = hour.split(',').map(format_hour).collect();
        return format!("at {}", hours.join(", "));
    }
    format!("at {}", format_hour(hour))
}

fn format_hour(h: &str) -> String {
    match h.parse::<u32>() {
        Ok(0) => "12am".to_string(),
        Ok(n) if n < 12 => format!("{}am", n),
        Ok(12) => "12pm".to_string(),
        Ok(n) if n < 24 => format!("{}pm", n - 12),
        _ => h.to_string(),
    }
}

fn describe_dow(dow: &str) -> String {
    let day_name = |d: &str| -> String {
        match d {
            "0" | "7" => "Sun",
            "1" => "Mon",
            "2" => "Tue",
            "3" => "Wed",
            "4" => "Thu",
            "5" => "Fri",
            "6" => "Sat",
            _ => d,
        }
        .to_string()
    };

    if dow.contains('-') {
        let parts: Vec<&str> = dow.split('-').collect();
        if parts.len() == 2 {
            return format!("{}-{}", day_name(parts[0]), day_name(parts[1]));
        }
    }
    if dow.contains(',') {
        let days: Vec<String> = dow.split(',').map(&day_name).collect();
        return days.join(", ");
    }
    day_name(dow)
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

    /// Disable cache and force a full scan of all source directories
    #[arg(long)]
    pub no_cache: bool,

    /// Preserve subdirectory structure when moving files (enables recursive scanning)
    #[arg(long)]
    pub preserve_paths: bool,

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
