//! Account-wide GitHub commit activity; independent of any local checkout.
use chrono::{Datelike, Local, Months, NaiveDate, TimeZone};
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

fn boundary<T: TimeZone>(date: NaiveDate, timezone: &T) -> Result<String, String> {
    // Resolve each date separately so DST changes do not shift subsequent days.
    timezone
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map(|time| time.fixed_offset().to_rfc3339())
        .ok_or_else(|| "Local midnight is unavailable for an activity date".into())
}

fn query<T: TimeZone>(dates: &[NaiveDate], offset: usize, timezone: &T) -> Result<String, String> {
    let mut q = String::from("query { viewer { login ");
    if let (Some(start), Some(end)) = (dates.first().filter(|_| offset == 0), dates.last()) {
        let start = boundary(*start, timezone)?;
        let end = boundary(end.succ_opt().unwrap(), timezone)?;
        let end = (chrono::DateTime::parse_from_rfc3339(&end).unwrap()
            - chrono::Duration::seconds(1))
        .to_rfc3339();
        let _ = write!(q, "summary: contributionsCollection(from: \"{start}\", to: \"{end}\") {{ totalCommitContributions totalPullRequestContributions totalRepositoriesWithContributedCommits }} ");
    }
    for (i, date) in dates.iter().enumerate().skip(offset).take(QUERY_DAYS) {
        let start = boundary(*date, timezone)?;
        let end = boundary(date.succ_opt().unwrap(), timezone)?;
        let end = (chrono::DateTime::parse_from_rfc3339(&end).unwrap()
            - chrono::Duration::seconds(1))
        .to_rfc3339();
        let _ = write!(q, "d{i}: contributionsCollection(from: \"{start}\", to: \"{end}\") {{ totalCommitContributions }} ");
    }
    q.push_str("} }");
    Ok(q)
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
    let dates = window(Local::now().date_naive());
    let mut merged = serde_json::Map::new();
    let responses = std::thread::scope(|scope| {
        let requests = (0..dates.len())
            .step_by(QUERY_DAYS)
            .map(|offset| query(&dates, offset, &Local))
            .collect::<Result<Vec<_>, _>>()?;
        let handles: Vec<_> = requests
            .into_iter()
            .map(|query| {
                scope.spawn(move || {
                    let q = format!("query={query}");
                    crate::checkout_pr::gh(
                        &std::env::temp_dir(),
                        &["api", "graphql", "--hostname", "github.com", "-f", &q],
                    )
                    .map_err(|error| {
                        error
                            .replace("pull requests", "account activity")
                            .replace("repository access", "account access")
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "Activity request failed".to_owned())?
            })
            .collect::<Result<Vec<_>, String>>()
    })?;
    for response in responses {
        merge_batch(&mut merged, &response)?;
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
    use chrono::Utc;
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
    fn local_boundaries_use_offsets_across_utc_dates() {
        use chrono::FixedOffset;
        let date = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let east = FixedOffset::east_opt(19800).unwrap();
        let west = FixedOffset::west_opt(28800).unwrap();
        assert_eq!(boundary(date, &east).unwrap(), "2026-11-09T00:00:00+05:30");
        assert_eq!(boundary(date, &west).unwrap(), "2026-11-09T00:00:00-08:00");
        let q = query(&[date], 0, &east).unwrap();
        assert!(q.contains("2026-11-09T23:59:59+05:30"));
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
        let q = query(&dates, 0, &Utc).unwrap();
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
            let q = query(&dates, offset, &Utc).unwrap();
            assert_eq!(q.contains("summary:"), offset == 0);
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
