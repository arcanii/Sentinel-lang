//! Shared by the self-hosted driver tests: the refusals `scg` ports (ADR 0041 A14,
//! register D97).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The oracle's refusals the self-hosted typer ports, by diagnostic code. Every other type
/// error is still out of scope (ADR 0041 D7), so a driver typing such a program need not
/// refuse it.
const PORTED_REFUSALS: &[&str] = &[
    "sentinel::types::unknown_field",
    "sentinel::types::duplicate_field",
    "sentinel::types::missing_field",
    "sentinel::types::vec_to_array_element_not_plain",
    "sentinel::types::init_field_maybe_unassigned",
    "sentinel::types::init_field_read_before_assign",
    "sentinel::types::init_self_used_before_assigned",
    "sentinel::types::return_in_init",
];

/// Every `tests/ui` fixture whose pinned diagnostic (its `ui.rs` snapshot) carries one of
/// the ported codes must be refused by the self-hosted `driver` as well: a non-zero exit and
/// an output of exactly two lines, the code and then, after `scg: `, the oracle's message
/// (the line `snc types` reports after `snc: `). Answers how many fixtures it
/// checked, so a caller can pin that the set is not empty.
///
/// The other direction, a driver refusing a program the oracle accepts, is every corpus
/// differential: a refusal replaces the driver's output, so it cannot match the oracle's.
pub fn assert_refuses_what_the_oracle_refuses(driver: &Path, work: &Path) -> usize {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    let snaps = root.join("crates/sentinel-driver/tests/snapshots");
    let mut pinned: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&snaps).expect("read snapshots") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("ui__") || !name.ends_with(".snap") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read snapshot");
        // `expression: "reject_stderr(\"<fixture>\")"`, then the body after the second
        // `---`, whose first line is the diagnostic's code.
        let fixture = text
            .lines()
            .find_map(|l| l.strip_prefix("expression: \"reject_stderr(\\\""))
            .and_then(|rest| rest.split("\\\"").next())
            .map(str::to_string);
        let code = text
            .splitn(3, "---")
            .nth(2)
            .and_then(|body| body.lines().map(str::trim).find(|l| !l.is_empty()))
            .map(str::to_string);
        if let (Some(fixture), Some(code)) = (fixture, code) {
            if PORTED_REFUSALS.contains(&code.as_str()) {
                pinned.push((fixture, code));
            }
        }
    }
    pinned.sort();

    std::fs::create_dir_all(work).expect("create work dir");
    let input = work.join("input.sentinel");
    let mut failures: Vec<String> = Vec::new();
    for (fixture, code) in &pinned {
        let src = root.join("tests/ui").join(fixture);
        std::fs::copy(&src, &input).expect("stage fixture");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("types")
            .arg(&input)
            .output()
            .expect("run snc types");
        let oracle_err = String::from_utf8_lossy(&oracle.stderr);
        let message = oracle_err
            .lines()
            .find_map(|l| l.strip_prefix("snc: "))
            .unwrap_or("")
            .to_string();
        let out = Command::new(driver)
            .current_dir(work)
            .output()
            .expect("run the self-hosted driver");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines();
        let got_code = lines.next().unwrap_or("");
        let got_message = lines.next().and_then(|l| l.strip_prefix("scg: ")).unwrap_or("");
        // Exactly the one refusal: the first the walk meets, and nothing after it.
        let lines_out = stdout.lines().count();
        if out.status.success() || got_code != code || got_message != message || lines_out != 2 {
            failures.push(format!(
                "  {fixture}: oracle `{code}` / `{message}`; driver exit {:?}, `{got_code}` / `{got_message}` ({lines_out} lines)",
                out.status.code()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "the self-hosted driver did not refuse {}/{} fixture(s) the oracle refuses with a ported code:\n{}",
        failures.len(),
        pinned.len(),
        failures.join("\n")
    );
    pinned.len()
}
