use super::{events, settings};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    io,
    path::Path,
};

pub fn render_detail(home: &Path, session: Option<&str>) -> io::Result<String> {
    let mut report = format!(
        "Recording: {}\nDirectory: {}\n\n",
        if settings::enabled(home)? {
            "on"
        } else {
            "off"
        },
        home.join(".outrim").join("records").display()
    );
    let events = events(home)?;
    let mut executed = BTreeSet::new();
    let mut fallbacks = BTreeSet::new();
    for (_, event) in events.iter().filter(|(_, event)| {
        session.is_none_or(|session| event.session.as_deref() == Some(session))
    }) {
        if let Some(facts) = &event.diagnostics {
            let key = serde_json::to_string(&(
                event.host,
                &event.session,
                &event.tool_call_id,
                &facts.unit_id,
            ))?;
            if facts.unit_id.is_some() {
                match facts.execution_state.as_deref() {
                    Some("executed") => {
                        executed.insert(key);
                    }
                    Some("runtime_fallback") => {
                        fallbacks.insert(key);
                    }
                    _ => {}
                }
            }
        }
    }
    writeln!(
        report,
        "Executed commands: {}\nRuntime fallbacks: {}\n",
        executed.len(),
        fallbacks.len()
    )
    .unwrap();
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
            if let Some(path) = &event.content.arguments {
                writeln!(report, "  Arguments: {}", directory.join(path).display()).unwrap();
            }
            if let Some(facts) = &event.diagnostics {
                writeln!(report, "  Diagnostics: {}", serde_json::to_string(facts)?).unwrap();
            }
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
