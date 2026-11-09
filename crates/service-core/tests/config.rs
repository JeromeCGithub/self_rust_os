use rustos_service_core::{config::*, Restart};
const SERVICE: &str = "Version=1\nKind=service\nName=glow\nExec=/BIN/GLOW.SVC\n";
fn failure(text: &str, kind: ErrorKind, line: usize) {
    let error = parse(text.as_bytes()).unwrap_err();
    assert_eq!(error.kind, kind, "{text}");
    assert_eq!(error.line, line, "{text}");
}
#[test]
fn service_preserves_explicit_vectors_without_shell_semantics() {
    let text = format!("{SERVICE}Arg=\nArg=two words\nArg=$HOME; echo nope\nEnv=MODE=a=b\nWorkingDirectory=/\nReady=spawned\nRestart=on-failure\nRestartDelayTicks=2\nRestartMaxDelayTicks=8\nRestartBurst=3\nRestartWindowTicks=100\nHealthyResetTicks=50\n");
    let unit = parse(text.as_bytes()).unwrap();
    assert_eq!(unit.name, "glow");
    assert_eq!(unit.kind, Kind::Service);
    assert_eq!(unit.executable.as_deref(), Some("/BIN/GLOW.SVC"));
    assert_eq!(unit.arguments, ["", "two words", "$HOME; echo nope"]);
    assert_eq!(unit.environment, ["MODE=a=b"]);
    assert_eq!(unit.policy.restart, Restart::OnFailure);
    assert_eq!(
        (
            unit.policy.delay,
            unit.policy.maximum_delay,
            unit.policy.burst
        ),
        (2, 8, 3)
    );
}
#[test]
fn target_retains_activation_and_ordering_as_distinct_edges() {
    let unit =
        parse(b"Version=1\nKind=target\nName=boot\nRequires=glow\nWants=optional\nAfter=setup\n")
            .unwrap();
    assert_eq!(unit.kind, Kind::Target);
    assert!(unit.executable.is_none());
    assert_eq!(unit.requires, ["glow"]);
    assert_eq!(unit.wants, ["optional"]);
    assert_eq!(unit.after, ["setup"]);
}
#[test]
fn crlf_comments_and_outer_ascii_whitespace_are_supported() {
    let unit = parse(b" # comment\r\n\r\n Version = 1 \r\nKind=service\r\nName=glow\r\nExec=/BIN/GLOW.SVC\r\nArg=  two spaces inside  \r\n").unwrap();
    assert_eq!(unit.arguments, ["two spaces inside"]);
}
#[test]
fn encoding_and_line_errors_are_precise() {
    assert_eq!(
        parse(b"\xff").unwrap_err(),
        Error {
            line: 1,
            kind: ErrorKind::Encoding
        }
    );
    failure(&format!("{SERVICE}Arg=a\0b\n"), ErrorKind::Encoding, 5);
    failure(&format!("{SERVICE}Arg=a\rb\n"), ErrorKind::Encoding, 5);
    failure(&format!("{SERVICE}not a directive\n"), ErrorKind::Syntax, 5);
    failure(&format!("{SERVICE}Bogus=1\n"), ErrorKind::UnknownKey, 5);
}
#[test]
fn duplicate_singletons_environment_names_and_edges_are_rejected() {
    failure(&format!("{SERVICE}Name=other\n"), ErrorKind::Duplicate, 5);
    failure(
        &format!("{SERVICE}Env=MODE=a\nEnv=MODE=b\n"),
        ErrorKind::Duplicate,
        6,
    );
    failure(
        &format!("{SERVICE}Requires=other\nRequires=other\n"),
        ErrorKind::Duplicate,
        6,
    );
    let unit = parse(format!("{SERVICE}Requires=other\nAfter=other\n").as_bytes()).unwrap();
    assert_eq!(unit.requires, unit.after);
}
#[test]
fn unsupported_future_mechanisms_are_not_silently_accepted() {
    failure(
        &format!("{SERVICE}Ready=bus-name\n"),
        ErrorKind::Unsupported,
        5,
    );
    failure(
        &format!("{SERVICE}WorkingDirectory=/HOME\n"),
        ErrorKind::Unsupported,
        5,
    );
    failure(
        &format!("{SERVICE}StopTimeoutTicks=90\n"),
        ErrorKind::UnknownKey,
        5,
    );
    failure(
        "Version=2\nKind=service\nName=glow\nExec=/BIN/GLOW.SVC\n",
        ErrorKind::Unsupported,
        1,
    );
}
#[test]
fn validates_required_fields_names_paths_and_environment() {
    for text in ["", "Version=1\n", "Version=1\nKind=service\nName=glow\n"] {
        assert_eq!(
            parse(text.as_bytes()).unwrap_err().kind,
            ErrorKind::MissingField
        );
    }
    for value in ["../glow", "Glow", "bad name", "", ".", "a/b"] {
        let text = SERVICE.replace("Name=glow", &format!("Name={value}"));
        failure(&text, ErrorKind::InvalidValue, 3);
    }
    for path in [
        "GLOW.SVC",
        "/BIN/../GLOW.SVC",
        "/BIN//GLOW.SVC",
        "/",
        "/BIN/./GLOW.SVC",
    ] {
        failure(
            &SERVICE.replace("/BIN/GLOW.SVC", path),
            ErrorKind::InvalidValue,
            4,
        );
    }
    for value in ["MISSING_EQUALS", "=x", "1KEY=x", "BAD KEY=x"] {
        failure(
            &format!("{SERVICE}Env={value}\n"),
            ErrorKind::InvalidValue,
            5,
        );
    }
}
#[test]
fn rejects_service_fields_in_targets_even_when_kind_is_last() {
    let text = "Version=1\nName=boot\nExec=/BIN/GLOW.SVC\nKind=target\n";
    assert_eq!(
        parse(text.as_bytes()).unwrap_err().kind,
        ErrorKind::InvalidValue
    );
}
#[test]
fn validates_numeric_boundaries_and_cross_field_restart_limits() {
    for value in ["-1", "18446744073709551616", "abc", "0"] {
        failure(
            &format!("{SERVICE}RestartDelayTicks={value}\n"),
            ErrorKind::InvalidValue,
            5,
        );
    }
    for value in ["0", "33", "65536"] {
        failure(
            &format!("{SERVICE}RestartBurst={value}\n"),
            ErrorKind::InvalidValue,
            5,
        );
    }
    assert!(
        parse(format!("{SERVICE}RestartDelayTicks=20\nRestartMaxDelayTicks=10\n").as_bytes())
            .is_err()
    );
    assert!(parse(format!("{SERVICE}RestartWindowTicks={}\n", u64::MAX).as_bytes()).is_err());
}
#[test]
fn bounds_file_vectors_launch_bytes_and_dependency_names() {
    let mut text = SERVICE.to_owned();
    for _ in 0..MAX_ARGS {
        text.push_str("Arg=x\n");
    }
    assert_eq!(parse(text.as_bytes()).unwrap().arguments.len(), MAX_ARGS);
    text.push_str("Arg=x\n");
    assert_eq!(parse(text.as_bytes()).unwrap_err().kind, ErrorKind::Limit);
    let text = format!("{SERVICE}Arg={}\n", "x".repeat(MAX_LAUNCH_BYTES));
    assert_eq!(parse(text.as_bytes()).unwrap_err().kind, ErrorKind::Limit);
    assert_eq!(
        parse(&vec![b' '; MAX_FILE_BYTES + 1]).unwrap_err().kind,
        ErrorKind::Limit
    );
    failure(
        &format!("{SERVICE}Requires={}\n", "x".repeat(49)),
        ErrorKind::InvalidValue,
        5,
    );
}

#[test]
fn rejects_control_characters_in_values_before_allocation() {
    for control in ['\u{7f}', '\u{85}'] {
        failure(
            &format!("{SERVICE}Arg=a{control}b\n"),
            ErrorKind::Encoding,
            5,
        );
    }
}
#[test]
fn exact_file_launch_environment_and_edge_limits_are_observable() {
    let mut bytes = SERVICE.as_bytes().to_vec();
    bytes.resize(MAX_FILE_BYTES, b' ');
    assert!(parse(&bytes).is_ok());
    let available = MAX_LAUNCH_BYTES - "/BIN/GLOW.SVC".len() - 2;
    let exact = format!("{SERVICE}Arg={}\n", "x".repeat(available));
    assert!(parse(exact.as_bytes()).is_ok());
    assert_eq!(
        parse(format!("{SERVICE}Arg={}\n", "x".repeat(available + 1)).as_bytes())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let mut text = SERVICE.to_owned();
    for index in 0..MAX_ENV {
        text.push_str(&format!("Env=KEY{index}=x\n"));
    }
    assert_eq!(parse(text.as_bytes()).unwrap().environment.len(), MAX_ENV);
    text.push_str("Env=OVERFLOW=x\n");
    assert_eq!(parse(text.as_bytes()).unwrap_err().kind, ErrorKind::Limit);
    let mut text = SERVICE.to_owned();
    for index in 0..MAX_EDGES {
        text.push_str(&format!("Requires=u{index}\n"));
    }
    assert_eq!(parse(text.as_bytes()).unwrap().requires.len(), MAX_EDGES);
    text.push_str("After=overflow\n");
    assert_eq!(parse(text.as_bytes()).unwrap_err().kind, ErrorKind::Limit);
}

#[test]
fn notify_configuration_has_explicit_start_deadline_and_optional_watchdog() {
    let unit = parse(
        format!("{SERVICE}Ready=notify\nStartTimeoutTicks=45\nWatchdogTicks=18\n").as_bytes(),
    )
    .unwrap();
    assert_eq!(unit.readiness, Readiness::Notify);
    assert_eq!(unit.start_timeout, 45);
    assert_eq!(unit.watchdog, Some(18));
    let default = parse(SERVICE.as_bytes()).unwrap();
    assert_eq!(default.readiness, Readiness::Spawned);
    assert_eq!(default.start_timeout, 90);
    assert_eq!(default.watchdog, None);
}
#[test]
fn watchdog_requires_notify_and_new_timing_fields_reject_duplicates_and_invalid_numbers() {
    for key in ["StartTimeoutTicks", "WatchdogTicks"] {
        for value in ["0", "-1", "9223372036854775808", "18446744073709551616"] {
            let text = format!("{SERVICE}Ready=notify\n{key}={value}\n");
            failure(&text, ErrorKind::InvalidValue, 6);
        }
        failure(
            &format!("{SERVICE}Ready=notify\n{key}=1\n{key}=2\n"),
            ErrorKind::Duplicate,
            7,
        );
    }
    failure(
        &format!("{SERVICE}WatchdogTicks=2\n"),
        ErrorKind::InvalidValue,
        5,
    );
    failure(
        &format!("{SERVICE}WatchdogTicks=2\nReady=spawned\n"),
        ErrorKind::InvalidValue,
        5,
    );
}
#[test]
fn target_rejects_readiness_timing_keys_even_when_kind_is_last() {
    for key in ["StartTimeoutTicks", "WatchdogTicks"] {
        failure(
            &format!("Version=1\nName=boot\n{key}=2\nKind=target\n"),
            ErrorKind::InvalidValue,
            1,
        );
    }
}
