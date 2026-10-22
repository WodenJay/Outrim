use std::collections::HashSet;

struct Job<'a> {
    id: &'a str,
    heading: &'a str,
    state: &'a str,
    steps: Vec<Step<'a>>,
}

struct Step<'a> {
    text: &'a str,
    state: &'a str,
}

struct Snapshot<'a> {
    start: usize,
    end: usize,
    run: &'a str,
    state: &'a str,
    header: &'a str,
    trigger: &'a str,
    jobs: Vec<Job<'a>>,
}

pub(super) fn compact(lines: &[&str], view: &[String]) -> Option<String> {
    // Only gh's known rendering controls are accepted; never emulate a screen.
    let clean: Vec<_> = view
        .iter()
        .map(|line| screen_prefix(line))
        .collect::<Option<_>>()?;
    let mut snapshots = Vec::new();
    let mut annotations = false;
    let mut i = 0;
    while i < lines.len() {
        let refresh = i >= 2 && clean[i - 1].is_empty() && refresh_notice(clean[i - 2]);
        let screen = clean[i] != view[i];
        // An annotation can quote an entire report. A fresh renderer boundary,
        // not a header inside that opaque body, establishes another observation.
        if (!annotations || refresh || screen)
            && let Some(mut snapshot) = snapshot(lines, &clean, i)
        {
            if refresh {
                snapshot.start -= 2;
            }
            i = snapshot.end;
            snapshots.push(snapshot);
            annotations = false;
        } else {
            annotations |= clean[i] == "ANNOTATIONS";
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
    let mut result = String::new();
    let mut cursor = 0;
    let mut historical_jobs = HashSet::new();
    let mut historical_steps = HashSet::new();
    let mut historical_runs = HashSet::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        result.extend(lines[cursor..snapshot.start].iter().copied());
        if index + 1 == snapshots.len() {
            result.push_str(&format!(
                "[outrim]: Latest observed state ({} snapshots)\n",
                snapshots.len()
            ));
            result.push_str(latest.header);
            newline(&mut result);
            result.push_str(latest.trigger);
            newline(&mut result);
            result.push_str("\nJOBS\n");
            for (job, last_seen) in &jobs {
                result.push_str(job.heading);
                newline(&mut result);
                if *last_seen != index {
                    result.push_str("  [outrim]: Last observed in an earlier snapshot; absent from latest snapshot\n");
                }
                let successful = job
                    .steps
                    .iter()
                    .filter(|step| step.state.starts_with("  ✓ "))
                    .count();
                if successful > 0 {
                    result.push_str(&format!(
                        "  [outrim]: {successful} successful steps omitted\n"
                    ));
                }
                for step in &job.steps {
                    if !step.state.starts_with("  ✓ ") {
                        result.push_str(step.text);
                        newline(&mut result);
                    }
                }
            }
        } else {
            if non_success(snapshot.state)
                && snapshot.state != latest.state
                && historical_runs.insert(snapshot.state)
            {
                result.push_str(&format!(
                    "[outrim]: Historical evidence from snapshot {} (run state)\n",
                    index + 1
                ));
                result.push_str(snapshot.header);
                newline(&mut result);
            }
            for job in &snapshot.jobs {
                let current = jobs.iter().find(|(current, _)| current.id == job.id)?.0;
                let heading = non_success(job.state)
                    && job.state != current.state
                    && historical_jobs.insert((job.id, job.state));
                let steps: Vec<_> = job
                    .steps
                    .iter()
                    .filter(|step| {
                        step.state.starts_with("  X ")
                            && !current.steps.iter().any(|s| s.state == step.state)
                            && historical_steps.insert((job.id, step.state))
                    })
                    .collect();
                if heading || !steps.is_empty() {
                    result.push_str(&format!(
                        "[outrim]: Historical evidence for job ID {} (snapshot {})\n",
                        job.id,
                        index + 1
                    ));
                    result.push_str(job.heading);
                    newline(&mut result);
                    for step in steps {
                        result.push_str(step.text);
                        newline(&mut result);
                    }
                }
            }
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
    result.extend(lines[cursor..].iter().copied());
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

fn refresh_notice(line: &str) -> bool {
    line.strip_prefix("Refreshing run status every ")
        .and_then(|rest| rest.strip_suffix(" seconds. Press Ctrl+C to quit."))
        .is_some_and(|seconds| !seconds.is_empty() && seconds.chars().all(|c| c.is_ascii_digit()))
}

fn status(line: &str) -> bool {
    ["✓ ", "X ", "* ", "- "].iter().any(|s| line.starts_with(s))
}

fn non_success(line: &str) -> bool {
    line.starts_with("X ") || line.starts_with("- ")
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
                state: line,
                steps: Vec::new(),
            });
        } else if line.starts_with("  ") && status(&line[2..]) && line.len() > 4 {
            jobs.last_mut()?.steps.push(Step {
                text: lines[end],
                state: line,
            });
        } else if line.is_empty() || refresh_notice(line) {
            break;
        } else {
            return None;
        }
        if !lines[end].ends_with('\n') {
            return None;
        }
        end += 1;
    }
    // A blank line or the next explicit refresh delimits JOBS. EOF alone cannot
    // prove all jobs/steps arrived, even if the last line ended in a newline.
    if jobs.is_empty() || end == lines.len() {
        return None;
    }
    Some(Snapshot {
        start,
        end,
        run,
        state: view[start],
        header: lines[start],
        trigger: lines[start + 1],
        jobs,
    })
}
