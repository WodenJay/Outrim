use crate::{
    protocol::{Disposition, RecordDeliveryRequest, RecoveryAttribution, RecoveryRequest, VERSION},
    storage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as FmtWrite,
    fs::{self, File, OpenOptions},
    io::{self, Write as IoWrite},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub use crate::recording::{render_detail, set_recording};

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

    pub(crate) fn record_recovery(&self, recovery: &RecoveryRequest) -> io::Result<()> {
        if recovery.version != VERSION || recovery.event_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid recovery statistics",
            ));
        }
        storage::private_directory(&self.directory)?;
        let _lock = self.lock()?;
        let mut stats = self.read()?;
        if stats.recovery_events.insert(recovery.event_id.clone()) {
            if recovery.attribution == RecoveryAttribution::Confirmed {
                stats.recovery_chars = stats.recovery_chars.saturating_add(recovery.characters);
            }
            self.persist(&stats)?;
        }
        Ok(())
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
        storage::replace(file, &self.path)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stats {
    rules: BTreeMap<String, RuleStats>,
    outcomes: BTreeMap<Disposition, u64>,
    recovery_chars: u64,
    recovery_events: BTreeSet<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleStats {
    count: u64,
    before_chars: u64,
    after_chars: u64,
}

impl Stats {
    fn record(&mut self, delivery: &RecordDeliveryRequest) {
        let count = self.outcomes.entry(delivery.disposition).or_default();
        *count = count.saturating_add(1);

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
    if stats.outcomes.is_empty() && stats.recovery_events.is_empty() {
        return "No gain data.\n".into();
    }

    let before_chars: u64 = stats.rules.values().map(|rule| rule.before_chars).sum();
    let after_chars = stats.rules.values().map(|rule| rule.after_chars).sum();
    let saved_chars = before_chars.saturating_sub(after_chars);
    let savings_rate = percentage(saved_chars, before_chars);
    let outputs = stats.outcomes.values().sum();
    let mut report = String::new();

    writeln!(report, "Gain\n").unwrap();
    writeln!(report, "Processed outputs: {}", format_number(outputs)).unwrap();
    writeln!(
        report,
        "Compression savings: {} tokens",
        format_tokens(estimate_tokens(saved_chars))
    )
    .unwrap();
    writeln!(report, "Compression rate: {savings_rate:.0}%").unwrap();
    writeln!(
        report,
        "Recovery cost: {} tokens",
        format_tokens(estimate_tokens(stats.recovery_chars))
    )
    .unwrap();
    let net = i128::from(saved_chars) - i128::from(stats.recovery_chars);
    writeln!(
        report,
        "Net savings: {}{} tokens",
        if net < 0 { "-" } else { "" },
        format_tokens(estimate_tokens(net.unsigned_abs() as u64))
    )
    .unwrap();
    writeln!(
        report,
        "\nSavings: {}",
        bar(saved_chars, before_chars, SAVINGS_BAR_WIDTH)
    )
    .unwrap();

    writeln!(report, "\nBy rule").unwrap();
    writeln!(
        report,
        "{:<22} {:>7} {:>10} {:>6}  Impact",
        "Rule", "Count", "Saved", "Rate"
    )
    .unwrap();
    let mut rules: Vec<_> = stats.rules.iter().collect();
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
        let label = crate::rules::DISPLAY_NAMES
            .iter()
            .find_map(|(id, label)| (*id == name).then_some(*label))
            .unwrap_or(name);
        writeln!(
            report,
            "{label:<22} {:>7} {:>10} {:>5.0}%  {}",
            format_number(rule.count),
            format_tokens(estimate_tokens(saved)),
            percentage(saved, rule.before_chars),
            bar(saved, largest_saving, RULE_BAR_WIDTH)
        )
        .unwrap();
    }

    writeln!(report, "\nOutcomes").unwrap();
    for (disposition, label) in [
        (Disposition::Compressed, "Compressed"),
        (Disposition::Protected, "Protected"),
        (Disposition::RecoverySkipped, "Recovery skipped"),
        (Disposition::NotShorter, "No reduction"),
        (Disposition::NoMatch, "No matching rule"),
        (Disposition::FailedOpen, "Failed open"),
    ] {
        let count = stats.outcomes.get(&disposition).copied().unwrap_or(0);
        writeln!(report, "  {label}: {}", format_number(count)).unwrap();
    }
    report
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

fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    let (value, suffix) = if tokens < 999_950 {
        (format!("{:.1}", tokens as f64 / 1_000.0), "k")
    } else {
        (format!("{:.2}", tokens as f64 / 1_000_000.0), "M")
    };
    format!(
        "{}{suffix}",
        value.trim_end_matches('0').trim_end_matches('.')
    )
}
