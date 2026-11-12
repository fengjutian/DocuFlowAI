//! Phase 0 · P0-004: an editor connects to CodeWeaver over ACP, on the real runtime.
//!
//! The editor's own ACP client — the same `codeweaver-agent-acp` code the editor uses — drives
//! the *real* agent endpoint, and the endpoint runs the task on the product's primary runtime
//! with a real model. The mock runtime of the earlier check is gone: the model has to read a file
//! whose content exists nowhere else and answer with it, so an answer that is not in the file
//! cannot be produced.
//!
//! ```powershell
//! $env:MINIMAX_CN_API_KEY = "<your key>"
//! cargo run -p codeweaver-platform --example acp_real_runtime
//! ```
//!
//! The report is written to `codeweaver-hermes-lab/reports/phase0-acp-editor.json` in the shape
//! the Phase 0 aggregate reads, so this item joins the same checklist as the server-side probes.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use codeweaver_agent_acp::{Client, StopReason};
use codeweaver_agent_ui::AgentPanel;

/// The tool the probe expects the agent to use to read the nonce file.
const READ_TOOL: &str = "read_file";

/// A runtime that is not registered, used by the control arm.
const UNKNOWN_RUNTIME: &str = "no-such-runtime";

/// What the probe concluded about the turn.
#[derive(Debug, PartialEq)]
pub struct Verdict {
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// Judges one editor turn.
///
/// Four things have to hold, and each of them can fail on its own: the agent really called the
/// file-reading tool (an answer that names a random string cannot come from nowhere), it called
/// *that* tool and not some other one, the turn finished (EndTurn), and the transcript carries the
/// nonce that only exists in the workspace file.
pub fn judge(
    transcript: &str,
    tool_titles: &[String],
    stop_reason: Option<StopReason>,
    nonce: &str,
) -> Verdict {
    let mut reasons: Vec<String> = Vec::new();

    if tool_titles.is_empty() {
        reasons.push(
            "the agent never called a tool, so the answer could not have come from the file".into(),
        );
    } else if !tool_titles.iter().any(|title| title == READ_TOOL) {
        reasons.push(format!(
            "the agent called {} but not {READ_TOOL}, so the file was never read",
            tool_titles.join(", ")
        ));
    }
    match stop_reason {
        Some(StopReason::EndTurn) => {}
        Some(other) => reasons.push(format!("the turn ended with {other:?}, not EndTurn")),
        None => reasons.push("the turn never finished".into()),
    }
    if !transcript.contains(nonce) {
        reasons.push(
            "the transcript does not carry the nonce that only exists in the workspace file".into(),
        );
    }

    Verdict {
        passed: reasons.is_empty(),
        reasons,
    }
}

/// ISO-8601 UTC, to the millisecond.
fn iso_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    let millis = now.as_millis() as i64;
    let seconds = millis.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
        millis.rem_euclid(1000)
    )
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// A nonce that exists nowhere else: not in the prompt, not in the model, not in the transcript.
fn fresh_nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    format!("NONCE-ACP-{}-{}", std::process::id(), now.as_millis())
}

fn run_git(args: &[&str], cwd: &Path) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed in {}", cwd.display());
}

/// A workspace the agent may read: a git repository with one file that holds the nonce.
fn make_workspace(root: &Path, nonce: &str) -> PathBuf {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("the workspace directory is created");
    fs::write(workspace.join("nonce.txt"), format!("{nonce}\n"))
        .expect("the nonce file is written");
    run_git(&["init", "-b", "main"], &workspace);
    run_git(&["config", "user.name", "CodeWeaver Probe"], &workspace);
    run_git(
        &["config", "user.email", "probe@codeweaver.local"],
        &workspace,
    );
    run_git(&["add", "-A"], &workspace);
    run_git(&["commit", "-m", "initial"], &workspace);
    workspace
}

/// The journal's own account of the run.
///
/// The editor only sees what the ACP layer forwards, so a run that dies early looks the same as a
/// run that never started. The journal is where the truth is: the terminal entry says what
/// happened. A probe that cannot say *why* it failed is a probe that reports "it broke".
fn journal_evidence(data_dir: &Path) -> Vec<String> {
    let mut files: Vec<PathBuf> = match fs::read_dir(data_dir.join("journal")) {
        Ok(dir) => dir
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect(),
        Err(_) => return Vec::new(),
    };
    files.sort();

    let mut lines = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let text = fs::read_to_string(&file).unwrap_or_default();
        for line in text.lines() {
            let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let category = entry.get("category").and_then(|v| v.as_str()).unwrap_or("");
            if !category.starts_with("execution.") {
                continue;
            }
            let payload = entry
                .get("payload")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_string());
            let payload = if payload.chars().count() > 600 {
                let head: String = payload.chars().take(600).collect();
                format!("{head}…")
            } else {
                payload
            };
            lines.push(format!("journal {name} {category} {payload}"));
        }
    }
    lines
}

/// Waits for the journal to reach a terminal entry.
///
/// The turn ends before the journal does. The coordinator publishes the terminal event and
/// appends it in the same tick, but the probe reads the file from another process, so a read that
/// happens too early stops at `execution.started`: a run with no ending at all, which reads like
/// a hang rather than the failure it actually was.
fn await_terminal_journal(data_dir: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let terminal = journal_evidence(data_dir).iter().any(|line| {
            line.contains("execution.completed")
                || line.contains("execution.failed")
                || line.contains("execution.cancelled")
        });
        if terminal {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Judges the control arm: the endpoint refused, and it said which runtime it could not find.
///
/// A named runtime that is not registered must be refused, not replaced (D-047). This arm runs
/// before any model call, so it costs no tokens and cannot be faked by a slow network: either
/// the endpoint refused on the spot, or it did not.
pub fn judge_refusal(error: Option<&str>, runtime_id: &str) -> Verdict {
    let mut reasons: Vec<String> = Vec::new();
    match error {
        None => reasons.push("the endpoint ran the task anyway, on a runtime nobody named".into()),
        Some(message) => {
            if !message.contains(runtime_id) {
                reasons.push(format!(
                    "the refusal does not name the runtime '{runtime_id}': {message}"
                ));
            }
        }
    }
    Verdict {
        passed: reasons.is_empty(),
        reasons,
    }
}

/// The endpoint the editor spawns, pointed at a runtime and a workspace.
///
/// The reference runtime is pointed at a closed port so that nothing can quietly answer for it:
/// the only runtime that can serve a task here is the one that was named.
fn endpoint_config(
    endpoint: &Path,
    data_dir: &Path,
    workspace: &Path,
    worktree_root: &Path,
    runtime_id: &str,
) -> codeweaver_agent_acp::EndpointConfig {
    codeweaver_agent_acp::EndpointConfig::new("node", vec![endpoint.to_string_lossy().to_string()])
        .with_env(
            "CODEWEAVER_DATA_DIR",
            data_dir.to_string_lossy().to_string(),
        )
        .with_env(
            "CODEWEAVER_REPOSITORY",
            workspace.to_string_lossy().to_string(),
        )
        .with_env(
            "CODEWEAVER_WORKTREE_ROOT",
            worktree_root.to_string_lossy().to_string(),
        )
        .with_env("HERMES_HTTP_BASE_URL", "http://127.0.0.1:9")
        .with_env("PI_MODEL", "MiniMax-M3")
        .with_env("CODEWEAVER_ACP_RUNTIME_ID", runtime_id)
}

/// Runs one turn and waits for it to end.
///
/// The turn ends in one of three ways: the agent finished, the endpoint refused, or the deadline
/// passed. The first two are answers; the third is not, and it is left to the judge.
fn run_turn(
    client: &Client,
    session_id: &str,
    prompt: &str,
    timeout: Duration,
) -> (AgentPanel, usize) {
    let mut panel = AgentPanel::new();
    panel.begin_turn();
    client
        .start_prompt(session_id, prompt)
        .expect("the prompt is written");

    let deadline = Instant::now() + timeout;
    let mut events = 0usize;
    while Instant::now() < deadline {
        for event in client.drain_events() {
            panel.apply(event);
            events += 1;
        }
        if panel.last_stop_reason.is_some() || panel.error.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    (panel, events)
}

/// The control arm: a runtime that is not registered is refused.
///
/// This arm runs before the model arm and costs no tokens: the refusal happens when the task
/// starts, long before anything is sent to a model. An endpoint that quietly ran the task on some
/// other runtime would pass the main arm too, so this arm is what makes the main arm mean
/// something.
fn control_arm(endpoint: &Path, workspace: &Path, scratch: &Path) -> (Verdict, Option<String>) {
    let data_dir = scratch.join("data-unknown-runtime");
    fs::create_dir_all(&data_dir).expect("the control arm's data directory is created");
    let config = endpoint_config(
        endpoint,
        &data_dir,
        workspace,
        &scratch.join("worktrees-control"),
        UNKNOWN_RUNTIME,
    );

    let client = match Client::start(&config) {
        Ok(client) => client,
        Err(error) => {
            return (
                Verdict {
                    passed: false,
                    reasons: vec![format!("the endpoint did not start: {error}")],
                },
                None,
            );
        }
    };
    let session = match client.new_session(workspace) {
        Ok(session) => session,
        Err(error) => {
            return (
                Verdict {
                    passed: false,
                    reasons: vec![format!("the session did not open: {error}")],
                },
                None,
            );
        }
    };

    let (panel, _events) = run_turn(
        &client,
        &session.session_id,
        "say pong",
        Duration::from_secs(60),
    );
    let refusal = panel.error.clone();
    let verdict = judge_refusal(refusal.as_deref(), UNKNOWN_RUNTIME);
    (verdict, refusal)
}

/// The pinned client base: the Zed version this editor is built on.
fn zed_version(root: &Path) -> String {
    let pin = root.join("client/PIN.json");
    match fs::read_to_string(&pin) {
        Ok(text) => parse_zed_version(&text),
        Err(_) => "unknown".to_string(),
    }
}

/// The version out of the pin text.
///
/// The pin file is written by a PowerShell script, so it carries a UTF-8 BOM. `serde_json` does
/// not skip it, and a pin that reads as "unknown" is worse than no pin at all: it looks like a
/// fact about the client and is not one.
fn parse_zed_version(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("zedVersion")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Whether the run was stopped by the environment rather than by the product.
///
/// A model provider that refuses the call for lack of quota prevented the verification; it did
/// not contradict it. Calling that "failed" blames the product for the environment, and calling
/// it "passed" is a lie — the check simply did not happen.
pub fn is_environment_refusal(journal: &[String]) -> bool {
    journal.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("execution.failed")
            && (lower.contains("429")
                || lower.contains("rate_limit")
                || lower.contains("usage limit")
                || lower.contains("用量上限"))
    })
}

fn build_report(
    status: &str,
    notes: Option<&str>,
    evidence: &[String],
    started_at: &str,
    finished_at: &str,
) -> serde_json::Value {
    // The result carries `notes` only when there is a note. A `null` in that field is not the same
    // as an absent one: the Phase 0 aggregator validates the shape and rejects a null note.
    let mut result = serde_json::json!({
        "id": "P0-004",
        "title": "ACP editor connection",
        "status": status,
        "startedAt": started_at,
        "finishedAt": finished_at,
        "evidence": evidence
    });
    if let Some(notes) = notes {
        result["notes"] = serde_json::Value::String(notes.to_owned());
    }

    serde_json::json!({
        "schemaVersion": "1.0",
        "generatedAt": finished_at,
        "summary": {
            "total": 1,
            "passed": if status == "passed" { 1 } else { 0 },
            "failed": if status == "failed" { 1 } else { 0 },
            "blocked": if status == "blocked" { 1 } else { 0 },
            "notRun": 0
        },
        "results": [result]
    })
}

fn write_report(
    path: &Path,
    status: &str,
    notes: Option<&str>,
    evidence: &[String],
    started_at: &str,
    finished_at: &str,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let report = build_report(status, notes, evidence, started_at, finished_at);
    fs::write(path, serde_json::to_string_pretty(&report)? + "\n")
}

fn main() {
    let root = repo_root();
    let endpoint = root.join("codeweaver-hermes-lab/dist/runners/acp-server.js");
    if !endpoint.exists() {
        eprintln!("skipped: {} is not built", endpoint.display());
        return;
    }
    if std::env::var("MINIMAX_CN_API_KEY").is_err() {
        eprintln!("skipped: MINIMAX_CN_API_KEY is not set; nothing to probe.");
        return;
    }

    let started_at = iso_timestamp();
    let nonce = fresh_nonce();
    // The scratch lives in the workspace's ignored build tree rather than in the system temp
    // directory: a probe must not depend on a writable temp directory, and it must not leave
    // anything behind in the repository either.
    let scratch = root
        .join("editor")
        .join("target")
        .join("acp-probe")
        .join(format!("run-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("the scratch directory is created");
    let workspace = make_workspace(&scratch, &nonce);
    let data_dir = scratch.join("data");
    fs::create_dir_all(&data_dir).expect("the data directory is created");

    // The endpoint is the process the editor spawns. It runs the product's primary runtime; the
    // reference runtime is pointed at a closed port so that nothing can quietly answer for it.
    let config = endpoint_config(
        &endpoint,
        &data_dir,
        &workspace,
        &scratch.join("worktrees"),
        "pi-local",
    );

    // The control arm comes first: it costs no tokens and it decides whether the main arm means
    // anything.
    let (control, refusal) = control_arm(&endpoint, &workspace, &scratch);

    let client = Client::start(&config).expect("the agent endpoint starts");
    let session = client.new_session(&workspace).expect("the session opens");
    let (panel, events) = run_turn(
        &client,
        &session.session_id,
        "Read the file nonce.txt in your working directory with the read_file tool, then reply with \
         its exact content and nothing else.",
        Duration::from_secs(180),
    );

    let tool_titles: Vec<String> = panel.tools.iter().map(|tool| tool.title.clone()).collect();
    // The journal is what says why a run ended, so it has to be complete before it is read.
    let journal_terminal = await_terminal_journal(&data_dir, Duration::from_secs(15));
    let main = judge(
        &panel.transcript,
        &tool_titles,
        panel.last_stop_reason,
        &nonce,
    );
    let control_passed = control.passed;
    let mut reasons = main.reasons.clone();
    reasons.extend(control.reasons.clone());
    let verdict = Verdict {
        passed: main.passed && control.passed,
        reasons,
    };
    let finished_at = iso_timestamp();

    let mut evidence = vec![
        format!("client=codeweaver-agent-acp (the editor's own client)"),
        format!("endpoint={}", endpoint.display()),
        format!("runtime=pi-local (named through CODEWEAVER_ACP_RUNTIME_ID)"),
        format!("model=MiniMax-M3 (provider minimax-cn)"),
        format!("session={}", session.session_id),
        format!("events={events}"),
        format!("tools={} ({})", tool_titles.len(), tool_titles.join(", ")),
        format!("stopReason={:?}", panel.last_stop_reason),
        format!(
            "control=refused ({})",
            refusal.as_deref().unwrap_or("the endpoint refused nothing")
        ),
        format!(
            "journalTerminal={}",
            if journal_terminal {
                "reached"
            } else {
                "missing"
            }
        ),
        format!("transcriptHasNonce={}", panel.transcript.contains(&nonce)),
        format!("transcript={}", panel.transcript.trim()),
        format!(
            "zedVersion={} (client/PIN.json — the client base this editor is built on)",
            zed_version(&root)
        ),
    ];
    if let Some(error) = &panel.error {
        evidence.push(format!("panelError={error}"));
    }
    let journal = journal_evidence(&data_dir);
    evidence.extend(journal.clone());

    // A provider that refused the call for lack of quota did not contradict the check; it
    // prevented it. That is `blocked`, not `failed` (D-020) and certainly not `passed`. A broken
    // control arm is not the environment's fault, though: that stays `failed`.
    let blocked = !verdict.passed && control_passed && is_environment_refusal(&journal);
    let status = if verdict.passed {
        "passed"
    } else if blocked {
        "blocked"
    } else {
        "failed"
    };
    let notes = if verdict.passed {
        None
    } else if blocked {
        Some(format!(
            "blocked: the model provider refused the call for lack of quota, so the editor path could \
             not be verified this time. The control arm ran and passed (the endpoint refused the \
             unregistered runtime '{}'). The turn's own reasons: {}",
            UNKNOWN_RUNTIME,
            main.reasons.join("; ")
        ))
    } else {
        Some(verdict.reasons.join("; "))
    };

    let report_path = root.join("codeweaver-hermes-lab/reports/phase0-acp-editor.json");
    write_report(
        &report_path,
        status,
        notes.as_deref(),
        &evidence,
        &started_at,
        &finished_at,
    )
    .expect("the report is written");
    if verdict.passed {
        let _ = fs::remove_dir_all(&scratch);
    } else {
        // A failed run keeps its scene: the journal that explains it is in there.
        println!("the scratch directory was kept: {}", scratch.display());
    }

    println!("Phase 0 ACP editor report: {}", report_path.display());
    for line in &evidence {
        println!("  {line}");
    }
    if verdict.passed {
        println!(
            "\nOK: the editor's ACP client reached the real runtime and the model did the work"
        );
    } else if blocked {
        // Blocked is not failed: the check did not happen, and the environment says why. Only a
        // contradicted check fails the probe.
        println!(
            "\nBLOCKED: the model provider refused the call for lack of quota; the editor path could \
             not be verified this time"
        );
    } else {
        eprintln!("\nFAIL: {}", verdict.reasons.join("; "));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<String> {
        vec!["read_file".to_string()]
    }

    #[test]
    fn a_turn_that_read_the_file_and_answered_passes() {
        let verdict = judge(
            "NONCE-ACP-1-2\n",
            &tools(),
            Some(StopReason::EndTurn),
            "NONCE-ACP-1-2",
        );
        assert!(verdict.passed, "{:?}", verdict.reasons);
    }

    #[test]
    fn a_transcript_without_the_nonce_is_not_a_pass() {
        let verdict = judge(
            "I could not read it.",
            &tools(),
            Some(StopReason::EndTurn),
            "NONCE-ACP-1-2",
        );
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("nonce"))
        );
    }

    #[test]
    fn a_turn_without_a_tool_call_is_not_a_pass() {
        let verdict = judge(
            "NONCE-ACP-1-2\n",
            &[],
            Some(StopReason::EndTurn),
            "NONCE-ACP-1-2",
        );
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("never called a tool"))
        );
    }

    #[test]
    fn a_turn_that_called_another_tool_is_not_a_pass() {
        let verdict = judge(
            "NONCE-ACP-1-2\n",
            &["search_code".to_string()],
            Some(StopReason::EndTurn),
            "NONCE-ACP-1-2",
        );
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("read_file"))
        );
    }

    #[test]
    fn a_turn_that_did_not_finish_is_not_a_pass() {
        let verdict = judge("NONCE-ACP-1-2\n", &tools(), None, "NONCE-ACP-1-2");
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("never finished"))
        );
    }

    #[test]
    fn a_refused_turn_is_not_a_pass() {
        let verdict = judge(
            "NONCE-ACP-1-2\n",
            &tools(),
            Some(StopReason::Refusal),
            "NONCE-ACP-1-2",
        );
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("not EndTurn"))
        );
    }

    #[test]
    fn the_timestamp_is_iso_8601_utc() {
        let stamp = iso_timestamp();
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert!(stamp.starts_with("20"), "{stamp}");
    }

    #[test]
    fn the_epoch_is_rendered_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn the_pin_version_is_read_through_a_utf8_bom() {
        assert_eq!(
            parse_zed_version("\u{feff}{\"zedVersion\":\"1.29.0\"}"),
            "1.29.0"
        );
        assert_eq!(parse_zed_version("{\"zedVersion\":\"1.29.0\"}"), "1.29.0");
    }

    #[test]
    fn a_pin_that_names_no_version_is_unknown() {
        assert_eq!(parse_zed_version("{}"), "unknown");
        assert_eq!(parse_zed_version("not json"), "unknown");
    }

    #[test]
    fn a_refusal_that_names_the_runtime_is_a_pass() {
        let verdict = judge_refusal(Some("Unknown runtime no-such-runtime"), "no-such-runtime");
        assert!(verdict.passed, "{:?}", verdict.reasons);
    }

    #[test]
    fn a_turn_that_ran_anyway_is_not_a_pass() {
        let verdict = judge_refusal(None, "no-such-runtime");
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("ran the task anyway"))
        );
    }

    #[test]
    fn a_refusal_that_names_nothing_is_not_a_pass() {
        let verdict = judge_refusal(
            Some("the snapshot could not be captured"),
            "no-such-runtime",
        );
        assert!(!verdict.passed);
        assert!(
            verdict
                .reasons
                .iter()
                .any(|reason| reason.contains("does not name the runtime"))
        );
    }

    #[test]
    fn a_passing_report_carries_no_note_and_a_failing_one_says_why() {
        let passing = build_report("passed", None, &["evidence".to_string()], "t0", "t1");
        let result = &passing["results"][0];
        assert_eq!(result["status"], "passed");
        // The aggregator validates the shape: a `notes: null` is a different thing from no note.
        assert!(
            result.get("notes").is_none(),
            "a passing result must not carry a note field: {}",
            result
        );

        let failing = build_report("failed", Some("no tool call"), &[], "t0", "t1");
        assert_eq!(failing["results"][0]["status"], "failed");
        assert_eq!(failing["results"][0]["notes"], "no tool call");

        // A blocked result is neither passed nor failed: the check did not happen.
        let blocked = build_report(
            "blocked",
            Some("the provider ran out of quota"),
            &[],
            "t0",
            "t1",
        );
        assert_eq!(blocked["results"][0]["status"], "blocked");
        assert_eq!(blocked["summary"]["blocked"], 1);
        assert_eq!(blocked["summary"]["failed"], 0);
        assert_eq!(blocked["summary"]["passed"], 0);
    }

    #[test]
    fn a_provider_quota_refusal_is_an_environment_refusal() {
        let journal = vec![
            "journal exec.jsonl execution.started {}".to_string(),
            "journal exec.jsonl execution.failed {\"error\":\"429 rate_limit_error\"}".to_string(),
        ];
        assert!(is_environment_refusal(&journal));
        // A failure the product caused is not the environment's fault.
        assert!(!is_environment_refusal(&[
            "journal exec.jsonl execution.failed {\"error\":\"the tool was denied\"}".to_string()
        ]));
        // A run that finished is not a refusal.
        assert!(!is_environment_refusal(&[
            "journal exec.jsonl execution.completed {}".to_string()
        ]));
    }
}
