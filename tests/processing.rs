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
fn source_and_configuration_are_protected_before_content_routing() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    for source in [Source::SourceCode, Source::Configuration] {
        let input = request(&log(), source);
        let response = processor.process(input.clone());
        assert_eq!(response.disposition, Disposition::Protected);
        assert_eq!(response.text, input.text);
        assert!(response.applied_rule.is_none());
        assert!(response.original_path.is_none());
    }
    let records: Vec<_> = (0..80)
        .map(|id| json!({"id": id, "kind": "event", "state": "complete"}))
        .collect();
    let json_text = serde_json::to_string_pretty(&json!({"records": records})).unwrap();
    for source in [Source::SourceCode, Source::Configuration] {
        let input = request(&json_text, source);
        let response = processor.process(input.clone());
        assert_eq!(response.disposition, Disposition::Protected);
        assert_eq!(response.text, input.text);
        assert!(response.applied_rule.is_none());
        assert!(response.original_path.is_none());
    }
    let mut input = request(&json_text, Source::Unknown);
    input.tool.input = json!({"path": "config/settings.json"});
    let response = processor.process(input.clone());
    assert_eq!(response.disposition, Disposition::Protected);
    assert_eq!(response.text, input.text);
    let mut input = request(&log(), Source::BuildLog);
    input.tool.input = json!({"path": "src/main.rs"});
    let response = processor.process(input.clone());
    assert_eq!(response.disposition, Disposition::Protected);
    assert_eq!(response.text, input.text);
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
    let home = tempfile::tempdir().unwrap();
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
        if expected == Disposition::NotShorter {
            assert_eq!(response.text, text);
            assert!(response.original_path.is_none());
            assert!(response.applied_rule.is_none());
        } else {
            assert_eq!(response.text.chars().count() + 1, text.chars().count());
        }
    }
}

#[test]
fn short_logs_and_unimplemented_sources_pass_through() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());
    for text in ["", "one line", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n"] {
        let response = processor.process(request(text, Source::BuildLog));
        assert_eq!(response.disposition, Disposition::NotShorter);
        assert_eq!(response.text, text);
    }
    for source in [Source::Search, Source::Unknown] {
        let response = processor.process(request(&log(), source));
        assert_eq!(response.disposition, Disposition::NoMatch);
        assert_eq!(response.text, log());
    }
    assert!(!home.path().join(".outrim").exists());
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
    let text = log();
    let response = Processor::new(home.path()).process(request(&text, Source::TestLog));
    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("log_tail"));
    let path = response.original_path.unwrap();
    assert!(Path::new(&path).is_absolute());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(
        response.text,
        format!(
            "01\n02\n03\n04\n05\n06\n07\n08\n09\n10\n[outrim]: The full output is available at {path}"
        )
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
fn repeated_json_records_choose_the_smaller_lossless_toon_candidate() {
    let home = tempfile::tempdir().unwrap();
    let records: Vec<_> = (0..80)
        .map(|id| {
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
    let rows = vec![row; 40].join(",\n    ");
    let text = format!("{{\n  \"records\": [\n    {rows}\n  ]\n}}");
    let expected = format!("{{\"records\":[{}]}}", vec![compact_row; 40].join(","));

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
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
    let rows = vec![row; 80].join(",\n    ");
    let text = format!("{{\n  \"records\": [\n    {rows}\n  ]\n}}");

    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(response.applied_rule.as_deref(), Some("json_compact"));
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
