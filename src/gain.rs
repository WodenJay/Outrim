use crate::{
    protocol::{Disposition, RecordDeliveryRequest, VERSION},
    storage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt::Write as FmtWrite,
    fs::{self, File, OpenOptions},
    io::{self, Write as IoWrite},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

#[cfg(windows)]
use std::{thread, time::Duration};

const STATS_FILE: &str = "gain.json";
const LOCK_FILE: &str = "gain.lock";
const SAVINGS_BAR_WIDTH: usize = 24;
const RULE_BAR_WIDTH: usize = 10;

pub(crate) struct Store {
    directory: PathBuf,
    path: PathBuf,
    lock_path: PathBuf,
}

impl Store {
    pub(crate) fn new(home: &Path) -> Self {
        let directory = home.join(".outrim");
        Self {
            path: directory.join(STATS_FILE),
            lock_path: directory.join(LOCK_FILE),
            directory,
        }
    }

    pub(crate) fn record(&self, delivery: RecordDeliveryRequest) -> io::Result<()> {
        if !valid_delivery(&delivery) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid delivery statistics",
            ));
        }
        storage::private_directory(&self.directory)?;
        let _lock = self.lock()?;

        let mut stats = self.read()?;
        stats.record(&delivery);
        self.persist(&stats)
    }

    fn lock(&self) -> io::Result<File> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)?;
        lock.lock()?;
        Ok(lock)
    }

    fn read_for_render(&self) -> io::Result<Stats> {
        let lock_exists = exists(&self.lock_path)?;
        let stats_exist = !lock_exists && exists(&self.path)?;
        if stats_exist || lock_exists {
            let _lock = self.lock()?;
            self.read()
        } else {
            Ok(Stats::default())
        }
    }

    fn read(&self) -> io::Result<Stats> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Stats::default()),
            Err(error) => Err(error),
        }
    }

    fn persist(&self, stats: &Stats) -> io::Result<()> {
        let mut file = NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(file.as_file_mut(), stats).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        #[cfg(windows)]
        persist_tempfile_with_retry(file, &self.path)?;
        #[cfg(not(windows))]
        file.persist(&self.path).map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Stats {
    outputs: u64,
    before_chars: u64,
    after_chars: u64,
    rules: BTreeMap<String, RuleStats>,
    outcomes: OutcomeCounts,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct RuleStats {
    count: u64,
    before_chars: u64,
    after_chars: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct OutcomeCounts {
    compressed: u64,
    protected: u64,
    recovery_skipped: u64,
    not_shorter: u64,
    no_match: u64,
    failed_open: u64,
}

impl Stats {
    fn record(&mut self, delivery: &RecordDeliveryRequest) {
        self.outputs = self.outputs.saturating_add(1);
        self.before_chars = self.before_chars.saturating_add(delivery.before_chars);
        self.after_chars = self.after_chars.saturating_add(delivery.after_chars);
        self.outcomes.record(delivery.disposition);

        if delivery.disposition == Disposition::Compressed {
            let rule = self
                .rules
                .entry(delivery.applied_rule.as_deref().unwrap().to_owned())
                .or_default();
            rule.count = rule.count.saturating_add(1);
            rule.before_chars = rule.before_chars.saturating_add(delivery.before_chars);
            rule.after_chars = rule.after_chars.saturating_add(delivery.after_chars);
        }
    }
}

impl OutcomeCounts {
    fn record(&mut self, disposition: Disposition) {
        let count = match disposition {
            Disposition::Compressed => &mut self.compressed,
            Disposition::Protected => &mut self.protected,
            Disposition::RecoverySkipped => &mut self.recovery_skipped,
            Disposition::NotShorter => &mut self.not_shorter,
            Disposition::NoMatch => &mut self.no_match,
            Disposition::FailedOpen => &mut self.failed_open,
        };
        *count = count.saturating_add(1);
    }
}

fn valid_delivery(delivery: &RecordDeliveryRequest) -> bool {
    if delivery.version != VERSION {
        return false;
    }
    match delivery.disposition {
        Disposition::Compressed => {
            delivery
                .applied_rule
                .as_ref()
                .is_some_and(|rule| !rule.is_empty())
                && delivery.after_chars < delivery.before_chars
        }
        _ => delivery.applied_rule.is_none() && delivery.after_chars == delivery.before_chars,
    }
}

pub fn set_recording(home: &Path, enabled: bool) -> io::Result<String> {
    let directory = home.join(".outrim");
    storage::private_directory(&directory)?;
    let mut file = NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(&mut file, &serde_json::json!({ "recording": enabled }))?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    let path = directory.join("recording.json");
    #[cfg(windows)]
    persist_tempfile_with_retry(file, &path)?;
    #[cfg(not(windows))]
    file.persist(path).map_err(|error| error.error)?;
    Ok(format!(
        "Recording {}. Restart hosts to apply.\n",
        if enabled { "on" } else { "off" }
    ))
}

pub(crate) fn recording_enabled(home: &Path) -> io::Result<bool> {
    #[derive(Deserialize)]
    struct Settings {
        recording: bool,
    }
    match fs::read(home.join(".outrim/recording.json")) {
        Ok(bytes) => Ok(serde_json::from_slice::<Settings>(&bytes)?.recording),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn render_detail(home: &Path, session: Option<&str>) -> io::Result<String> {
    let mut report = format!(
        "Recording: {}\nDirectory: {}\n\n",
        if recording_enabled(home)? {
            "on"
        } else {
            "off"
        },
        home.join(".outrim").join("records").display()
    );
    let events = crate::recording::events(home)?;
    if let Some(session) = session {
        writeln!(report, "Session: {session}").unwrap();
        let mut found = false;
        for (directory, event) in events
            .iter()
            .filter(|(_, event)| event.session.as_deref() == Some(session))
        {
            found = true;
            writeln!(
                report,
                "\n{} #{} · {} · call {}",
                event.instance_id,
                event.sequence,
                event.tool_name,
                event.tool_call_id.as_deref().unwrap_or("unknown")
            )
            .unwrap();
            writeln!(
                report,
                "  Outcome: {} · rule {} · chars {} → {}",
                serde_json::to_value(&event.outcome)?
                    .as_str()
                    .unwrap_or("unknown"),
                event.applied_rule.as_deref().unwrap_or("none"),
                event
                    .before_chars
                    .map_or_else(|| "unknown".into(), |value| value.to_string()),
                event
                    .after_chars
                    .map_or_else(|| "unknown".into(), |value| value.to_string())
            )
            .unwrap();
            if let Some(reason) = &event.reason {
                writeln!(report, "  Reason: {reason}").unwrap();
            }
            if let Some(execution) = &event.execution {
                writeln!(
                    report,
                    "  Execution: {} · {} · recording {}",
                    execution.unit_id,
                    serde_json::to_value(execution.stream)?
                        .as_str()
                        .unwrap_or("unknown"),
                    execution.recording
                )
                .unwrap();
            }
            for reference in &event.original_references {
                writeln!(
                    report,
                    "  Original reference: {} · record {}",
                    reference.original_path,
                    reference.record_id.as_deref().unwrap_or("unavailable")
                )
                .unwrap();
            }
            writeln!(
                report,
                "  Events: {}",
                directory.join("events.jsonl").display()
            )
            .unwrap();
            writeln!(
                report,
                "  Arguments: {}",
                directory.join(&event.content.arguments).display()
            )
            .unwrap();
            if let Some(path) = &event.content.original {
                writeln!(report, "  Original: {}", directory.join(path).display()).unwrap();
            }
            if let Some(path) = &event.content.r#final {
                writeln!(report, "  Final: {}", directory.join(path).display()).unwrap();
            }
        }
        if !found {
            writeln!(report, "No records for this session.").unwrap();
        }
    } else {
        let mut sessions = BTreeMap::new();
        for (directory, event) in &events {
            *sessions.entry(event.session.as_deref()).or_insert(0_u64) += 1;
            if let Some(execution) = &event.execution {
                writeln!(
                    report,
                    "Execution: {} · {} · {} · chars {} → {}\n  Events: {}",
                    execution.unit_id,
                    serde_json::to_value(execution.stream)?
                        .as_str()
                        .unwrap_or("unknown"),
                    serde_json::to_value(&event.outcome)?
                        .as_str()
                        .unwrap_or("unknown"),
                    event
                        .before_chars
                        .map_or_else(|| "unmeasured".into(), |count| count.to_string()),
                    event
                        .after_chars
                        .map_or_else(|| "unmeasured".into(), |count| count.to_string()),
                    directory.join("events.jsonl").display()
                )
                .unwrap();
            }
        }
        if sessions.is_empty() {
            writeln!(report, "No recorded sessions.").unwrap();
        }
        for (session, count) in sessions {
            writeln!(
                report,
                "Session: {} · {count} records",
                session.unwrap_or("unknown")
            )
            .unwrap();
        }
    }
    Ok(report)
}

pub fn clear(home: &Path) -> io::Result<()> {
    let store = Store::new(home);
    if !exists(&store.directory)? {
        return Ok(());
    }
    // Keep the lock file in place so all writers use the same lock.
    let _lock = store.lock()?;
    for (path, directory) in [(store.path, false), (store.directory.join("records"), true)] {
        let result = if directory {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if let Err(error) = result
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(io::Error::new(
                error.kind(),
                format!("Clear incomplete: {}: {error}", path.display()),
            ));
        }
    }
    Ok(())
}

pub fn render(home: &Path) -> io::Result<String> {
    Ok(render_stats(&Store::new(home).read_for_render()?))
}

fn render_stats(stats: &Stats) -> String {
    if stats.outputs == 0 {
        return "No gain data.\n".into();
    }

    let before_tokens = estimate_tokens(stats.before_chars);
    let after_tokens = estimate_tokens(stats.after_chars);
    let saved_chars = stats.before_chars.saturating_sub(stats.after_chars);
    let saved_tokens = before_tokens.saturating_sub(after_tokens);
    let savings_rate = percentage(saved_chars, stats.before_chars);
    let mut report = String::new();

    writeln!(report, "Gain\n").unwrap();
    writeln!(report, "Outputs: {}", format_number(stats.outputs)).unwrap();
    writeln!(report, "Tokens (est.)").unwrap();
    writeln!(report, "  Before: {}", format_number(before_tokens)).unwrap();
    writeln!(report, "  After:  {}", format_number(after_tokens)).unwrap();
    writeln!(
        report,
        "  Saved:  {} ({savings_rate:.1}%)",
        format_number(saved_tokens)
    )
    .unwrap();
    writeln!(
        report,
        "\nSavings: {}",
        bar(saved_chars, stats.before_chars, SAVINGS_BAR_WIDTH)
    )
    .unwrap();

    writeln!(report, "\nBy rule").unwrap();
    writeln!(
        report,
        "{:<22} {:>7} {:>10} {:>10} {:>10} {:>7}  Impact",
        "Rule", "Count", "Before", "After", "Saved", "Rate"
    )
    .unwrap();
    let mut rules: Vec<_> = stats
        .rules
        .iter()
        .filter(|(name, _)| name.as_str() != "generic_line_fallback")
        .collect();
    rules.sort_by(|(left_name, left), (right_name, right)| {
        saved_chars_for(right)
            .cmp(&saved_chars_for(left))
            .then_with(|| left_name.cmp(right_name))
    });
    if rules.is_empty() {
        writeln!(report, "No compressed outputs.").unwrap();
    }
    let largest_saving = rules.first().map_or(0, |(_, rule)| saved_chars_for(rule));
    for (name, rule) in rules {
        let saved = saved_chars_for(rule);
        let label = rule_label(name);
        writeln!(
            report,
            "{label:<22} {:>7} {:>10} {:>10} {:>10} {:>6.1}%  {}",
            format_number(rule.count),
            format_number(estimate_tokens(rule.before_chars)),
            format_number(estimate_tokens(rule.after_chars)),
            format_number(
                estimate_tokens(rule.before_chars)
                    .saturating_sub(estimate_tokens(rule.after_chars)),
            ),
            percentage(saved, rule.before_chars),
            bar(saved, largest_saving, RULE_BAR_WIDTH)
        )
        .unwrap();
    }

    writeln!(report, "\nOutcomes").unwrap();
    for (label, count) in [
        ("Compressed", stats.outcomes.compressed),
        ("Protected", stats.outcomes.protected),
        ("Recovery skipped", stats.outcomes.recovery_skipped),
        ("No reduction", stats.outcomes.not_shorter),
        ("No matching rule", stats.outcomes.no_match),
        ("Failed open", stats.outcomes.failed_open),
    ] {
        writeln!(report, "  {label}: {}", format_number(count)).unwrap();
    }
    report
}

#[cfg(windows)]
fn persist_tempfile_with_retry(mut file: NamedTempFile, path: &Path) -> io::Result<()> {
    // Windows can deny replacement while a reader holds gain.json open; retry for 195ms.
    const RETRY_DELAYS_MS: [u64; 7] = [5, 10, 20, 40, 40, 40, 40];

    for delay_ms in RETRY_DELAYS_MS {
        match file.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) if error.error.kind() == io::ErrorKind::PermissionDenied => {
                file = error.file;
                thread::sleep(Duration::from_millis(delay_ms));
            }
            Err(error) => return Err(error.error),
        }
    }

    file.persist(path).map(|_| ()).map_err(|error| error.error)
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn saved_chars_for(rule: &RuleStats) -> u64 {
    rule.before_chars.saturating_sub(rule.after_chars)
}

fn estimate_tokens(chars: u64) -> u64 {
    chars / 4 + u64::from(chars % 4 >= 2)
}

fn percentage(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

fn bar(value: u64, maximum: u64, width: usize) -> String {
    if maximum == 0 {
        return "░".repeat(width);
    }
    let filled = if value == 0 {
        0
    } else {
        ((u128::from(value) * width as u128 + u128::from(maximum) / 2) / u128::from(maximum))
            .clamp(1, width as u128) as usize
    };
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn format_number(number: u64) -> String {
    let digits = number.to_string();
    let first_group = digits.len() % 3;
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    if first_group != 0 {
        formatted.push_str(&digits[..first_group]);
    }
    for group in digits.as_bytes()[first_group..].chunks(3) {
        if !formatted.is_empty() {
            formatted.push(',');
        }
        formatted.push_str(std::str::from_utf8(group).unwrap());
    }
    formatted
}

fn rule_label(rule: &str) -> &str {
    match rule {
        "log_tail" => "Log tail",
        "json_compact" => "JSON compact",
        "json_toon" => "JSON TOON",
        "search_hit_groups" => "Search groups",
        _ => rule,
    }
}
