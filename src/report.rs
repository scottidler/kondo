use colored::*;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Move,
    Dedup,
    Skip,
    Conflict,
    Exclude,
    Unmanaged,
    Error,
}

impl Action {
    /// Present tense label (for dry-run)
    fn present(&self) -> &'static str {
        match self {
            Action::Move => "move",
            Action::Dedup => "dedup",
            Action::Skip => "skip",
            Action::Conflict => "conflict",
            Action::Exclude => "exclude",
            Action::Unmanaged => "unmanaged",
            Action::Error => "error",
        }
    }

    /// Past tense label (for normal mode)
    fn past(&self) -> &'static str {
        match self {
            Action::Move => "moved",
            Action::Dedup => "deduped",
            Action::Skip => "skipped",
            Action::Conflict => "conflict",
            Action::Exclude => "excluded",
            Action::Unmanaged => "unmanaged",
            Action::Error => "error",
        }
    }

    fn colorize(&self, text: &str) -> ColoredString {
        match self {
            Action::Move | Action::Dedup => text.green(),
            Action::Skip => text.yellow(),
            Action::Conflict | Action::Error => text.red(),
            Action::Exclude | Action::Unmanaged => text.dimmed(),
        }
    }
}

#[derive(Debug)]
pub struct ReportEntry {
    pub action: Action,
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    pub reason: Option<String>,
}

impl fmt::Display for ReportEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source.display())?;
        if let Some(dest) = &self.destination {
            write!(f, " -> {}", dest.display())?;
        }
        if let Some(reason) = &self.reason {
            write!(f, " ({})", reason)?;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub entries: Vec<ReportEntry>,
}

impl Report {
    pub fn push(&mut self, action: Action, source: PathBuf, destination: Option<PathBuf>, reason: Option<String>) {
        self.entries.push(ReportEntry {
            action,
            source,
            destination,
            reason,
        });
    }

    pub fn count(&self, action: &Action) -> usize {
        self.entries.iter().filter(|e| &e.action == action).count()
    }

    pub fn print(&self, dry_run: bool, verbose: bool) {
        let actions = [
            Action::Unmanaged,
            Action::Exclude,
            Action::Move,
            Action::Dedup,
            Action::Skip,
            Action::Conflict,
            Action::Error,
        ];

        // Header
        if dry_run {
            println!("{}", "kondo report (dry run)".cyan().bold());
        } else {
            println!("{}", "kondo report".cyan().bold());
        }

        // Summary counts
        for action in &actions {
            let count = self.count(action);
            let label = if dry_run { action.present() } else { action.past() };
            println!("  {} {:>4} file(s)", action.colorize(&format!("{:<10}", label)), count);
        }

        // Details: grouped by action type, unmanaged hidden unless verbose
        let mut has_details = false;
        for action in &actions {
            if !verbose && *action == Action::Unmanaged {
                continue;
            }
            let entries: Vec<&ReportEntry> = self.entries.iter().filter(|e| &e.action == action).collect();
            if entries.is_empty() {
                continue;
            }
            if !has_details {
                println!();
                println!("{}:", "details".bold());
                has_details = true;
            }
            for entry in entries {
                let label = if dry_run { entry.action.present() } else { entry.action.past() };
                println!("  {} {}", entry.action.colorize(&format!("{:<10}", label)), entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_counts() {
        let mut report = Report::default();
        report.push(
            Action::Move,
            PathBuf::from("a.png"),
            Some(PathBuf::from("/dest/a.png")),
            None,
        );
        report.push(
            Action::Move,
            PathBuf::from("b.png"),
            Some(PathBuf::from("/dest/b.png")),
            None,
        );
        report.push(
            Action::Unmanaged,
            PathBuf::from("c.zip"),
            None,
            Some("no matching rule".to_string()),
        );
        report.push(
            Action::Error,
            PathBuf::from("d.png"),
            None,
            Some("permission denied".to_string()),
        );

        assert_eq!(report.count(&Action::Move), 2);
        assert_eq!(report.count(&Action::Unmanaged), 1);
        assert_eq!(report.count(&Action::Error), 1);
        assert_eq!(report.count(&Action::Dedup), 0);
        assert_eq!(report.count(&Action::Skip), 0);
        assert_eq!(report.count(&Action::Conflict), 0);
        assert_eq!(report.count(&Action::Exclude), 0);
    }

    #[test]
    fn test_report_entry_display_with_reason() {
        let entry = ReportEntry {
            action: Action::Unmanaged,
            source: PathBuf::from("/downloads/test.zip"),
            destination: None,
            reason: Some("no matching rule".to_string()),
        };
        assert_eq!(format!("{}", entry), "/downloads/test.zip (no matching rule)");
    }

    #[test]
    fn test_report_entry_display_with_dest() {
        let entry = ReportEntry {
            action: Action::Move,
            source: PathBuf::from("/downloads/photo.png"),
            destination: Some(PathBuf::from("/pictures/photo.png")),
            reason: None,
        };
        assert_eq!(format!("{}", entry), "/downloads/photo.png -> /pictures/photo.png");
    }

    #[test]
    fn test_report_entry_display_with_dest_and_reason() {
        let entry = ReportEntry {
            action: Action::Skip,
            source: PathBuf::from("/downloads/photo.png"),
            destination: Some(PathBuf::from("/pictures/photo.png")),
            reason: Some("already exists at /pictures".to_string()),
        };
        assert_eq!(
            format!("{}", entry),
            "/downloads/photo.png -> /pictures/photo.png (already exists at /pictures)"
        );
    }

    #[test]
    fn test_action_labels() {
        assert_eq!(Action::Move.present(), "move");
        assert_eq!(Action::Move.past(), "moved");
        assert_eq!(Action::Dedup.present(), "dedup");
        assert_eq!(Action::Dedup.past(), "deduped");
        assert_eq!(Action::Skip.present(), "skip");
        assert_eq!(Action::Skip.past(), "skipped");
        assert_eq!(Action::Unmanaged.present(), "unmanaged");
        assert_eq!(Action::Unmanaged.past(), "unmanaged");
    }

    #[test]
    fn test_empty_report() {
        let report = Report::default();
        assert_eq!(report.entries.len(), 0);
        assert_eq!(report.count(&Action::Move), 0);
    }
}
