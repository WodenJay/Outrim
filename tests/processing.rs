use outrim::{Processor, protocol::*};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn request(text: &str, source: Source) -> Request {
    Request {
        version: VERSION,
        id: "request-1".into(),
        host: Host::Pi,
        session: "session-1".into(),
        tool: Tool {
            name: "bash".into(),
            input: json!({"command": "cargo test"}),
            source,
            exit_code: Some(0),
        },
        text: text.into(),
    }
}

fn log() -> String {
    format!(
        "{}\n01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n",
        "old output ".repeat(200)
    )
}

#[test]
fn path_lists_group_contiguous_prefixes_and_save_the_original() {
    let home = tempfile::tempdir().unwrap();
    let prefix = format!("workspace/{}/", "long-directory/".repeat(24));
    let text = format!("{prefix}client.rs\n{prefix}server.rs\n{prefix}server.rs\nother.txt\n");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("path_prefix_groups"));
    let original_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "{prefix}\n  client.rs\n  server.rs\n  server.rs\nother.txt\n[outrim]: The full output is available at {original_path}"
        )
    );
}

#[test]
fn path_lists_reject_the_whole_list_when_any_record_is_unsupported() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let prefix = format!("workspace/{}/", "long-directory/".repeat(24));
    for invalid in [
        "bare",
        "space name.rs",
        "tab\tname.rs",
        "bad\0name.rs",
        "bad\rname.rs",
        "src/a:b.rs",
        "1:src/a.rs",
        "C:src/a:b.rs",
        "unknown notice",
        "42 files found",
        "\nother.rs",
        "\r\nother.rs",
        "src/a.rs:1:hit",
        "[12 matches limit reached.]",
    ] {
        let text = format!("{prefix}a.rs\n{prefix}b.rs\n{invalid}\n");
        let response = processor.process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::NoMatch, "{invalid:?}");
        assert_eq!(response.text, text);
        assert!(response.original_path.is_none());
        assert!(response.candidates.is_empty());
    }
    for suffix in ["-1-context", "-1-context-2-more"] {
        let text = format!("{prefix}a.rs{suffix}\n{prefix}b.rs{suffix}\n");
        let response = processor.process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::NoMatch, "{suffix}");
        assert_eq!(response.text, text);
        assert!(response.candidates.is_empty());
    }
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn path_lists_keep_greedy_blocks_order_and_explicit_directory_entries() {
    let home = tempfile::tempdir().unwrap();
    let prefix = format!("{}/", "long-directory".repeat(24));
    let text = format!(
        "{prefix}\n{prefix}deep/a.rs\n{prefix}deep/b.rs\n{prefix}other.rs\nelsewhere.txt\n{prefix}deep/a.rs\n{prefix}deep/a.rs\n{prefix}child/\na/one\na/{prefix}b\na/{prefix}c\n"
    );
    let response = Processor::new(home.path()).process(request(&text, Source::Search));
    assert_eq!(response.applied_rule.as_deref(), Some("path_prefix_groups"));
    let original_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "{prefix}\n{prefix}deep/\n  a.rs\n  b.rs\n{prefix}other.rs\nelsewhere.txt\n{prefix}deep/\n  a.rs\n  a.rs\n{prefix}child/\na/one\na/{prefix}\n  b\n  c\n[outrim]: The full output is available at {original_path}"
        )
    );
}

#[test]
fn path_lists_preserve_literal_paths_and_each_record_ending() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    for prefix in [
        format!("Src/{}/.././dir//", "long-directory/".repeat(24)),
        format!(r"C:\Work\{}\.\..\\", "long-directory\\".repeat(24)),
        format!(r"C:Work\{}/Mixed\\", "long-directory/".repeat(24)),
    ] {
        for (first, second, tail) in [
            ("\n", "\n", ""),
            ("\r\n", "\r\n", "\r\n\r\n"),
            ("\r\n", "\n", "\n\r\n"),
            ("\n", "", ""),
        ] {
            let text = format!("{prefix}a.rs{first}{prefix}B/{second}{tail}");
            let response = processor.process(request(&text, Source::Search));
            assert_eq!(response.applied_rule.as_deref(), Some("path_prefix_groups"));
            let original_path = response.original_path.unwrap();
            assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
            let separator = if second.is_empty() { "\n" } else { "" };
            assert_eq!(
                response.text,
                format!(
                    "{prefix}{first}  a.rs{first}  B/{second}{tail}{separator}[outrim]: The full output is available at {original_path}"
                )
            );
        }
    }
}

#[test]
fn path_lists_do_not_group_similar_names_or_locally_unprofitable_blocks() {
    let home = tempfile::tempdir().unwrap();
    let prefix = format!("{}/", "long-directory".repeat(24));
    let unchanged = format!(
        "a/one\na/two\n🦀🦀🦀🦀/a\n🦀🦀🦀🦀/b\n{prefix}first.rs\nelsewhere.txt\n{prefix}second.rs\n{}a.rs\n{}b.rs\n",
        "similar-name".repeat(24),
        "similar-name".repeat(24)
    );
    let text = format!("{unchanged}{prefix}a.rs\n{prefix}b.rs\n");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));
    let original_path = response.original_path.unwrap();
    assert_eq!(
        response.text,
        format!(
            "{unchanged}{prefix}\n  a.rs\n  b.rs\n[outrim]: The full output is available at {original_path}"
        )
    );
}

#[test]
fn path_lists_require_strict_unicode_savings_after_the_recovery_hint() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let probe_text = format!("{0}/a.rs\n{0}/b.rs", "long-directory".repeat(40));
    let probe = processor.process(request(&probe_text, Source::Search));
    let hint_length = format!(
        "\n[outrim]: The full output is available at {}",
        probe.original_path.unwrap()
    )
    .chars()
    .count();
    // Two paths save prefix length minus the header newline and four spaces.
    for (prefix_length, expected) in [
        (hint_length + 4, Disposition::NotShorter),
        (hint_length + 5, Disposition::NotShorter),
        (hint_length + 6, Disposition::Compressed),
    ] {
        let prefix = format!("{}/", "🦀".repeat(prefix_length - 1));
        let text = format!("{prefix}a.rs\n{prefix}b.rs");
        let response = processor.process(request(&text, Source::Search));
        assert_eq!(response.disposition, expected);
        assert_eq!(response.candidates[0].rule, "path_prefix_groups");
        assert_eq!(
            response.candidates[0].final_chars.unwrap() as usize,
            text.chars().count() + hint_length + 5 - prefix_length
        );
        if expected == Disposition::NotShorter {
            assert_eq!(response.text, text);
            assert!(response.applied_rule.is_none());
            assert!(response.original_path.is_none());
        } else {
            assert_eq!(response.text.chars().count() + 1, text.chars().count());
        }
    }
    let text = "src/network/a.rs\nsrc/network/b.rs\n";
    let response = processor.process(request(text, Source::Search));
    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, text);
    assert!(response.original_path.is_none());
    let text = "a/one\na/two\n";
    let response = processor.process(request(text, Source::Search));
    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
}

#[test]
fn path_lists_respect_source_protection_and_storage_failure() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let prefix = format!("{}/", "long-directory".repeat(40));
    let text = format!("{prefix}a.rs\n{prefix}b.rs\n");
    for source in [
        Source::Unknown,
        Source::SourceCode,
        Source::Configuration,
        Source::BuildLog,
        Source::TestLog,
        Source::RuntimeLog,
    ] {
        let response = processor.process(request(&text, source));
        assert_eq!(response.text, text);
        assert!(
            response
                .candidates
                .iter()
                .all(|candidate| candidate.rule != "path_prefix_groups")
        );
        assert!(response.original_path.is_none());
    }
    for key in ["path", "file_path", "filePath"] {
        for path in ["src/main.rs", "config/settings.json"] {
            let mut input = request(&text, Source::Search);
            input.tool.input = json!({key: path});
            let response = processor.process(input);
            assert_eq!(response.disposition, Disposition::Protected);
            assert_eq!(response.text, text);
            assert!(response.candidates.is_empty());
        }
    }
    fs::write(home.path().join(".outrim"), "blocked").unwrap();
    let response = processor.process(request(&text, Source::Search));
    assert_eq!(response.disposition, Disposition::FailedOpen);
    assert_eq!(response.text, text);
    assert!(response.original_path.is_none());
    assert!(response.applied_rule.is_none());
    assert_eq!(response.candidates[0].reason, "storage_failed");
}

#[test]
fn source_and_configuration_are_protected_before_content_routing() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let long_text = (1..=24)
        .map(|line| format!("content {line}: {}\n", "🦀".repeat(30)))
        .collect::<String>();
    let records: Vec<_> = (0..80)
        .map(|id| json!({"id": id, "kind": "event", "state": "complete"}))
        .collect();
    let json_text = serde_json::to_string_pretty(&json!({"records": records})).unwrap();

    for source in [Source::SourceCode, Source::Configuration] {
        for text in [&long_text, &json_text] {
            let input = request(text, source);
            let response = processor.process(input.clone());
            assert_eq!(response.disposition, Disposition::Protected);
            assert_eq!(response.text, input.text);
            assert!(response.applied_rule.is_none());
            assert!(response.original_path.is_none());
        }
    }

    let build_log = log();
    for (text, source, path) in [
        (&long_text, Source::Unknown, "src/main.rs"),
        (&json_text, Source::Unknown, "config/settings.json"),
        (&json_text, Source::Search, "config/settings.json"),
        (&json_text, Source::Search, "src/main.rs"),
        (&build_log, Source::BuildLog, "src/main.rs"),
    ] {
        let mut input = request(text, source);
        input.tool.input = json!({"path": path});
        let response = processor.process(input.clone());
        assert_eq!(response.disposition, Disposition::Protected);
        assert_eq!(response.text, input.text);
    }
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn opaque_sessions_have_bounded_safe_directories() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let sessions = [String::new(), "长🦀/../".repeat(1000), "normal".into()];
    let mut directories = std::collections::HashSet::new();
    for session in sessions {
        let mut input = request(&log(), Source::TestLog);
        input.session = session;
        let first = processor.process(input.clone()).original_path.unwrap();
        let second = processor.process(input).original_path.unwrap();
        let directory = Path::new(&first).parent().unwrap();
        assert_eq!(directory, Path::new(&second).parent().unwrap());
        assert!(directory.file_name().unwrap().len() <= 64);
        assert!(directory.starts_with(home.path().join(".outrim/outputs/pi")));
        assert_eq!(fs::read_to_string(&first).unwrap(), log());
        assert_eq!(fs::read_to_string(&second).unwrap(), log());
        assert_ne!(first, second);
        directories.insert(directory.to_path_buf());
    }
    assert_eq!(directories.len(), 3);
}

#[test]
fn unsupported_protocol_version_fails_open() {
    let home = tempfile::tempdir().unwrap();
    let mut input = request(&log(), Source::TestLog);
    input.version = VERSION + 1;
    let response = Processor::new(home.path()).process(input.clone());
    assert_eq!(response.disposition, Disposition::FailedOpen);
    assert_eq!(response.text, input.text);
    assert!(response.original_path.is_none());
}

#[test]
fn final_hint_counts_and_unicode_codepoints_must_be_strictly_shorter() {
    let home = tempfile::Builder::new()
        .prefix("outrim-恢复-🦀-")
        .tempdir()
        .unwrap();
    let processor = Processor::new(home.path());
    let probe = processor.process(request(&log(), Source::TestLog));
    let tail = "01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n";
    // The saved path has fixed length within this host/session. Place original
    // lengths one codepoint below, equal to, and above the observed final text.
    let boundary = probe.text.chars().count() - tail.chars().count() - 1;
    for (count, expected) in [
        (boundary - 1, Disposition::NotShorter),
        (boundary, Disposition::NotShorter),
        (boundary + 1, Disposition::Compressed),
    ] {
        let text = format!("{}\n{tail}", "🦀".repeat(count));
        let response = processor.process(request(&text, Source::RuntimeLog));
        assert_eq!(response.disposition, expected);
        assert_eq!(
            response.candidates[0].final_chars,
            Some(probe.text.chars().count() as u64)
        );
        if expected == Disposition::NotShorter {
            assert_eq!(response.candidates[0].reason, "not_shorter_with_hint");
            assert_eq!(response.text, text);
            assert!(response.original_path.is_none());
            assert!(response.applied_rule.is_none());
        } else {
            assert_eq!(response.candidates[0].reason, "selected");
            assert_eq!(response.text.chars().count() + 1, text.chars().count());
        }
    }
}

#[test]
fn short_logs_and_unmatched_content_pass_through() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    for text in ["", "one line", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n"] {
        let response = processor.process(request(text, Source::BuildLog));
        assert_eq!(response.disposition, Disposition::NotShorter);
        assert_eq!(response.text, text);
        assert_eq!(response.candidates[0].reason, "not_shorter");
        assert_eq!(response.candidates[0].final_chars, None);
    }
    for source in [Source::Search, Source::Unknown] {
        let response = processor.process(request(&log(), source));
        assert_eq!(response.disposition, Disposition::NoMatch);
        assert_eq!(response.text, log());
    }
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn read_output_remains_eligible_for_json_compression() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let mut items: Vec<Value> = (0..80).map(|id| json!({"id": id})).collect();
    items.push(json!([true, null, {"nested": "value"}]));
    let value = json!({"items": items});
    let text = serde_json::to_string_pretty(&value).unwrap();

    for host in [Host::Pi, Host::Opencode] {
        let mut input = request(&text, Source::Unknown);
        input.host = host;
        input.tool.name = "read".into();
        input.tool.input = json!({"path": "response.txt"});
        let response = processor.process(input);
        assert_eq!(response.disposition, Disposition::Compressed);
        assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
        let (encoded, _) = response.text.split_once("\n[outrim]:").unwrap();
        assert_eq!(serde_json::from_str::<Value>(encoded).unwrap(), value);
    }
}

#[test]
fn saving_failure_returns_original_without_internal_error_text() {
    let home = tempfile::tempdir().unwrap();
    fs::write(home.path().join(".outrim"), "blocked").unwrap();
    let response = Processor::new(home.path()).process(request(&log(), Source::TestLog));
    assert_eq!(response.disposition, Disposition::FailedOpen);
    assert_eq!(response.text, log());
    assert!(response.original_path.is_none());
    assert!(response.applied_rule.is_none());
    assert_eq!(response.candidates[0].reason, "storage_failed");
    assert_eq!(response.candidates[0].final_chars, None);
}

#[cfg(unix)]
#[test]
fn originals_and_their_directories_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let response = Processor::new(home.path()).process(request(&log(), Source::TestLog));
    let path = response.original_path.unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let mut directory = Path::new(&path).parent().unwrap();
    while directory != home.path() {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        directory = directory.parent().unwrap();
    }
}

#[cfg(windows)]
#[test]
fn originals_and_their_directories_grant_access_only_to_the_owner() {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
                SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION,
        },
    };
    let home = tempfile::tempdir().unwrap();
    let response = Processor::new(home.path()).process(request(&log(), Source::TestLog));
    let path = response.original_path.unwrap();
    let mut entry = Path::new(&path);
    while entry != home.path() {
        let wide: Vec<_> = entry.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        let mut sddl = ptr::null_mut();
        // SAFETY: all inputs are valid NUL-terminated paths or live output slots;
        // the two Windows-allocated buffers are released before the next loop.
        unsafe {
            assert_eq!(
                GetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut descriptor
                ),
                0
            );
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut sddl,
                    ptr::null_mut()
                ),
                0
            );
            let mut length = 0;
            while *sddl.add(length) != 0 {
                length += 1;
            }
            let acl = String::from_utf16(std::slice::from_raw_parts(sddl, length)).unwrap();
            LocalFree(sddl.cast());
            LocalFree(descriptor);
            assert_eq!(acl.matches('(').count(), 1, "{entry:?}: {acl}");
            assert!(acl.ends_with(";FA;;;OW)"), "{entry:?}: {acl}");
        }
        entry = entry.parent().unwrap();
    }
}

#[test]
fn logs_keep_last_ten_lines_and_recover_exact_input() {
    let home = tempfile::tempdir().unwrap();
    let numbered_lines = (1..=23)
        .map(|line| format!("{line:02}\n"))
        .collect::<String>();
    let text = format!("{}\n{numbered_lines}", "old output ".repeat(200));
    let response = Processor::new(home.path()).process(request(&text, Source::TestLog));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("log_tail"));
    let path = response.original_path.unwrap();
    assert!(Path::new(&path).is_absolute());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "14\n15\n16\n17\n18\n19\n20\n21\n22\n23\n[outrim]: The full output is available at {path}"
        )
    );
}

#[test]
fn search_without_hit_groups_can_use_existing_json_rules() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let mut items: Vec<_> = (0..80).map(|id| json!({"id": id})).collect();
    items.push(json!([true, null, {"nested": "value"}]));
    let value = json!({"items": items});
    let text = serde_json::to_string_pretty(&value).unwrap();
    let mut input = request(&text, Source::Search);
    input.tool.input = json!({"command": "rg --json hit ."});
    let response = processor.process(input);

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
    let (encoded, _) = response.text.split_once("\n[outrim]:").unwrap();
    assert_eq!(serde_json::from_str::<Value>(encoded).unwrap(), value);
    assert!(response.text.chars().count() < text.chars().count());
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        text
    );
}

#[test]
fn mixed_json_uses_a_lossless_compact_candidate_when_final_text_is_shorter() {
    let home = tempfile::tempdir().unwrap();
    let mut items: Vec<Value> = (0..80)
        .map(|index| json!({"name": format!("item,{index}\nwith\\\"quotes"), "id": index}))
        .collect();
    items.extend([json!([true, null, {"nested": "value"}]), json!("tail")]);
    let value = json!({"items": items});
    let text = serde_json::to_string_pretty(&value).unwrap();

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
    let path = response.original_path.unwrap();
    assert!(Path::new(&path).is_absolute());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    let (encoded, hint) = response
        .text
        .split_once("\n[outrim]: The full output is available at ")
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(encoded).unwrap(), value);
    assert_eq!(hint, path);
}

#[test]
fn equal_final_cost_keeps_the_first_eligible_candidate() {
    let home = tempfile::tempdir().unwrap();
    let text = format!("{}1", " ".repeat(1000));
    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
    assert_eq!(response.candidates.len(), 2);
    assert_eq!(response.candidates[0].rule, "json_compact");
    assert_eq!(response.candidates[0].reason, "selected");
    assert_eq!(response.candidates[1].rule, "json_toon");
    assert_eq!(
        response.candidates[1].reason,
        "smaller_or_equal_final_selected"
    );
    for candidate in &response.candidates {
        assert_eq!(
            candidate.final_chars,
            Some(response.text.chars().count() as u64)
        );
    }
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        text
    );
}

#[test]
fn repeated_json_records_choose_the_smaller_lossless_toon_candidate() {
    let home = tempfile::tempdir().unwrap();
    // Repetition makes the table representation beat the fixed path hint.
    let records: Vec<_> = (0..80)
        .map(|index| {
            let id = if index % 2 == 0 {
                9_007_199_254_740_991i64
            } else {
                -9_007_199_254_740_991i64
            };
            json!({
                "id": id,
                "enabled": id % 2 == 0,
                "kind": "event",
                "nullable": null,
                "source": "worker",
                "state": "complete",
                "special": if id % 2 == 0 { "true" } else { "line,\nbreak" }
            })
        })
        .collect();
    let value = json!({"records": records});
    let text = serde_json::to_string_pretty(&value).unwrap();

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_toon"));
    assert_eq!(
        response.candidates[0].reason,
        "smaller_or_equal_final_selected"
    );
    assert_eq!(response.candidates[1].reason, "selected");
    assert!(response.candidates[0].final_chars > response.candidates[1].final_chars);
    assert_eq!(
        response.candidates[1].final_chars,
        Some(response.text.chars().count() as u64)
    );
    let path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    let (encoded, hint) = response
        .text
        .split_once("\n[outrim]: The full output is available at ")
        .unwrap();
    assert_eq!(
        toon_format::decode_default::<Value>(encoded).unwrap(),
        value
    );
    assert_eq!(hint, path);
}

#[test]
fn numeric_and_escaped_string_boundaries_fall_back_to_exact_compact_json() {
    let home = tempfile::tempdir().unwrap();
    let row = r#"{ "lower": 9007199254740991, "upper": 9007199254740992, "overflow": 18446744073709551616, "decimal": 0.1000000000000000055511151231257827, "negative_zero": -0, "escaped": "line\\n\\t\\\"quoted\\\"\\\\path/🦀" }"#;
    let compact_row = r#"{"lower":9007199254740991,"upper":9007199254740992,"overflow":18446744073709551616,"decimal":0.1000000000000000055511151231257827,"negative_zero":-0,"escaped":"line\\n\\t\\\"quoted\\\"\\\\path/🦀"}"#;
    // Repeat enough rows for whitespace savings to beat the fixed path hint.
    let rows = vec![row; 40].join(",\n    ");
    let text = format!("{{\n  \"records\": [\n    {rows}\n  ]\n}}");
    let expected = format!("{{\"records\":[{}]}}", vec![compact_row; 40].join(","));

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
    let original_path = response.original_path.as_ref().unwrap();
    assert_eq!(fs::read_to_string(original_path).unwrap(), text);
    let (encoded, _) = response
        .text
        .split_once("\n[outrim]: The full output is available at ")
        .unwrap();
    assert_eq!(encoded, expected);
}

#[test]
fn duplicate_json_keys_are_preserved_by_compact_fallback() {
    let home = tempfile::tempdir().unwrap();
    let row = r#"{ "id": 1, "id": 2, "kind": "same" }"#;
    // Repeat enough rows for whitespace savings to beat the fixed path hint.
    let rows = vec![row; 80].join(",\n    ");
    let text = format!("{{\n  \"records\": [\n    {rows}\n  ]\n}}");

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
    let original_path = response.original_path.as_ref().unwrap();
    assert_eq!(fs::read_to_string(original_path).unwrap(), text);
    let (encoded, _) = response
        .text
        .split_once("\n[outrim]: The full output is available at ")
        .unwrap();
    assert_eq!(
        encoded,
        format!(
            "{{\"records\":[{}]}}",
            vec![r#"{"id":1,"id":2,"kind":"same"}"#; 80].join(",")
        )
    );
}

#[test]
fn json_with_no_final_savings_and_unrepresentable_numbers_passes_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let text = " { \"state\": \"🦀\" } ";
    let response = processor.process(request(text, Source::Unknown));
    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());

    let out_of_range = "{ \"number\": 1e400 }";
    let response = processor.process(request(out_of_range, Source::Unknown));
    assert_eq!(response.text, out_of_range);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn parser_rejected_multiline_json_passes_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let row = format!("{{\"number\":1e400,\"payload\":\"{}\"}}", "x".repeat(100));
    let rows = vec![row; 24].join(",\n    ");
    let text = format!("{{\n  \"records\": [\n    {rows}\n  ]\n}}");

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn search_candidate_that_loses_ingress_savings_to_hint_keeps_original_output() {
    let home = tempfile::tempdir().unwrap();
    let path = "short.rs";
    let text = format!("{path}:1:hit\n{path}:2:hit\n{path}:3:hit\n{path}:4:hit\n{path}:5:hit\n");
    let grouped = "short.rs:\n  1:hit\n  2:hit\n  3:hit\n  4:hit\n  5:hit\n";
    assert!(grouped.chars().count() < text.chars().count());

    let response = Processor::new(home.path()).process(request(&text, Source::Search));
    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn search_candidate_without_ingress_savings_keeps_original_output() {
    let home = tempfile::tempdir().unwrap();
    let content = "x".repeat(64);
    let text = format!("a.rs:1:{content}\na.rs:2:{content}\n");
    let grouped = format!("a.rs:\n  1:{content}\n  2:{content}\n");
    // The header and indentation exactly offset the repeated-path savings.
    assert_eq!(grouped.chars().count(), text.chars().count());

    let response = Processor::new(home.path()).process(request(&text, Source::Search));
    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn unstructured_search_text_for_a_source_path_remains_protected() {
    let home = tempfile::tempdir().unwrap();
    let text = (1..=24)
        .map(|line| {
            format!(
                "fn line_{line}() {{ let text = \"{}\"; }}\n",
                "x".repeat(100)
            )
        })
        .collect::<String>();
    let mut input = request(&text, Source::Search);
    input.tool.input = json!({"path": "src/main.rs"});
    let response = Processor::new(home.path()).process(input);

    assert_eq!(response.disposition, Disposition::Protected);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn context_search_groups_blocks_without_losing_records_or_limit_notice() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/client.rs", "network-component/".repeat(8));
    let text = format!(
        "{file}-41- const retries = 3;\n{file}:42: const timeout = 5000;\n{file}-43- connect(retries, timeout);\n--\n{file}-88- const config = {{\n{file}:89:   timeout: 10000,\n{file}-90- }};\n[100 matches limit reached.]\n"
    );
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("search_hit_groups"));
    let original_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "{file}:\n  41- const retries = 3;\n  42: const timeout = 5000;\n  43- connect(retries, timeout);\n  --\n  88- const config = {{\n  89:   timeout: 10000,\n  90- }};\n[100 matches limit reached.]\n[outrim]: The full output is available at {original_path}"
        )
    );
}

#[test]
fn search_output_with_mixed_context_delimiters_is_left_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let path = "workspace/deeply/nested/source-file.rs";
    let text =
        format!("{path}:12:match\n{path}-13:context\n--\n{path}:14:match\n{path}:15:match\n");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn extended_search_keeps_interleaved_paths_and_cross_file_separators_in_order() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/a.rs", "long-component/".repeat(12));
    // Each new record type independently opts into order-preserving grouping.
    for (middle, expected_middle, separator, ending) in [
        (format!("{file}-3-context\n"), "  3-context\n", "", ""),
        (String::new(), "", "", "[0007 matches limit reached.]\n"),
        ("--\n".into(), "--\n", "--\n", ""),
    ] {
        let text = format!(
            "{file}:9:first\n{file}:9:duplicate\n{middle}other.rs:2:other\n{separator}{file}:1:last\n{file}:1:last\n{ending}"
        );
        let response = Processor::new(home.path()).process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::Compressed);
        let original_path = response.original_path.unwrap();
        assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
        assert_eq!(
            response.text,
            format!(
                "{file}:\n  9:first\n  9:duplicate\n{expected_middle}other.rs:\n  2:other\n{separator}{file}:\n  1:last\n  1:last\n{ending}[outrim]: The full output is available at {original_path}"
            )
        );
    }
}

#[test]
fn context_search_preserves_positions_body_and_windows_paths() {
    let home = tempfile::tempdir().unwrap();
    let file = format!(
        r"C:\workspace\{}\module-name.rs",
        "long-directory\\".repeat(8)
    );
    let text = format!(
        "{file}-0009-\n{file}:0008:03: body:7:value -8- -- [12 matches limit reached.]\n{file}-9- body:8:value -9- -- [13 matches limit reached.]\n{file}:8\n{file}:8:\n"
    );
    let response = Processor::new(home.path()).process(request(&text, Source::Search));
    assert_eq!(response.disposition, Disposition::Compressed);
    let original_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "{file}:\n  0009-\n  0008:03: body:7:value -8- -- [12 matches limit reached.]\n  9- body:8:value -9- -- [13 matches limit reached.]\n  8\n  8:\n[outrim]: The full output is available at {original_path}"
        )
    );
}

#[test]
fn context_search_preserves_record_endings_and_terminal_blank_lines() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/module.rs", "long-component/".repeat(12));
    for (first_ending, second_ending, tail) in [
        ("\n", "\n", ""),
        ("\r\n", "\r\n", "\r\n\r\n"),
        ("\r\n", "\n", "\n\r\n"),
        ("\n", "", ""),
    ] {
        let text = format!(
            "{file}:18446744073709551615:hit{first_ending}{file}-0001-context{second_ending}{tail}"
        );
        let response = Processor::new(home.path()).process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::Compressed);
        let original_path = response.original_path.unwrap();
        assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
        let separator = if second_ending.is_empty() { "\n" } else { "" };
        assert_eq!(
            response.text,
            format!(
                "{file}:{first_ending}  18446744073709551615:hit{first_ending}  0001-context{second_ending}{tail}{separator}[outrim]: The full output is available at {original_path}"
            )
        );
    }
    // A final notice also keeps its terminator and any following blank records.
    for ending in ["", "\n", "\r\n\r\n"] {
        let text = format!(
            "{file}:1:hit\n{file}:2:hit\n[999999999999999999999999 matches limit reached.]{ending}"
        );
        let response = Processor::new(home.path()).process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::Compressed);
        let original_path = response.original_path.unwrap();
        assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
        let separator = if ending.is_empty() { "\n" } else { "" };
        assert_eq!(
            response.text,
            format!(
                "{file}:\n  1:hit\n  2:hit\n[999999999999999999999999 matches limit reached.]{ending}{separator}[outrim]: The full output is available at {original_path}"
            )
        );
    }
}

#[test]
fn malformed_context_search_results_are_not_partially_grouped() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/module.rs", "long-component/".repeat(10));
    let valid = format!("{file}:1:hit\n{file}-2-context\n{file}:3:hit\n");
    for invalid in [
        "FILE-4:mixed\n",
        "FILE-4\n",
        "FILE-0-context\n",
        "FILE-18446744073709551616-context\n",
        "FILE-四-context\n",
        "FILE-4-body-5-ambiguous\n",
        "FILE-4-body.rs:5:ambiguous\n",
        "unknown line\n",
        "\nFILE:4:hit\n",
        "--\n",
        "--\n--\nFILE:4:hit\n",
        "[1 matches limit reached.]\nFILE:4:hit\n",
        "[1 matches limit reached.]\n[2 matches limit reached.]\n",
        "[0 matches limit reached.]\n",
        "[000 matches limit reached.]\n",
        "[1.5 matches limit reached.]\n",
        "[-1 matches limit reached.]\n",
        "[１ matches limit reached.]\n",
        "[ matches limit reached.]\n",
        "[2 matches limit reached]\n",
        "[2 matches truncated.]\n",
    ] {
        let text = format!("{valid}{}", invalid.replace("FILE", &file));
        let response = Processor::new(home.path()).process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::NoMatch, "{invalid}");
        assert_eq!(response.text, text);
        assert!(response.applied_rule.is_none());
        assert!(response.original_path.is_none());
    }
    for text in [
        format!("--\n{valid}"),
        format!("{file}-1-context\n{file}-2-context\n"),
        format!("{file}-1-context\n--\n{file}-2-context\n[2 matches limit reached.]\n"),
        "[2 matches limit reached.]\n".into(),
        "--\n".into(),
    ] {
        let response = Processor::new(home.path()).process(request(&text, Source::Search));
        // A lone bracketed notice can still reach the independent JSON rule.
        assert!(matches!(
            response.disposition,
            Disposition::NoMatch | Disposition::NotShorter
        ));
        assert!(
            response
                .candidates
                .iter()
                .all(|candidate| candidate.rule != "search_hit_groups")
        );
        assert_eq!(response.text, text);
        assert!(response.applied_rule.is_none());
        assert!(response.original_path.is_none());
    }
    assert!(!home.path().join(".outrim").exists());
}

#[test]
fn context_search_requires_strict_final_unicode_savings_including_hint_separator() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let file = format!("workspace/{}/a.rs", "long-component/".repeat(12));
    let probe = processor.process(request(
        &format!("{file}:1:hit\n{file}-2-context"),
        Source::Search,
    ));
    let hint_length = format!(
        "\n[outrim]: The full output is available at {}",
        probe.original_path.unwrap()
    )
    .chars()
    .count();
    // Two records save path length minus four header/indentation characters.
    // Vary Unicode path length to land below, on, and above the final hint cost.
    for (path_length, expected) in [
        (hint_length + 3, Disposition::NotShorter),
        (hint_length + 4, Disposition::NotShorter),
        (hint_length + 5, Disposition::Compressed),
    ] {
        let file = format!("{}.rs", "🦀".repeat(path_length - 3));
        let text = format!("{file}:1:hit\n{file}-2-context");
        let response = processor.process(request(&text, Source::Search));
        assert_eq!(response.disposition, expected);
        if expected == Disposition::Compressed {
            assert_eq!(response.text.chars().count() + 1, text.chars().count());
            assert_eq!(
                fs::read_to_string(response.original_path.unwrap()).unwrap(),
                text
            );
        } else {
            assert_eq!(response.text, text);
            assert!(response.original_path.is_none());
            assert!(response.applied_rule.is_none());
            assert!(response.candidates[0].body_chars < text.chars().count() as u64);
        }
    }
}

#[test]
fn context_search_keeps_source_gate_protection_and_storage_failure_behavior() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/module.rs", "long-component/".repeat(12));
    let text = format!("{file}:1:hit\n{file}-2-context\n");
    let processor = Processor::new(home.path());
    for source in [Source::Unknown, Source::SourceCode, Source::Configuration] {
        let response = processor.process(request(&text, source));
        assert_eq!(response.text, text);
        assert!(response.applied_rule.is_none());
        assert!(response.original_path.is_none());
    }
    assert!(!home.path().join(".outrim").exists());
    fs::write(home.path().join(".outrim"), "blocked").unwrap();
    let mut input = request(&text, Source::Search);
    input.tool.input = json!({"path": "src/module.rs"});
    let response = processor.process(input);
    assert_eq!(response.disposition, Disposition::FailedOpen);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn prose_prefixes_are_not_mistaken_for_search_paths() {
    let home = tempfile::tempdir().unwrap();
    for text in [
        "note a.rs:12:hello\nnote a.rs:13:world\n",
        "error:12:timeout\nerror:13:timeout\n",
        "x:y:12:foo\nx:y:13:bar\n",
        "12 matches in a.rs:3:x\n12 matches in a.rs:4:y\n",
    ] {
        let response = Processor::new(home.path()).process(request(text, Source::Search));
        assert_eq!(response.disposition, Disposition::NoMatch);
        assert_eq!(response.text, text);
    }
}

#[test]
fn terminal_blank_records_are_preserved_while_grouping_hits() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/module.rs", "long-component-name/".repeat(8));
    let text = format!("{file}:12:hit\n{file}:47:hit\n\n");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::Compressed);
    let original_path = response.original_path.unwrap();
    assert!(response.text.contains(&format!(
        "{file}:\n  12:hit\n  47:hit\n\n[outrim]: The full output is available at {original_path}"
    )));
    assert_eq!(fs::read_to_string(original_path).unwrap(), text);
}

#[test]
fn whitespace_in_path_prefixes_fails_open() {
    let home = tempfile::tempdir().unwrap();
    let path = format!("workspace/{}/a.rs", "deep-component/".repeat(10));
    let texts = [
        format!(" {path}:12:hello\n {path}:13:world\n"),
        format!("12 matches in {path}:12:hello\n12 matches in {path}:13:world\n"),
    ];
    for text in texts {
        let response = Processor::new(home.path()).process(request(&text, Source::Search));
        assert_eq!(response.disposition, Disposition::NoMatch);
        assert_eq!(response.text, text);
    }
}

#[test]
fn paths_without_repeated_hits_are_left_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let text = "src/main.rs:12:hit\nsrc/lib.rs:21:hit\n";
    let response = Processor::new(home.path()).process(request(text, Source::Search));

    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
}

#[test]
fn short_path_only_search_output_is_not_shorter_with_hint() {
    let home = tempfile::tempdir().unwrap();
    let text = "src/main.rs\nsrc/lib.rs\nsrc/main.rs\n";
    let response = Processor::new(home.path()).process(request(text, Source::Search));

    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, text);
}

#[test]
fn numeric_text_is_not_mistaken_for_a_search_path() {
    let home = tempfile::tempdir().unwrap();
    let text = "3:12:fails\n3:13:fails\n";
    let response = Processor::new(home.path()).process(request(text, Source::Search));

    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn search_records_with_ambiguous_numeric_colons_are_not_grouped() {
    let home = tempfile::tempdir().unwrap();
    let text = "C:12345678901234567890:5:hit\nC:12345678901234567890:6:hit\n";
    let response = Processor::new(home.path()).process(request(text, Source::Search));

    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
    assert!(response.applied_rule.is_none());
    assert!(response.original_path.is_none());
}

#[test]
fn drive_relative_paths_are_not_mistaken_for_line_number_records() {
    let home = tempfile::tempdir().unwrap();
    let text = "C:12:hit\nC:13:hit\n";
    let response = Processor::new(home.path()).process(request(text, Source::Search));

    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
}

#[test]
fn unsupported_extensionless_paths_fail_open_in_mixed_lists() {
    let home = tempfile::tempdir().unwrap();
    let path = format!("workspace/{}/module.rs", "long-component-name/".repeat(8));
    let text = format!("{path}:12:hit\nMakefile:3:hit\n{path}:47:hit\n");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, text);
}

#[test]
fn repeated_text_remains_tied_to_each_file_and_position() {
    let home = tempfile::tempdir().unwrap();
    let file = format!(
        "workspace/{}/duplicate.rs",
        "long-component-name/".repeat(8)
    );
    let other = "workspace/another/deeply/nested/module.rs";
    let mut text = format!("{file}:12:3:same match\n{other}:9:7:same match\n");
    for (line, column) in [(21, 4), (35, 6), (47, 8), (60, 2), (72, 9)] {
        text.push_str(&format!("{file}:{line}:{column}:same match\n"));
    }
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("search_hit_groups"));
    assert!(response.text.contains(&format!(
        "{file}:\n  12:3:same match\n  21:4:same match\n  35:6:same match\n  47:8:same match\n  60:2:same match\n  72:9:same match\n"
    )));
    assert!(
        response
            .text
            .contains(&format!("{other}:\n  9:7:same match\n"))
    );
    let original_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(original_path).unwrap(), text);
}

#[test]
fn grouping_separates_an_unterminated_hit_moved_before_another_group() {
    let home = tempfile::tempdir().unwrap();
    let file = format!(
        "workspace/{}/duplicate.rs",
        "long-component-name/".repeat(8)
    );
    let text = format!("{file}:12:first hit\nother.rs:3:other hit\n{file}:21:last hit");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert!(response.text.contains(&format!(
        "{file}:\n  12:first hit\n  21:last hit\nother.rs:\n  3:other hit\n"
    )));
}

#[test]
fn search_grouping_preserves_crlf_record_endings_and_trailing_blank_lines() {
    let home = tempfile::tempdir().unwrap();
    let file = format!("workspace/{}/module.rs", "long-component-name/".repeat(12));
    let text =
        format!("{file}:12:match\r\n{file}:47:match\r\n{file}:81:match\r\n{file}:96:match\r\n\r\n");
    let response = Processor::new(home.path()).process(request(&text, Source::Search));

    assert_eq!(response.disposition, Disposition::Compressed);
    let original_path = response.original_path.unwrap();
    assert!(response.text.contains(&format!(
        "{file}:\r\n  12:match\r\n  47:match\r\n  81:match\r\n  96:match\r\n\r\n[outrim]: The full output is available at {original_path}"
    )));
    assert_eq!(fs::read_to_string(original_path).unwrap(), text);
}

#[test]
fn log_source_keeps_log_policy_for_search_shaped_lines() {
    let home = tempfile::tempdir().unwrap();
    let path = format!("workspace/{}/module.rs", "long-component-name/".repeat(5));
    let mut text = format!("{}\n", "older log output ".repeat(50));
    let mut tail = String::new();
    for line in 1..=10 {
        let record = format!("{path}:{line}:hit\n");
        text.push_str(&record);
        tail.push_str(&record);
    }
    let response = Processor::new(home.path()).process(request(&text, Source::TestLog));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("log_tail"));
    assert!(response.text.starts_with(&tail));
    assert_eq!(
        fs::read_to_string(response.original_path.unwrap()).unwrap(),
        text
    );
}

#[test]
fn position_only_search_hits_group_repeated_paths_without_losing_positions() {
    let home = tempfile::tempdir().unwrap();
    let file = format!(
        "workspace/{}/src/module.rs",
        "long-location-component/".repeat(6)
    );
    let text = format!("{file}:8\n{file}:21\n{file}:34\n{file}:55\n");
    let mut input = request(&text, Source::Search);
    input.tool.input = json!({"path": "src/module.rs"});
    let response = Processor::new(home.path()).process(input);

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("search_hit_groups"));
    let original_path = response.original_path.unwrap();
    assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "{file}:\n  8\n  21\n  34\n  55\n[outrim]: The full output is available at {original_path}"
        )
    );
}

#[test]
fn search_results_group_repeated_paths_without_losing_hits_or_positions() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    let file = format!(
        "workspace/{}/src/duplicate.rs",
        "long-component-name/".repeat(6)
    );
    let other = "workspace/another/very/deep/module.rs";
    let text = format!(
        "{file}:12:same match\n{file}:47:same match\n{file}:81:same match\n{file}:96:same match\n{other}:9:same match\n"
    );
    for tool_input in [
        json!({"path": "src/duplicate.rs", "pattern": "same match"}),
        json!({"command": "rg -n 'same match' ."}),
        json!({"command": "grep -Rn 'same match' ."}),
        json!({"command": "git grep -n 'same match'"}),
    ] {
        let mut input = request(&text, Source::Search);
        input.tool.input = tool_input;
        let response = processor.process(input);

        assert_eq!(response.disposition, Disposition::Compressed);
        assert_eq!(response.applied_rule.as_deref(), Some("search_hit_groups"));
        assert!(response.text.chars().count() < text.chars().count());
        let original_path = response.original_path.unwrap();
        assert_eq!(fs::read_to_string(&original_path).unwrap(), text);
        assert_eq!(
            response.text,
            format!(
                "{file}:\n  12:same match\n  47:same match\n  81:same match\n  96:same match\n{other}:\n  9:same match\n[outrim]: The full output is available at {original_path}"
            )
        );
    }
}
