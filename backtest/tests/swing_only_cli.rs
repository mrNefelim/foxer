use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "forex swing only cli {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn help_exposes_swing_commands_without_the_retired_fvg_interface() {
    let result = Command::new(env!("CARGO_BIN_EXE_forex-backtest"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    for command in [
        "swing-history",
        "swing-replay",
        "dxy-build",
        "--report-only",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
    for option in ["--strategy", "--data-config", "--no-pdf"] {
        assert!(
            !help.contains(option),
            "retired FVG option advertised: {option}"
        );
    }
}

#[test]
fn retired_fvg_options_are_rejected_as_unsupported_commands() {
    let result = Command::new(env!("CARGO_BIN_EXE_forex-backtest"))
        .args(["--strategy", "does-not-exist.json"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unknown command: --strategy"));
}

#[cfg(unix)]
#[test]
fn report_only_passes_saved_swing_json_to_the_renderer_without_a_replay() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp();
    let input = root.join("report.json");
    let capture = root.join("renderer arguments.txt");
    fs::write(&input, b"{\"schema_version\":1}").unwrap();
    let python = root.join("python3");
    fs::write(
        &python,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$REPORT_CAPTURE\"\n",
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_forex-backtest"))
        .arg("--report-only")
        .arg(&root)
        .env("PATH", &root)
        .env("REPORT_CAPTURE", &capture)
        .env_remove("FOREX_REPORT_SCRIPT")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let args = fs::read_to_string(&capture).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    assert_eq!(
        args.len(),
        3,
        "renderer must get its script, saved metrics and PDF path"
    );
    assert!(args[0].ends_with("backtest/report/render_swing.py"));
    assert_eq!(args[1], input.to_str().unwrap());
    assert_eq!(args[2], root.join("report.pdf").to_str().unwrap());
    assert_eq!(fs::read(&input).unwrap(), b"{\"schema_version\":1}");
    assert!(!root.join("trades.csv").exists());
    fs::remove_dir_all(root).unwrap();
}
