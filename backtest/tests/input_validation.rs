use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "forex-input-validation-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}

fn config(dir: &Path, reference_exit: bool) -> PathBuf {
    let document: Value =
        serde_json::from_slice(&fs::read("../tests/fixtures/swing-legacy-rules.json").unwrap())
            .unwrap();
    let mut cfg = document["instruments"]["EURUSD"].clone();
    if reference_exit {
        cfg["reference_exit"] = json!({"sign": 1, "recent_events": 4});
    }
    let path = dir.join("swing.json");
    fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    path
}

fn replay(
    input: &PathBuf,
    cfg: &PathBuf,
    output: &PathBuf,
    index: Option<(&PathBuf, &PathBuf)>,
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_forex-backtest"));
    cmd.arg("swing-replay").arg(input).arg(cfg).arg(output);
    if let Some((index, index_cfg)) = index {
        cmd.arg(index).arg(index_cfg);
    }
    cmd.output().unwrap()
}

fn good_mt5_rows() -> String {
    "2024.01.08\t00:00:30\t1.1\t1.2\t1.0\t1.15\t1\t0\t0\n2024.01.08\t00:02:00\t1.15\t1.21\t1.14\t1.2\t1\t0\t0\n".into()
}

#[test]
fn swing_replay_rejects_invalid_ohlc_and_nonincreasing_times_without_output() {
    let dir = temp();
    let cfg = config(&dir, false);
    let cases = [
        "2024.01.08\t00:00:00\tNaN\t1.2\t1.0\t1.1\t1\t0\t0\n",
        "2024.01.08\t00:00:00\t1.1\tinf\t1.0\t1.1\t1\t0\t0\n",
        "2024.01.08\t00:00:00\t0\t1.2\t0\t1.1\t1\t0\t0\n",
        "2024.01.08\t00:00:00\t1.1\t1.0\t0.9\t1.05\t1\t0\t0\n",
        "2024.01.08\t00:01:00\t1.1\t1.2\t1.0\t1.15\t1\t0\t0\n2024.01.08\t00:01:00\t1.15\t1.21\t1.14\t1.2\t1\t0\t0\n",
    ];
    for (i, csv) in cases.into_iter().enumerate() {
        let input = dir.join(format!("bad-{i}.csv"));
        let output = dir.join(format!("bad-{i}-trades.csv"));
        fs::write(&input, csv).unwrap();
        let result = replay(&input, &cfg, &output, None);
        assert!(!result.status.success(), "case {i} unexpectedly passed");
        assert!(!output.exists(), "case {i} left a result file");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn swing_replay_requires_reference_index_but_accepts_no_index_events() {
    let dir = temp();
    let cfg = config(&dir, true);
    let input = dir.join("price.csv");
    fs::write(&input, good_mt5_rows()).unwrap();
    let missing_output = dir.join("missing.csv");
    let missing = replay(&input, &cfg, &missing_output, None);
    assert!(
        !missing.status.success(),
        "reference_exit without index must fail: {}",
        String::from_utf8_lossy(&missing.stderr)
    );
    assert!(!missing_output.exists());

    let index_cfg = config(&dir, false);
    let index = dir.join("index.csv");
    // One valid minute is not required to produce any index signal.
    fs::write(&index, good_mt5_rows()).unwrap();
    let output = dir.join("with-index.csv");
    let result = replay(&input, &cfg, &output, Some((&index, &index_cfg)));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dxy_build_rejects_empty_intersection_and_preserves_existing_output() {
    let dir = temp();
    let mut inputs = Vec::new();
    for i in 0..6 {
        let path = dir.join(format!("pair-{i}.csv"));
        fs::write(
            &path,
            format!("2024.01.08\t00:{:02}:00\t1.1\t1.2\t1.0\t1.15\t1\t0\t0\n", i),
        )
        .unwrap();
        inputs.push(path);
    }
    let output = dir.join("dxy.csv");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_forex-backtest"));
    cmd.arg("dxy-build").args(&inputs).arg(&output);
    let result = cmd.output().unwrap();
    assert!(
        !result.status.success(),
        "empty timestamp intersection must fail"
    );
    assert!(!output.exists());

    for path in &inputs {
        fs::write(path, "2024.01.08\t00:00:00\t1.1\t1.2\t1.0\t1.15\t1\t0\t0\n").unwrap();
    }
    let existing = dir.join("existing.csv");
    fs::write(&existing, b"keep me\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_forex-backtest"));
    cmd.arg("dxy-build").args(&inputs).arg(&existing);
    assert!(!cmd.status().unwrap().success());
    assert_eq!(fs::read(existing).unwrap(), b"keep me\n");
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn dxy_build_refuses_a_dangling_output_symlink() {
    use std::os::unix::fs::symlink;
    let dir = temp();
    let inputs: Vec<_> = (0..6)
        .map(|i| {
            let path = dir.join(format!("pair-{i}.csv"));
            fs::write(
                &path,
                "2024.01.08\t00:00:00\t1.1\t1.2\t1.0\t1.15\t1\t0\t0\n",
            )
            .unwrap();
            path
        })
        .collect();
    let target = dir.join("absent-target");
    let link = dir.join("link.csv");
    symlink(&target, &link).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_forex-backtest"));
    cmd.arg("dxy-build").args(&inputs).arg(&link);
    assert!(!cmd.status().unwrap().success());
    assert!(
        !target.exists(),
        "writer followed and created the dangling target"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn swing_replay_allows_gaps_and_subminute_times_and_refuses_existing_output() {
    let dir = temp();
    let cfg = config(&dir, false);
    let input = dir.join("valid.csv");
    fs::write(&input, good_mt5_rows()).unwrap();
    let output = dir.join("trades.csv");
    let result = replay(&input, &cfg, &output, None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let before = fs::read(&output).unwrap();
    let again = replay(&input, &cfg, &output, None);
    assert!(!again.status.success());
    assert_eq!(fs::read(&output).unwrap(), before);

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let dangling_target = dir.join("swing-dangling-target");
        let link = dir.join("swing-dangling.csv");
        symlink(&dangling_target, &link).unwrap();
        let refused = replay(&input, &cfg, &link, None);
        assert!(!refused.status.success());
        assert!(!dangling_target.exists());
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn swing_replay_rejects_empty_primary_history_before_output() {
    let dir = temp();
    let cfg = config(&dir, false);
    for (name, contents) in [
        ("zero-byte", ""),
        (
            "header-only",
            "<DATE>\t<TIME>\t<OPEN>\t<HIGH>\t<LOW>\t<CLOSE>\t<TICKVOL>\t<VOL>\t<SPREAD>\n",
        ),
    ] {
        let input = dir.join(format!("{name}-mt5.csv"));
        let output = dir.join(format!("{name}-trades.csv"));
        fs::write(&input, contents).unwrap();
        let result = replay(&input, &cfg, &output, None);
        assert!(!result.status.success(), "{name} primary history must fail");
        assert!(!output.exists(), "{name} primary must fail before output");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn swing_replay_rejects_empty_reference_index_before_output() {
    let dir = temp();
    let cfg = config(&dir, true);
    let index_cfg = config(&dir, false);
    let input = dir.join("valid-primary.csv");
    fs::write(&input, good_mt5_rows()).unwrap();
    for (name, contents) in [
        ("zero-byte", ""),
        (
            "header-only",
            "<DATE>\t<TIME>\t<OPEN>\t<HIGH>\t<LOW>\t<CLOSE>\t<TICKVOL>\t<VOL>\t<SPREAD>\n",
        ),
    ] {
        let index = dir.join(format!("{name}-index.csv"));
        let output = dir.join(format!("{name}-index-trades.csv"));
        fs::write(&index, contents).unwrap();
        let result = replay(&input, &cfg, &output, Some((&index, &index_cfg)));
        assert!(!result.status.success(), "{name} reference index must fail");
        assert!(!output.exists(), "{name} index must fail before output");
    }
    fs::remove_dir_all(dir).unwrap();
}
