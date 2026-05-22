use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::config::MandatumConfig;
use crate::AppState;

pub struct Spawner {
    config: Arc<MandatumConfig>,
    state: Arc<AppState>,
    running_per_role: Arc<Mutex<HashMap<String, usize>>>,
    log_dir: PathBuf,
}

impl Spawner {
    pub fn new(config: Arc<MandatumConfig>, state: Arc<AppState>, log_dir: PathBuf) -> Self {
        Self {
            config,
            state,
            running_per_role: Arc::new(Mutex::new(HashMap::new())),
            log_dir,
        }
    }

    pub async fn run(self: Arc<Self>) {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            self.check_and_spawn().await;
        }
    }

    async fn check_and_spawn(&self) {
        let roles: Vec<String> = self.config.agents.iter().map(|a| a.role.clone()).collect();
        for role in roles.iter().map(|s| s.as_str()) {
            let max = self.config.max_concurrent_for_role(role);
            let running = *self
                .running_per_role
                .lock()
                .unwrap()
                .get(role)
                .unwrap_or(&0);
            let slots = max.saturating_sub(running);
            if slots == 0 {
                continue;
            }
            match self.state.db.count_available_tasks_for_role(role).await {
                Ok(0) => {}
                Ok(available) => {
                    for _ in 0..slots.min(available) {
                        self.spawn_agent(role).await;
                    }
                }
                Err(e) => tracing::warn!("DB check for role {} failed: {}", role, e),
            }
        }
    }

    async fn spawn_agent(&self, role: &str) {
        let agent_type = self.config.agent_type(role).to_string();
        let short_id = &Uuid::new_v4().to_string()[..8].to_string();
        let agent_id = format!("{}-{}", role, short_id);

        let script_name = format!("run-{}.sh", role);
        let script_path = PathBuf::from(&self.config.agents_dir)
            .join(&agent_type)
            .join(&script_name);

        if !script_path.exists() {
            tracing::warn!(
                "Agent script not found: {} — cannot spawn {} agent",
                script_path.display(),
                role
            );
            return;
        }

        let project_dir = self
            .state
            .repo_path
            .clone()
            .or_else(|| self.config.project_dir.clone())
            .unwrap_or_else(|| ".".to_string());

        let base_instructions = self.config.additional_instructions(role);
        let additional = if self.config.caveman_for_role(role) {
            let caveman = "Respond terse like caveman. Drop articles, filler words, \
                pleasantries, hedging. Fragments OK. Short synonyms preferred. \
                Technical terms exact. Code blocks unchanged.";
            if base_instructions.is_empty() {
                caveman.to_string()
            } else {
                format!("{}\n\n{}", caveman, base_instructions)
            }
        } else {
            base_instructions.to_string()
        };

        let log_path = self.log_dir.join(format!("{}.log", agent_id));
        let log_file = match tokio::fs::File::create(&log_path).await {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(
                    "Cannot create log file {}: {}",
                    log_path.display(),
                    e
                );
                return;
            }
        };

        let model = self.config.model_for_role(role);
        let effort = self.config.effort_for_role(role);

        let mut cmd = match self.config.runtime.as_str() {
            "docker" => match build_docker_command(
                &self.config,
                &agent_id,
                &agent_type,
                &script_name,
                &project_dir,
                &additional,
                model.as_deref(),
                effort.as_deref(),
                self.config.success_for(role),
                self.config.failure_for(role),
            ) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        "Cannot build docker command for {}: {}",
                        agent_id,
                        e
                    );
                    return;
                }
            },
            _ => {
                let mut c = Command::new("bash");
                c.arg(&script_path)
                    .env("AGENT_ID", &agent_id)
                    .env("PROJECT_DIR", &project_dir)
                    .env("MANDATUM_ONCE", "1")
                    .env("ADDITIONAL_INSTRUCTIONS", &additional);
                if let Some(ref m) = model {
                    c.env("MANDATUM_MODEL", m);
                }
                if let Some(ref e) = effort {
                    c.env("MANDATUM_EFFORT", e);
                }
                if let Some(s) = self.config.success_for(role) {
                    c.env("MANDATUM_SUCCESS_STATUS", s);
                }
                if let Some(f) = self.config.failure_for(role) {
                    c.env("MANDATUM_FAILURE_STATUS", f);
                }
                c
            }
        };
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to spawn agent {}: {}", agent_id, e);
                return;
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        {
            let mut map = self.running_per_role.lock().unwrap();
            *map.entry(role.to_string()).or_insert(0) += 1;
        }

        tracing::info!(
            "Spawned {} agent {} for role {}",
            agent_type,
            agent_id,
            role
        );

        let broadcaster = self.state.broadcaster.clone();
        let state_for_exit = Arc::clone(&self.state);
        let agent_id_clone = agent_id.clone();
        let role_counter = Arc::clone(&self.running_per_role);
        let role_str = role.to_string();

        // Channel merges stdout + stderr into a single ordered stream.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let tx_err = tx.clone();

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx_err.send(line);
            }
        });

        tokio::spawn(async move {
            let mut log_writer =
                tokio::io::BufWriter::new(log_file);

            while let Some(line) = rx.recv().await {
                let ts = Utc::now().to_rfc3339();
                let _ = log_writer.write_all(line.as_bytes()).await;
                let _ = log_writer.write_all(b"\n").await;
                let _ = log_writer.flush().await;
                broadcaster.broadcast(
                    serde_json::json!({
                        "event": "agent_log",
                        "data": {
                            "agent_id": agent_id_clone,
                            "line": line,
                            "ts": ts
                        }
                    })
                    .to_string(),
                );
            }

            let _ = child.wait().await;

            // Mark inactive immediately so the UI doesn't keep showing the
            // agent as live until the 5-minute heartbeat threshold expires.
            if let Err(e) = state_for_exit.db.mark_agent_inactive(&agent_id_clone).await {
                tracing::warn!("Failed to mark agent {} inactive: {}", agent_id_clone, e);
            }
            broadcaster.broadcast(
                serde_json::json!({
                    "event": "agent_updated",
                    "data": {"agent_id": agent_id_clone}
                })
                .to_string(),
            );

            let mut map = role_counter.lock().unwrap();
            let count = map.entry(role_str).or_insert(0);
            if *count > 0 {
                *count -= 1;
            }
            tracing::info!("Agent {} exited", agent_id_clone);
        });
    }
}

/// Build a `docker run` command that executes the role's bash script inside
/// the configured agent image. The target project and the agents directory
/// are mounted at the same absolute path inside the container as on the host
/// so that any absolute paths git stores (e.g. worktree gitdirs) resolve in
/// both contexts. The container reaches mandatum's REST and MCP ports via
/// host.docker.internal.
fn build_docker_command(
    config: &MandatumConfig,
    agent_id: &str,
    agent_type: &str,
    script_name: &str,
    project_dir: &str,
    additional: &str,
    model: Option<&str>,
    effort: Option<&str>,
    success_status: Option<&str>,
    failure_status: Option<&str>,
) -> Result<Command, std::io::Error> {
    let agents_dir_abs = std::fs::canonicalize(&config.agents_dir)?;
    let project_dir_abs = std::fs::canonicalize(project_dir)?;
    let project_dir_str = project_dir_abs.display().to_string();
    let agents_dir_str = agents_dir_abs.display().to_string();
    let container_script = format!("{}/{}/{}", agents_dir_str, agent_type, script_name);
    let mcp_config_path = format!("{}/claude/mcp-config-docker.json", agents_dir_str);

    let mut cmd = Command::new("docker");
    cmd.args([
        "run",
        "--rm",
        "--name",
        agent_id,
        "--add-host=host.docker.internal:host-gateway",
        "-w",
        &project_dir_str,
        "-e",
        &format!("AGENT_ID={agent_id}"),
        "-e",
        &format!("PROJECT_DIR={project_dir_str}"),
        "-e",
        "MANDATUM_ONCE=1",
        "-e",
        &format!("ADDITIONAL_INSTRUCTIONS={additional}"),
        "-e",
        "MANDATUM_REST_URL=http://host.docker.internal:3001",
        "-e",
        "MANDATUM_MCP_URL=http://host.docker.internal:3002",
        "-e",
        &format!("MCP_CONFIG={mcp_config_path}"),
        "-e",
        "LOG_DIR=/tmp/agent-logs",
        // Tells claude that running as root is OK inside this container.
        // Undocumented but stable since 2024; the image is a sealed sandbox.
        "-e",
        "IS_SANDBOX=1",
        "-v",
        &format!("{project_dir_str}:{project_dir_str}"),
        "-v",
        &format!("{agents_dir_str}:{agents_dir_str}:ro"),
    ]);

    // Forward Anthropic credentials. Precedence:
    //   ANTHROPIC_AUTH_TOKEN     → from auth_token_helper (fresh per spawn)
    //                             or, fallback, from the host env
    //   ANTHROPIC_API_KEY        → from host env only (static sk-ant-… keys)
    //   ANTHROPIC_CUSTOM_HEADERS → from mandatum.yaml first, else host env
    if let Ok(val) = std::env::var("ANTHROPIC_API_KEY") {
        cmd.args(["-e", &format!("ANTHROPIC_API_KEY={val}")]);
    }
    let token = match &config.auth_token_helper {
        Some(helper) => run_auth_helper(helper)
            .or_else(|_| std::env::var("ANTHROPIC_AUTH_TOKEN"))
            .ok(),
        None => std::env::var("ANTHROPIC_AUTH_TOKEN").ok(),
    };
    if let Some(val) = token {
        cmd.args(["-e", &format!("ANTHROPIC_AUTH_TOKEN={val}")]);
    }
    let headers = config
        .anthropic_custom_headers
        .clone()
        .or_else(|| std::env::var("ANTHROPIC_CUSTOM_HEADERS").ok());
    if let Some(val) = headers {
        cmd.args(["-e", &format!("ANTHROPIC_CUSTOM_HEADERS={val}")]);
    }
    if let Some(m) = model {
        cmd.args(["-e", &format!("MANDATUM_MODEL={m}")]);
    }
    if let Some(e) = effort {
        cmd.args(["-e", &format!("MANDATUM_EFFORT={e}")]);
    }
    if let Some(s) = success_status {
        cmd.args(["-e", &format!("MANDATUM_SUCCESS_STATUS={s}")]);
    }
    if let Some(f) = failure_status {
        cmd.args(["-e", &format!("MANDATUM_FAILURE_STATUS={f}")]);
    }
    // Always point the container at the host-side reverse proxy. Container
    // networking can't reach VPN-routed gateways directly, so claude calls
    // http://host.docker.internal:3003/... and the host's mitmdump (started
    // via `make proxy`) re-issues the request over TLS via the host's VPN.
    // The user's host-side ANTHROPIC_BASE_URL is deliberately ignored here.
    cmd.args([
        "-e",
        "ANTHROPIC_BASE_URL=http://host.docker.internal:3003",
    ]);

    cmd.arg(&config.docker_image)
        .arg("bash")
        .arg(&container_script);

    Ok(cmd)
}

/// Run a shell command and return its trimmed stdout. Used by the spawner
/// to refresh the bearer token immediately before each `docker run`.
fn run_auth_helper(command: &str) -> Result<String, std::io::Error> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("auth_token_helper exited {}: {}", output.status, stderr.trim());
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "auth_token_helper failed",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
