//! Account-wide GitHub commit activity; independent of any local checkout.
use chrono::{Datelike, Months, NaiveDate, Utc};
use serde::Deserialize;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct Day {
    pub date: NaiveDate,
    pub commits: u64,
}
#[derive(Clone, Debug)]
pub struct Activity {
    pub login: String,
    pub days: Vec<Day>,
    pub commits: u64,
    pub pull_requests: u64,
    pub repositories: u64,
    pub longest_streak: usize,
}
impl Activity {
    pub fn busiest(&self) -> Option<&Day> {
        self.days
            .iter()
            .filter(|d| d.commits > 0)
            .max_by_key(|d| d.commits)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    total_commit_contributions: u64,
    total_pull_request_contributions: u64,
    total_repositories_with_contributed_commits: u64,
}

pub fn window(today: NaiveDate) -> Vec<NaiveDate> {
    let start = today.checked_sub_months(Months::new(4)).unwrap_or(today);
    start
        .iter_days()
        .take_while(|date| *date <= today)
        .collect()
}

// Daily collections deliberately use commit totals, not contributionCalendar,
// which also counts PRs, issues and reviews. No repository-list cap/pagination.
const QUERY_DAYS: usize = 40;

fn query(dates: &[NaiveDate], offset: usize) -> String {
    let mut q = String::from("query { viewer { login ");
    if let (Some(start), Some(end)) = (dates.first(), dates.last()) {
        let _ = write!(q, "summary: contributionsCollection(from: \"{start}T00:00:00Z\", to: \"{end}T23:59:59Z\") {{ totalCommitContributions totalPullRequestContributions totalRepositoriesWithContributedCommits }} ");
    }
    for (i, date) in dates.iter().enumerate().skip(offset).take(QUERY_DAYS) {
        let _ = write!(q, "d{i}: contributionsCollection(from: \"{date}T00:00:00Z\", to: \"{date}T23:59:59Z\") {{ totalCommitContributions }} ");
    }
    q.push_str("} }");
    q
}

pub fn parse(text: &str, dates: &[NaiveDate]) -> Result<Activity, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "Invalid GitHub activity response")?;
    if root.get("errors").is_some() {
        return Err("GitHub activity is unavailable. Check account access and retry.".into());
    }
    let viewer = &root["data"]["viewer"];
    let login = viewer["login"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("GitHub account unavailable")?
        .to_owned();
    let summary: Counts = serde_json::from_value(viewer["summary"].clone())
        .map_err(|_| "GitHub activity totals unavailable")?;
    let mut days = Vec::with_capacity(dates.len());
    let (mut streak, mut longest_streak) = (0, 0);
    for (i, date) in dates.iter().enumerate() {
        let commits = viewer[format!("d{i}")]["totalCommitContributions"]
            .as_u64()
            .ok_or("GitHub daily commit activity unavailable")?;
        streak = if commits > 0 { streak + 1 } else { 0 };
        longest_streak = longest_streak.max(streak);
        days.push(Day {
            date: *date,
            commits,
        });
    }
    if days.iter().map(|d| d.commits).sum::<u64>() != summary.total_commit_contributions {
        return Err("GitHub activity changed during loading. Retry to refresh.".into());
    }
    Ok(Activity {
        login,
        days,
        commits: summary.total_commit_contributions,
        pull_requests: summary.total_pull_request_contributions,
        repositories: summary.total_repositories_with_contributed_commits,
        longest_streak,
    })
}

pub fn load() -> Result<Activity, String> {
    let dates = window(Utc::now().date_naive());
    let mut merged = serde_json::Map::new();
    for offset in (0..dates.len()).step_by(QUERY_DAYS) {
        let q = format!("query={}", query(&dates, offset));
        let text = crate::checkout_pr::gh(
            &std::env::temp_dir(),
            &["api", "graphql", "--hostname", "github.com", "-f", &q],
        )
        .map_err(|error| {
            error
                .replace("pull requests", "account activity")
                .replace("repository access", "account access")
        })?;
        merge_batch(&mut merged, &text)?;
    }
    parse(
        &serde_json::json!({"data":{"viewer":merged}}).to_string(),
        &dates,
    )
}

fn merge_batch(
    merged: &mut serde_json::Map<String, serde_json::Value>,
    text: &str,
) -> Result<(), String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "Invalid GitHub activity response")?;
    if root.get("errors").is_some() {
        return Err("GitHub activity lookup failed. Retry to refresh.".into());
    }
    let viewer = root["data"]["viewer"]
        .as_object()
        .ok_or("GitHub account unavailable")?;
    for (key, value) in viewer {
        if merged.get(key).is_some_and(|previous| previous != value) {
            return Err("GitHub account activity changed during loading. Retry to refresh.".into());
        }
        merged.insert(key.clone(), value.clone());
    }
    Ok(())
}

pub fn weekday(date: NaiveDate) -> usize {
    date.weekday().num_days_from_sunday() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn calendar_window_clamps_month_ends_and_crosses_years() {
        let dates = window(NaiveDate::from_ymd_opt(2026, 1, 31).unwrap());
        assert_eq!(dates[0].to_string(), "2025-09-30");
        assert_eq!(dates.len(), 124);
        assert_eq!(weekday(dates[0]), 2);
        assert_eq!(
            window(NaiveDate::from_ymd_opt(2024, 6, 30).unwrap())[0].to_string(),
            "2024-02-29"
        );
    }
    fn fixture() -> (String, Vec<NaiveDate>) {
        let dates = (0..5)
            .map(|n| NaiveDate::from_ymd_opt(2026, 1, n + 1).unwrap())
            .collect();
        (json!({"data":{"viewer":{"login":"account","summary":{"totalCommitContributions":10,"totalPullRequestContributions":4,"totalRepositoriesWithContributedCommits":3},"d0":{"totalCommitContributions":1},"d1":{"totalCommitContributions":2},"d2":{"totalCommitContributions":0},"d3":{"totalCommitContributions":5},"d4":{"totalCommitContributions":2}}}}).to_string(), dates)
    }
    #[test]
    fn summaries_use_commit_days_and_separate_pr_totals() {
        let (text, dates) = fixture();
        let a = parse(&text, &dates).unwrap();
        assert_eq!(
            (a.commits, a.pull_requests, a.repositories, a.longest_streak),
            (10, 4, 3, 2)
        );
        assert_eq!(a.busiest().unwrap().date, dates[3]);
        let q = query(&dates, 0);
        assert!(!q.contains("contributionCalendar"));
        assert!(!q.contains("repository("));
    }
    #[test]
    fn partial_errors_missing_days_and_inconsistent_totals_are_rejected() {
        let (text, dates) = fixture();
        assert!(parse(&text.replace("\"d2\"", "\"missing\""), &dates).is_err());
        assert!(parse(
            &text.replace(
                "\"totalCommitContributions\":10",
                "\"totalCommitContributions\":11"
            ),
            &dates
        )
        .is_err());
        assert!(parse("{\"errors\":[],\"data\":null}", &dates).is_err());
    }
    #[test]
    fn batches_cover_four_months_and_reject_account_changes() {
        let dates = window(NaiveDate::from_ymd_opt(2026, 11, 9).unwrap());
        for offset in (0..dates.len()).step_by(QUERY_DAYS) {
            let q = query(&dates, offset);
            assert!(q.contains("2026-07-09T00:00:00Z"));
            for i in 0..dates.len() {
                assert_eq!(
                    q.contains(&format!("d{i}:")),
                    (offset..(offset + QUERY_DAYS).min(dates.len())).contains(&i)
                );
            }
        }
        let (text, _) = fixture();
        let mut merged = serde_json::Map::new();
        merge_batch(&mut merged, &text).unwrap();
        assert!(merge_batch(&mut merged, &text.replace("account", "different")).is_err());
        assert!(merge_batch(&mut merged, "{\"errors\":[{}]}").is_err());
    }

    #[test]
    #[ignore = "Requires an authenticated GitHub CLI and network"]
    fn live_account_activity() {
        let a = load().unwrap();
        assert!((121..=124).contains(&a.days.len()));
        println!(
            "Loaded {} days for @{}: {} commits, {} PRs, {} repositories",
            a.days.len(),
            a.login,
            a.commits,
            a.pull_requests,
            a.repositories
        );
    }
}
