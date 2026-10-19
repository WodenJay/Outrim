use outrim::{Processor, protocol::*};
use serde_json::json;
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
fn source_and_configuration_are_protected_before_log_routing() {
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
fn short_logs_and_short_unknown_content_pass_through() {
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
fn generic_fallback_keeps_head_and_tail_and_restores_original() {
    let home = tempfile::tempdir().unwrap();
    let lines: Vec<_> = (1..=24)
        .map(|line| format!("row {line}: {} 🦀\n", "日志".repeat(30)))
        .collect();
    let text = lines.concat();
    let retained = format!("{}{}", lines[..10].concat(), lines[14..].concat());
    let response = Processor::new(home.path()).process(request(&text, Source::Unknown));

    assert_eq!(response.disposition, Disposition::Compressed);
    assert_eq!(
        response.applied_rule.as_deref(),
        Some("generic_line_fallback")
    );
    let path = response.original_path.unwrap();
    assert!(Path::new(&path).is_absolute());
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(
        response.text,
        format!("{retained}[outrim]: The full output is available at {path}")
    );
}

#[test]
fn generic_fallback_observes_line_and_unicode_length_boundaries() {
    let home = tempfile::tempdir().unwrap();
    let processor = Processor::new(home.path());

    let nineteen = (1..=19)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    let response = processor.process(request(&nineteen, Source::Unknown));
    assert_eq!(response.disposition, Disposition::NoMatch);
    assert_eq!(response.text, nineteen);

    let twenty = (1..=20)
        .map(|line| format!("line {line}: {}\n", "🦀".repeat(40)))
        .collect::<String>();
    let response = processor.process(request(&twenty, Source::Unknown));
    assert_eq!(response.disposition, Disposition::NotShorter);
    assert_eq!(response.text, twenty);
    assert!(response.original_path.is_none());

    let head = (1..=10)
        .map(|line| format!("head {line}: {}\n", "🦀".repeat(40)))
        .collect::<String>();
    let tail = (11..=20)
        .map(|line| format!("tail {line}: {}\n", "🦀".repeat(40)))
        .collect::<String>();
    let retained = format!("{head}{tail}");
    let probe_text = format!("{head}{}\n{tail}", "🦀".repeat(200));
    let probe = processor.process(request(&probe_text, Source::Unknown));
    assert_eq!(probe.disposition, Disposition::Compressed);
    let omitted_length = probe.text.chars().count() - retained.chars().count();
    let equal_text = format!("{head}{}\n{tail}", "🦀".repeat(omitted_length - 1));
    let equal = processor.process(request(&equal_text, Source::Unknown));
    assert_eq!(equal.disposition, Disposition::NotShorter);
    assert_eq!(equal.text, equal_text);

    let longer_text = format!("{head}{}\n{tail}", "🦀".repeat(omitted_length));
    let longer = processor.process(request(&longer_text, Source::Unknown));
    assert_eq!(longer.disposition, Disposition::Compressed);
    assert_eq!(longer_text.chars().count(), longer.text.chars().count() + 1);
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
