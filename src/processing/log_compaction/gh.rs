use std::collections::BTreeMap;

struct Job<'a> {
    id: &'a str,
    heading: &'a str,
    steps: Vec<(&'a str, bool)>,
}

struct Snapshot<'a> {
    start: usize,
    end: usize,
    run: &'a str,
    header: &'a str,
    trigger: &'a str,
    jobs: Vec<Job<'a>>,
}

pub(super) fn compact(lines: &[&str], view: &[String]) -> Option<String> {
    // The only screen effects supported are the ones gh's renderer emits.
    let clean: Vec<_> = view
        .iter()
        .map(|line| screen_prefix(line))
        .collect::<Option<_>>()?;
    let mut snapshots = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(snapshot) = snapshot(lines, &clean, i) {
            i = snapshot.end;
            snapshots.push(snapshot);
        } else {
            i += 1;
        }
    }
    let latest = snapshots.last()?;
    if snapshots.iter().any(|s| s.run != latest.run) {
        return None;
    }
    let mut jobs = Vec::<(&Job<'_>, usize)>::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        for job in &snapshot.jobs {
            if let Some(existing) = jobs.iter_mut().find(|(old, _)| old.id == job.id) {
                *existing = (job, index);
            } else {
                jobs.push((job, index));
            }
        }
    }
    let mut result = format!(
        "[outrim]: Latest observed state ({} snapshots)\n",
        snapshots.len()
    );
    result.push_str(latest.header);
    newline(&mut result);
    result.push_str(latest.trigger);
    newline(&mut result);
    result.push_str("\nJOBS\n");
    for (job, index) in &jobs {
        result.push_str(job.heading);
        newline(&mut result);
        if *index + 1 != snapshots.len() {
            result.push_str(
                "  [outrim]: Last observed in an earlier snapshot; absent from latest snapshot\n",
            );
        }
        let successful = job.steps.iter().filter(|(_, success)| *success).count();
        if successful > 0 {
            result.push_str(&format!(
                "  [outrim]: {successful} successful steps omitted\n"
            ));
        }
        for (step, success) in &job.steps {
            if !success {
                result.push_str(step);
                newline(&mut result);
            }
        }
    }
    // A failure observed earlier is evidence, not necessarily the latest state.
    // Scope exact repeated observations to the same run, job and literal step.
    let mut evidence = BTreeMap::<(&str, &str), (usize, usize)>::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        for job in &snapshot.jobs {
            for (step, _) in &job.steps {
                let recognized = super::recognition(step)?;
                if screen_prefix(&recognized)?.starts_with("  X ") {
                    let entry = evidence.entry((job.id, step)).or_insert((0, index));
                    entry.0 += 1;
                }
            }
        }
    }
    for ((id, step), (count, _)) in evidence {
        let current = jobs.iter().find(|(job, _)| job.id == id)?.0;
        if !current.steps.iter().any(|(s, _)| *s == step) {
            result.push_str(&format!(
                "[outrim]: Historical evidence for job ID {id} (observed in {count} snapshots)\n"
            ));
            result.push_str(step);
            newline(&mut result);
        }
    }
    // Keep opaque annotations and watcher/custom diagnostics in observation
    // order. No guessing at multiline annotation boundaries or deduplication.
    let mut cursor = 0;
    for (index, snapshot) in snapshots.iter().enumerate() {
        let mut gap_end = snapshot.start;
        if gap_end >= cursor + 2
            && clean[gap_end - 1].is_empty()
            && clean[gap_end - 2].starts_with("Refreshing run status every ")
        {
            gap_end -= 2;
        }
        for line in &lines[cursor..gap_end] {
            result.push_str(line);
        }
        cursor = snapshot.end;
        let next = snapshots.get(index + 1).map_or(lines.len(), |s| s.start);
        if clean[cursor..next].contains(&"ANNOTATIONS") {
            result.push_str(&format!(
                "[outrim]: Evidence observed in snapshot {}\n",
                index + 1
            ));
        }
    }
    for line in &lines[cursor..] {
        result.push_str(line);
    }
    Some(result)
}

fn newline(text: &mut String) {
    if !text.ends_with('\n') {
        text.push('\n');
    }
}

fn screen_prefix(mut line: &str) -> Option<&str> {
    loop {
        let Some(rest) = line.strip_prefix('\x1b') else {
            return Some(line);
        };
        let control = [
            "[?1049h", "[?1049l", "[0;0H", "[H", "[2J", "[0J", "[J", "[?25l", "[?25h",
        ]
        .iter()
        .find(|control| rest.starts_with(**control))?;
        line = &rest[control.len()..];
    }
}

fn status(line: &str) -> bool {
    ["✓ ", "X ", "* ", "- "].iter().any(|s| line.starts_with(s))
}

fn run_id(line: &str) -> Option<&str> {
    if !status(line) {
        return None;
    }
    let (_, id) = line.rsplit_once(" · ")?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then_some(id)
}

fn job_id(line: &str) -> Option<&str> {
    if !status(line) {
        return None;
    }
    let (_, id) = line.rsplit_once(" (ID ")?;
    let id = id.strip_suffix(')')?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then_some(id)
}

fn snapshot<'a>(lines: &[&'a str], view: &[&'a str], start: usize) -> Option<Snapshot<'a>> {
    let run = run_id(view[start])?;
    if !view.get(start + 1)?.starts_with("Triggered via ")
        || !view.get(start + 2)?.is_empty()
        || *view.get(start + 3)? != "JOBS"
    {
        return None;
    }
    let mut jobs: Vec<Job<'a>> = Vec::new();
    let mut end = start + 4;
    while end < lines.len() {
        let line = view[end];
        if let Some(id) = job_id(line) {
            if jobs.iter().any(|job| job.id == id) {
                return None;
            }
            jobs.push(Job {
                id,
                heading: lines[end],
                steps: Vec::new(),
            });
        } else if line.starts_with("  ") && status(&line[2..]) && line.len() > 4 {
            jobs.last_mut()?
                .steps
                .push((lines[end], line.starts_with("  ✓ ")));
        } else if line.is_empty()
            || line.starts_with("Refreshing run status every ")
            || run_id(line).is_some()
        {
            break;
        } else {
            // A truncated or unrecognized job region cannot replace prior state.
            return None;
        }
        if !lines[end].ends_with('\n') {
            return None;
        }
        end += 1;
    }
    if jobs.is_empty() {
        return None;
    }
    Some(Snapshot {
        start,
        end,
        run,
        header: lines[start],
        trigger: lines[start + 1],
        jobs,
    })
}
