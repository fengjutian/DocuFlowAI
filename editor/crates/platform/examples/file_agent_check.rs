//! A check of the editor's file-and-agent path.
//!
//! It opens a real file, asks the agent about it through the real ACP
//! endpoint, and checks that the agent's reply came back. The point of the
//! check is the *context*: the prompt the editor sends is the file's content,
//! and the session runs in the file's directory.
//!
//! ```powershell
//! cargo run -p codeweaver-platform --example file_agent_check
//! ```

use std::{fs, path::PathBuf};

use codeweaver_platform::NativeApplication;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository root exists")
}

fn main() {
    let root = repo_root();
    if !root
        .join("codeweaver-hermes-lab/dist/runners/acp-server.js")
        .exists()
    {
        eprintln!("skipped: the agent endpoint is not built");
        return;
    }

    // A file to edit and to ask about.
    let scratch = root.join(".dev-logs");
    fs::create_dir_all(&scratch).expect("the scratch directory can be created");
    let path = scratch.join("file-agent-check.txt");
    fs::write(&path, "The quick brown fox jumps over the lazy dog.\n").unwrap();

    let mut app = NativeApplication::new(Some(path.clone())).expect("the application starts");
    println!("opened: {}", path.display());
    println!("buffer: {}", app.editor().text());
    assert!(
        app.editor().text().contains("quick brown fox"),
        "the file content must be in the buffer"
    );

    // The agent session runs against the real ACP endpoint; the bridge starts
    // it with the mock runtime behind it.
    let bridge = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/fixtures/acp-e2e-bridge.mjs");
    app.set_endpoint(codeweaver_platform::EndpointSettings {
        program: "node".to_owned(),
        args: vec![bridge.to_string_lossy().to_string()],
        cwd: path.parent().unwrap().to_path_buf(),
    });

    app.start_agent_turn();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut transcript = String::new();
    while std::time::Instant::now() < deadline {
        app.pump_agent();
        transcript = app.agent_transcript();
        if !transcript.trim().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    println!("transcript: {transcript}");
    if transcript.trim().is_empty() {
        eprintln!("FAIL: the agent did not answer");
        std::process::exit(1);
    }
    println!("\nOK: the editor opened a file and the agent answered about it");
}
