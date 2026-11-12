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

/// What the probe concluded about the turn.
#[derive(Debug, PartialEq)]
pub struct Verdict {
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// Judges one editor turn.
///
/// Three things have to hold, and each of them can fail on its own: the agent really called a
/// tool (an answer that names a random string cannot come from nowhere), the turn finished
/// (EndTurn), and the transcript carries the nonce that only exists in the workspace file.
pub fn judge(
    transcript: &str,
    tool_titles: &[String],
    stop_reason: Option<StopReason>,
    nonce: &str,
) -> Verdict {
    let mut reasons: Vec<String> = Vec::new();

    if tool_titles.is_empty() {
        reasons.push("the agent never called a tool, so the answer could not have come from the file".into());
    }
    match stop_reason {
        Some(StopReason::EndTurn) => {}
        Some(other) => reasons.push(format!("the turn ended with {other:?}, not EndTurn")),
        None => reasons.push("the turn never finished".into()),
    }
    if !transcript.contains(nonce) {
        reasons.push("the transcript does not carry the nonce that only exists in the workspace file".into());
    }

    Verdict {
        passed: reasons.is_empty(),
        reasons,
    }
}

/// ISO-8601 UTC, to the millisecond.
fn iso_timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is after 1970");
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
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("the clock is after 1970");
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
    fs::write(workspace.join("nonce.txt"), format!("{nonce}\n")).expect("the nonce file is written");
    run_git(&["init", "-b", "main"], &workspace);
    run_git(&["config", "user.name", "CodeWeaver Probe"], &workspace);
    run_git(&["config", "user.email", "probe@codeweaver.local"], &workspace);
    run_git(&["add", "-A"], &workspace);
    run_git(&["commit", "-m", "initial"], &workspace);
    workspace
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
        .and_then(|value| value.get("zedVersion").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn write_report(
    path: &Path,
    verdict: &Verdict,
    evidence: &[String],
    started_at: &str,
    finished_at: &str,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let report = serde_json::json!({
        "schemaVersion": "1.0",
        "generatedAt": finished_at,
        "summary": {
            "total": 1,
            "passed": if verdict.passed { 1 } else { 0 },
            "failed": if verdict.passed { 0 } else { 1 },
            "blocked": 0,
            "notRun": 0
        },
        "results": [{
            "id": "P0-004",
            "title": "ACP editor connection",
            "status": if verdict.passed { "passed" } else { "failed" },
            "startedAt": started_at,
            "finishedAt": finished_at,
            "evidence": evidence,
            "notes": if verdict.passed {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(verdict.reasons.join("; "))
            }
        }]
    });
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
    let config = codeweaver_agent_acp::EndpointConfig::new(
        "node",
        vec![endpoint.to_string_lossy().to_string()],
    )
    .with_env("CODEWEAVER_DATA_DIR", data_dir.to_string_lossy().to_string())
    .with_env("CODEWEAVER_REPOSITORY", workspace.to_string_lossy().to_string())
    .with_env("CODEWEAVER_WORKTREE_ROOT", scratch.join("worktrees").to_string_lossy().to_string())
    .with_env("HERMES_HTTP_BASE_URL", "http://127.0.0.1:9")
    .with_env("PI_MODEL", "MiniMax-M3")
    .with_env("CODEWEAVER_ACP_RUNTIME_ID", "pi-local");

    let client = Client::start(&config).expect("the agent endpoint starts");
    let session = client
        .new_session(&workspace)
        .expect("the session opens");

    let mut panel = AgentPanel::new();
    panel.begin_turn();
    client
        .start_prompt(
            &session.session_id,
            "Read the file nonce.txt in your working directory with the read_file tool, then reply with \
             its exact content and nothing else.",
        )
        .expect("the prompt is written");

    let deadline = Instant::now() + Duration::from_secs(180);
    let mut events = 0usize;
    while Instant::now() < deadline {
        for event in client.drain_events() {
            panel.apply(event);
            events += 1;
        }
        if panel.last_stop_reason.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let tool_titles: Vec<String> = panel.tools.iter().map(|tool| tool.title.clone()).collect();
    let verdict = judge(&panel.transcript, &tool_titles, panel.last_stop_reason, &nonce);
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
        format!("transcriptHasNonce={}", panel.transcript.contains(&nonce)),
        format!("transcript={}", panel.transcript.trim()),
        format!("zedVersion={} (client/PIN.json — the client base this editor is built on)", zed_version(&root)),
    ];
    if let Some(error) = &panel.error {
        evidence.push(format!("panelError={error}"));
    }

    let report_path = root.join("codeweaver-hermes-lab/reports/phase0-acp-editor.json");
    write_report(&report_path, &verdict, &evidence, &started_at, &finished_at)
        .expect("the report is written");
    let _ = fs::remove_dir_all(&scratch);

    println!("Phase 0 ACP editor report: {}", report_path.display());
    for line in &evidence {
        println!("  {line}");
    }
    if verdict.passed {
        println!("\nOK: the editor's ACP client reached the real runtime and the model did the work");
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
        let verdict = judge("I could not read it.", &tools(), Some(StopReason::EndTurn), "NONCE-ACP-1-2");
        assert!(!verdict.passed);
        assert!(verdict.reasons.iter().any(|reason| reason.contains("nonce")));
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
        assert!(verdict.reasons.iter().any(|reason| reason.contains("never called a tool")));
    }

    #[test]
    fn a_turn_that_did_not_finish_is_not_a_pass() {
        let verdict = judge("NONCE-ACP-1-2\n", &tools(), None, "NONCE-ACP-1-2");
        assert!(!verdict.passed);
        assert!(verdict.reasons.iter().any(|reason| reason.contains("never finished")));
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
        assert!(verdict.reasons.iter().any(|reason| reason.contains("not EndTurn")));
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
}
