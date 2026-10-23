use crate::{
    protocol::{Disposition, RecordDeliveryRequest, RecoveryAttribution, RecoveryRequest, VERSION},
    storage, tokens,
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
const RULE_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("log_compaction", "Log compaction"),
    ("search_hit_groups", "Search groups"),
    ("path_prefix_groups", "Path prefix groups"),
    ("json_compact", "JSON compact"),
    ("json_toon", "JSON TOON"),
];

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

    pub(crate) fn record(
        &self,
        delivery: &RecordDeliveryRequest,
        counts: tokens::Counts,
    ) -> io::Result<()> {
        if !valid_delivery(delivery) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid delivery statistics",
            ));
        }
        storage::private_directory(&self.directory)?;
        let _lock = self.lock()?;

        let mut stats = self.read()?;
        stats.record(delivery, counts);
        self.persist(&stats)
    }

    pub(crate) fn record_recovery(
        &self,
        recovery: &RecoveryRequest,
        tokens: u64,
    ) -> io::Result<()> {
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
                stats.recovery_tokens = stats.recovery_tokens.saturating_add(tokens);
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
        write_stats(file.as_file_mut(), stats)?;
        file.as_file().sync_all()?;
        storage::replace(file, &self.path)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

fn write_stats(writer: impl IoWrite, stats: &Stats) -> io::Result<()> {
    let mut buffer = io::BufWriter::new(writer);
    serde_json::to_writer(&mut buffer, stats).map_err(io::Error::other)?;
    buffer.write_all(b"\n")?;
    buffer.flush()
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stats {
    before_tokens: u64,
    after_tokens: u64,
    rules: BTreeMap<String, RuleStats>,
    outcomes: BTreeMap<Disposition, u64>,
    recovery_tokens: u64,
    recovery_events: BTreeSet<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleStats {
    count: u64,
    before_tokens: u64,
    after_tokens: u64,
}

impl Stats {
    fn record(&mut self, delivery: &RecordDeliveryRequest, counts: tokens::Counts) {
        let count = self.outcomes.entry(delivery.disposition).or_default();
        *count = count.saturating_add(1);
        self.before_tokens = self.before_tokens.saturating_add(counts.before);
        self.after_tokens = self.after_tokens.saturating_add(counts.after);

        if delivery.disposition == Disposition::Compressed {
            let rule = self
                .rules
                .entry(delivery.applied_rule.as_deref().unwrap().to_owned())
                .or_default();
            rule.count = rule.count.saturating_add(1);
            rule.before_tokens = rule.before_tokens.saturating_add(counts.before);
            rule.after_tokens = rule.after_tokens.saturating_add(counts.after);
        }
    }
}

fn valid_delivery(delivery: &RecordDeliveryRequest) -> bool {
    if delivery.version != VERSION {
        return false;
    }
    match delivery.disposition {
        Disposition::Compressed => delivery
            .applied_rule
            .as_ref()
            .is_some_and(|rule| !rule.is_empty()),
        _ => delivery.applied_rule.is_none() && delivery.final_text == delivery.original_text,
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

    let before_tokens = stats.before_tokens;
    let after_tokens = stats.after_tokens;
    let saved_tokens = i128::from(before_tokens) - i128::from(after_tokens);
    let savings_rate = percentage(saved_tokens, before_tokens);
    let outputs = stats.outcomes.values().sum();
    let mut report = String::new();

    writeln!(report, "Gain\n").unwrap();
    writeln!(report, "Processed outputs: {}", format_number(outputs)).unwrap();
    writeln!(
        report,
        "Compression savings: {} tokens",
        format_tokens(saved_tokens)
    )
    .unwrap();
    writeln!(report, "Compression rate: {savings_rate:.0}%").unwrap();
    writeln!(
        report,
        "Recovery cost: {} tokens",
        format_tokens(i128::from(stats.recovery_tokens))
    )
    .unwrap();
    let net = saved_tokens - i128::from(stats.recovery_tokens);
    writeln!(report, "Net savings: {} tokens", format_tokens(net)).unwrap();
    writeln!(
        report,
        "\nSavings: {}",
        bar(saved_tokens, i128::from(before_tokens), SAVINGS_BAR_WIDTH)
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
        saved_tokens_for(right)
            .cmp(&saved_tokens_for(left))
            .then_with(|| left_name.cmp(right_name))
    });
    if rules.is_empty() {
        writeln!(report, "No compressed outputs.").unwrap();
    }
    let largest_saving = rules.first().map_or(0, |(_, rule)| saved_tokens_for(rule));
    for (name, rule) in rules {
        let saved = saved_tokens_for(rule);
        let label = RULE_DISPLAY_NAMES
            .iter()
            .find_map(|(id, label)| (*id == name).then_some(*label))
            .unwrap_or(name);
        writeln!(
            report,
            "{label:<22} {:>7} {:>10} {:>5.0}%  {}",
            format_number(rule.count),
            format_tokens(saved),
            percentage(saved, rule.before_tokens),
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

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn serialization_propagates_buffer_flush_failure() {
        struct FailedDisk;
        impl io::Write for FailedDisk {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_stats(FailedDisk, &Stats::default())
                .unwrap_err()
                .to_string(),
            "disk full"
        );
    }
}

fn saved_tokens_for(rule: &RuleStats) -> i128 {
    i128::from(rule.before_tokens) - i128::from(rule.after_tokens)
}

fn percentage(numerator: i128, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

fn bar(value: i128, maximum: i128, width: usize) -> String {
    if maximum <= 0 || value <= 0 {
        return "░".repeat(width);
    }
    let filled = ((value as u128 * width as u128 + maximum as u128 / 2) / maximum as u128)
        .clamp(1, width as u128) as usize;
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

fn format_tokens(tokens: i128) -> String {
    if tokens < 0 {
        return format!("-{}", format_tokens(-tokens));
    }
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
