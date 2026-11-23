//! How pipeline state reads on screen: column dots, the one-line run status of a card,
//! stage markers and durations.

use gpui::{Hsla, SharedString};
use pipeline::{Issue, IssueStatus, LogKind, Run, RunStatus, StageStatus, StepKind};
use ui::Status;
use ui::theme::*;

/// Board columns (`Failed` is added only when an issue failed). `Build` is "In progress":
/// any step other than a decision runs.
pub const COLUMNS: [IssueStatus; 6] = [
    IssueStatus::Backlog,
    IssueStatus::Triage,
    IssueStatus::Build,
    IssueStatus::Approval,
    IssueStatus::Review,
    IssueStatus::Done,
];

/// The column an issue sits in (`Design` only exists in old data: in progress).
pub fn column_of(s: IssueStatus) -> IssueStatus {
    match s {
        IssueStatus::Design => IssueStatus::Build,
        s => s,
    }
}

/// "Agent", "Pull request"… for a stage's `kind` (free text of old runs as it is).
pub fn kind_label(kind: &str) -> String {
    match StepKind::parse(kind) {
        Some(k) => k.label().to_string(),
        None => kind.replace('_', " "),
    }
}

/// Issue kinds offered when creating an issue.
pub const KINDS: [&str; 3] = ["Feature", "Bug", "Chore"];

pub fn column_dot(s: IssueStatus) -> Status {
    match s {
        IssueStatus::Backlog => Status::Draft,
        IssueStatus::Triage | IssueStatus::Design | IssueStatus::Build => Status::Running,
        IssueStatus::Approval | IssueStatus::Review => Status::Waiting,
        IssueStatus::Done => Status::Live,
        IssueStatus::Failed => Status::Error,
    }
}

/// The card's status line: what the pipeline is doing with the issue right now.
#[derive(Clone, Debug)]
pub struct Line {
    pub dot: Status,
    pub text: SharedString,
    pub color: Hsla,
    /// The line is the pull request link ("PR open").
    pub pr: Option<SharedString>,
}

impl Line {
    fn new(dot: Status, text: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            dot,
            text: text.into(),
            color,
            pr: None,
        }
    }
}

pub fn status_line(issue: &Issue, run: Option<&Run>, active: bool) -> Line {
    let Some(run) = run else {
        return Line::new(Status::Draft, "Not started", TEXT_MUTED);
    };
    let pr = run.pr_url.clone().or_else(|| issue.pr_url.clone());
    match run.status {
        RunStatus::Running | RunStatus::WaitingApproval if active => {
            let stage = run
                .stages
                .iter()
                .find(|s| s.status == StageStatus::Running);
            let text = match stage {
                Some(s) => format!("{} running", s.name),
                None => "Starting".into(),
            };
            Line::new(Status::Running, text, INFO)
        }
        RunStatus::Running => Line::new(Status::Draft, "Stopped", TEXT_MUTED),
        RunStatus::WaitingApproval => {
            Line::new(Status::Waiting, "Waiting for your approval", WARNING)
        }
        RunStatus::Done => match (issue.status, pr) {
            (IssueStatus::Done, _) => Line::new(Status::Live, "Done", SUCCESS),
            (_, Some(url)) => Line {
                pr: Some(url.into()),
                ..Line::new(Status::Waiting, "PR open", ACCENT_TEXT)
            },
            _ => Line::new(Status::Live, "Finished", SUCCESS),
        },
        RunStatus::Failed => {
            let at = run
                .stages
                .iter()
                .find(|s| s.status == StageStatus::Failed)
                .map(|s| format!("Failed at {}", s.name))
                .unwrap_or_else(|| "Failed".into());
            Line::new(Status::Error, at, DANGER)
        }
        RunStatus::Cancelled => Line::new(Status::Draft, "Cancelled", TEXT_MUTED),
    }
}

/// Seconds the run has been working: until now while a worker runs, else until the last
/// stage finished.
pub fn run_elapsed(run: &Run, active: bool, now: u64) -> u64 {
    let end = if active {
        now
    } else {
        run.stages
            .iter()
            .map(|s| s.finished.max(s.started))
            .max()
            .unwrap_or(run.created)
            .max(run.created)
    };
    end.saturating_sub(run.created)
}

/// Done and skipped stages over all stages (0..=1).
pub fn run_fraction(run: &Run) -> f32 {
    let done = run
        .stages
        .iter()
        .filter(|s| matches!(s.status, StageStatus::Done | StageStatus::Skipped))
        .count();
    done as f32 / run.stages.len().max(1) as f32
}

/// `45s`, `4m 12s`, `1h 03m`.
pub fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// Short duration for cards: `45s`, `4m`, `1h 03m`.
pub fn duration_short(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// `812`, `48.2k`, `1.3M`.
pub fn tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", n as f64 / 1000.),
        _ => format!("{:.1}M", n as f64 / 1_000_000.),
    }
}

/// Word for a stage status (status colour is never the only signal).
pub fn stage_word(s: StageStatus) -> &'static str {
    match s {
        StageStatus::Pending => "Pending",
        StageStatus::Running => "Running",
        StageStatus::Waiting => "Waiting for you",
        StageStatus::Done => "Done",
        StageStatus::Skipped => "Skipped",
        StageStatus::Failed => "Failed",
    }
}

pub fn stage_color(s: StageStatus) -> Hsla {
    match s {
        StageStatus::Pending | StageStatus::Skipped => TEXT_MUTED,
        StageStatus::Running => INFO,
        StageStatus::Waiting => WARNING,
        StageStatus::Done => SUCCESS,
        StageStatus::Failed => DANGER,
    }
}

/// Colour of the `who` column of a log line.
pub fn log_who_color(k: LogKind) -> Hsla {
    match k {
        LogKind::Step => INFO_TEXT,
        LogKind::Text => TEXT,
        LogKind::Tool => ACCENT_TEXT,
        LogKind::Error => DANGER,
    }
}

pub fn log_text_color(k: LogKind) -> Hsla {
    match k {
        LogKind::Step | LogKind::Text => TEXT_SECONDARY,
        LogKind::Tool => TEXT_MUTED,
        LogKind::Error => DANGER,
    }
}

/// `HH:MM:SS` in local time.
pub fn clock(t: u64, offset: i64) -> String {
    let s = (t as i64 + offset).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration(42), "42s");
        assert_eq!(duration(252), "4m 12s");
        assert_eq!(duration(3780), "1h 03m");
        assert_eq!(duration_short(252), "4m");
        assert_eq!(tokens(48_210), "48.2k");
        assert_eq!(clock(3661, 0), "01:01:01");
    }

    #[test]
    fn lines() {
        let issue = Issue {
            key: "HAR-1".into(),
            title: "t".into(),
            body: String::new(),
            kind: "Bug".into(),
            created: 0,
            status: IssueStatus::Approval,
            pipeline: "p".into(),
            run: Some("r".into()),
            pr_url: None,
        };
        assert_eq!(
            status_line(&issue, None, false).text.as_ref(),
            "Not started"
        );
        let mut run = Run::new("HAR-1", &pipeline::config::legacy_default());
        run.status = RunStatus::WaitingApproval;
        let l = status_line(&issue, Some(&run), false);
        assert_eq!(
            (l.text.as_ref(), l.dot),
            ("Waiting for your approval", Status::Waiting)
        );
        run.status = RunStatus::Running;
        run.set_stage(0, StageStatus::Running, "");
        assert_eq!(
            status_line(&issue, Some(&run), true).text.as_ref(),
            "Triage running"
        );
        run.status = RunStatus::Failed;
        run.set_stage(0, StageStatus::Done, "");
        run.set_stage(5, StageStatus::Failed, "");
        assert_eq!(
            status_line(&issue, Some(&run), false).text.as_ref(),
            "Failed at Build"
        );
        assert_eq!(kind_label("pull_request"), "Pull request");
        assert_eq!(column_of(IssueStatus::Design), IssueStatus::Build);
    }
}
