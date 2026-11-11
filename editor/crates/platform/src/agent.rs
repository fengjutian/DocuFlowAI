//! The editor's agent session.
//!
//! The session owns the ACP client, the ACP session id and the panel that
//! shows the agent's output. It is created lazily, on the first prompt, so
//! that starting the editor does not spawn a runtime the user never asks for.
//!
//! Nothing here blocks the event loop: a prompt is started with
//! [`Client::start_prompt`], and the agent's output is collected by draining
//! the event stream once per frame.

use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

use codeweaver_agent_acp::{Client, EndpointConfig, PermissionOutcome};
use codeweaver_agent_ui::AgentPanel;

/// How the editor finds the agent endpoint.
///
/// The endpoint is a separate process, so the editor is told how to start it
/// through the environment. Without those variables the agent panel stays
/// empty and says so, rather than guessing a path.
#[derive(Clone, Debug)]
pub struct EndpointSettings {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl EndpointSettings {
    /// Reads the endpoint settings from the environment.
    ///
    /// `CODEWEAVER_ACP_COMMAND` names the program and `CODEWEAVER_ACP_ARGS`
    /// holds its arguments, separated by spaces. The session's working
    /// directory comes from `CODEWEAVER_ACP_CWD`, and defaults to the
    /// editor's own working directory.
    pub fn from_env() -> Option<Self> {
        let program = env::var("CODEWEAVER_ACP_COMMAND").ok()?;
        let args = env::var("CODEWEAVER_ACP_ARGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let cwd = env::var("CODEWEAVER_ACP_CWD")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        Some(EndpointSettings { program, args, cwd })
    }
}

/// The agent session shown in the panel.
pub struct AgentSession {
    client: Client,
    session_id: String,
    panel: AgentPanel,
    started_at: Instant,
}

impl AgentSession {
    /// Starts the endpoint and opens a session.
    pub fn start(settings: &EndpointSettings) -> Result<Self, String> {
        let config = EndpointConfig::new(settings.program.clone(), settings.args.clone());
        let client = Client::start(&config).map_err(|error| error.to_string())?;
        let session = client
            .new_session(&settings.cwd)
            .map_err(|error| error.to_string())?;
        Ok(AgentSession {
            client,
            session_id: session.session_id,
            panel: AgentPanel::new(),
            started_at: Instant::now(),
        })
    }

    /// The panel to draw.
    pub fn panel(&self) -> &AgentPanel {
        &self.panel
    }

    /// Starts a turn with the given prompt.
    pub fn start_prompt(&mut self, prompt: &str) -> Result<(), String> {
        self.panel.begin_turn();
        self.started_at = Instant::now();
        self.client
            .start_prompt(&self.session_id, prompt)
            .map_err(|error| error.to_string())
    }

    /// Cancels the running turn.
    pub fn cancel(&mut self) -> Result<(), String> {
        self.client
            .cancel(&self.session_id)
            .map_err(|error| error.to_string())
    }

    /// Answers the pending permission request.
    pub fn answer_permission(&mut self, option_id: &str) -> Result<(), String> {
        let Some(pending) = self.panel.pending_permission.clone() else {
            return Err("no permission request is waiting".to_owned());
        };
        self.client
            .respond_to_permission(pending.request_id, PermissionOutcome::selected(option_id))
            .map_err(|error| error.to_string())?;
        self.panel.pending_permission = None;
        Ok(())
    }

    /// Collects the agent's output.
    ///
    /// The editor calls this once per frame, so the agent's output appears
    /// while it is still arriving.
    pub fn pump(&mut self) -> bool {
        let events = self.client.drain_events();
        if events.is_empty() {
            return false;
        }
        for event in events {
            self.panel.apply(event);
        }
        true
    }

    /// Whether the agent is still working.
    pub fn is_running(&self) -> bool {
        self.panel.running
    }

    /// How long the current turn has been running.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// A short description of the session state, for the panel header.
pub fn session_summary(session: Option<&AgentSession>) -> String {
    match session {
        Some(session) => {
            let panel = session.panel();
            let state = if panel.running {
                format!("running {:.1}s", session.elapsed().as_secs_f64())
            } else if let Some(reason) = panel.last_stop_reason {
                format!("{reason:?}")
            } else {
                "idle".to_owned()
            };
            format!("CodeWeaver Agent · {state}")
        }
        None => "CodeWeaver Agent · not configured".to_owned(),
    }
}
