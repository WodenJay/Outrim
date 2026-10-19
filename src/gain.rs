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
                "invalid delivery statistics",
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

pub fn render(home: &Path) -> io::Result<String> {
    Ok(render_stats(&Store::new(home).read_for_render()?))
}

fn render_stats(stats: &Stats) -> String {
    if stats.outputs == 0 {
        return "Outrim Gain · Global\nNo gain data yet.\n".into();
    }

    let before_tokens = estimate_tokens(stats.before_chars);
    let after_tokens = estimate_tokens(stats.after_chars);
    let saved_chars = stats.before_chars.saturating_sub(stats.after_chars);
    let saved_tokens = before_tokens.saturating_sub(after_tokens);
    let savings_rate = percentage(saved_chars, stats.before_chars);
    let mut report = String::new();

    writeln!(report, "Outrim Gain · Global\n").unwrap();
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
        "\nSavings: {} {savings_rate:.1}%",
        bar(saved_chars, stats.before_chars, SAVINGS_BAR_WIDTH)
    )
    .unwrap();

    writeln!(report, "\nBy Rule").unwrap();
    writeln!(
        report,
        "{:<22} {:>7} {:>10} {:>10} {:>10} {:>7}  Impact",
        "Rule", "Count", "Before", "After", "Saved", "Rate"
    )
    .unwrap();
    let mut rules: Vec<_> = stats.rules.iter().collect();
    rules.sort_by(|(left_name, left), (right_name, right)| {
        saved_chars_for(right)
            .cmp(&saved_chars_for(left))
            .then_with(|| left_name.cmp(right_name))
    });
    if rules.is_empty() {
        writeln!(report, "(no compressed results yet)").unwrap();
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
        "generic_line_fallback" => "Head + tail",
        "json_compact" => "JSON compact",
        "json_toon" => "JSON TOON",
        "search_hit_groups" => "Search groups",
        _ => rule,
    }
}
