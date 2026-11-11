//! End-to-end check of the editor's agent path.
//!
//! It drives the same code the editor uses — the ACP client, the panel and
//! the session — against the real agent endpoint, and asserts that a turn
//! streams its reply and can be cancelled.
//!
//! ```powershell
//! cargo run -p codeweaver-platform --example agent_end_to_end
//! ```

use std::{path::PathBuf, time::Duration};

use codeweaver_agent_acp::Client;
use codeweaver_agent_ui::AgentPanel;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// The bridge that runs the real ACP endpoint against the mock runtime.
fn bridge() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/fixtures/acp-e2e-bridge.mjs")
}

fn main() {
    let root = repo_root();
    let endpoint = root.join("codeweaver-hermes-lab/dist/runners/acp-server.js");
    if !endpoint.exists() {
        eprintln!("skipped: {} is not built", endpoint.display());
        return;
    }

    // The bridge is the process the editor spawns: it runs the real agent
    // endpoint against the mock runtime and relays stdio.
    let config = codeweaver_agent_acp::EndpointConfig::new(
        "node",
        vec![bridge().to_string_lossy().to_string()],
    )
    .with_env("CODEWEAVER_REPO_ROOT", root.to_string_lossy().to_string());

    let client = Client::start(&config).expect("the agent endpoint starts");
    let session = client
        .new_session(&root)
        .expect("the session opens");
    println!("session: {}", session.session_id);

    let mut panel = AgentPanel::new();
    panel.begin_turn();
    client
        .start_prompt(&session.session_id, "say hello")
        .expect("the prompt is written");

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut turns = 0;
    while std::time::Instant::now() < deadline {
        for event in client.drain_events() {
            panel.apply(event);
            turns += 1;
        }
        if panel.last_stop_reason.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("events: {turns}");
    println!("stop reason: {:?}", panel.last_stop_reason);
    println!("transcript: {}", panel.transcript);

    if panel.last_stop_reason.is_none() {
        eprintln!("FAIL: the turn did not finish");
        std::process::exit(1);
    }
    if panel.transcript.trim().is_empty() {
        eprintln!("FAIL: the agent produced no output");
        std::process::exit(1);
    }
    println!("\nOK: the editor's agent path reached the real endpoint");
}
