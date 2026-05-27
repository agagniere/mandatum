use serde::Deserialize;

const DEFAULT_MAX_CONCURRENT: usize = 5;
const DEFAULT_RUNTIME: &str = "bash";
const DEFAULT_DOCKER_IMAGE: &str = "mandatum-agent:latest";

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Transition {
    pub success: Option<String>,
    pub failure: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AgentRoleConfig {
    pub role: String,
    #[serde(rename = "type", default = "default_agent_type")]
    pub agent_type: String,
    pub additional_instructions: Option<String>,
    pub max_concurrent: Option<usize>,
    pub caveman: Option<bool>,
    /// Claude model alias (`sonnet`, `opus`, `haiku`) or full name.
    pub model: Option<String>,
    /// Claude effort level (`low`, `medium`, `high`, `xhigh`, `max`).
    pub effort: Option<String>,
    #[serde(default)]
    pub transition: Transition,
    /// Inline role prompt passed to run-generic.sh as MANDATUM_ROLE_PROMPT.
    /// Takes priority over prompt_name. Supports $MANDATUM_SUCCESS_STATUS /
    /// $MANDATUM_FAILURE_STATUS substitution.
    pub prompt: Option<String>,
    /// Name of a prompt file at {agents_dir}/prompt_{name}.txt.
    /// Ignored when prompt is also set.
    pub prompt_name: Option<String>,
    /// Pre-fetch changes_requested activity and inject as review context before calling Claude.
    pub fetch_review_context: Option<bool>,
}

fn default_agent_type() -> String { "claude".to_string() }
fn default_agents_dir() -> String { "agents".to_string() }
fn default_max_concurrent() -> usize { DEFAULT_MAX_CONCURRENT }
fn default_caveman() -> bool { true }
fn default_runtime() -> String { DEFAULT_RUNTIME.to_string() }
fn default_docker_image() -> String { DEFAULT_DOCKER_IMAGE.to_string() }

#[derive(Deserialize, Clone, Debug, Default)]
pub struct MandatumConfig {
    pub project_dir: Option<String>,
    #[serde(default = "default_agents_dir")]
    pub agents_dir: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    #[serde(default = "default_caveman")]
    pub caveman: bool,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_docker_image")]
    pub docker_image: String,
    /// Shell command whose stdout is forwarded to the agent as
    /// `ANTHROPIC_AUTH_TOKEN`. Run once per spawn so tokens are always fresh.
    pub auth_token_helper: Option<String>,
    /// Multi-line headers forwarded as `ANTHROPIC_CUSTOM_HEADERS`.
    pub anthropic_custom_headers: Option<String>,
    /// Default claude model — overridden by per-role `model`.
    pub model: Option<String>,
    /// Default claude effort level — overridden by per-role `effort`.
    pub effort: Option<String>,
    #[serde(default)]
    pub agents: Vec<AgentRoleConfig>,
}

impl MandatumConfig {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&content)?)
    }

    pub fn role_config(&self, role: &str) -> Option<&AgentRoleConfig> {
        self.agents.iter().find(|a| a.role == role)
    }

    /// The first role in the pipeline — used as the default status for new tasks.
    pub fn first_role(&self) -> Option<&str> {
        self.agents.first().map(|a| a.role.as_str())
    }

    /// Status to set when this role's agent succeeds.
    pub fn success_for(&self, role: &str) -> Option<&str> {
        self.role_config(role)?.transition.success.as_deref()
    }

    /// Status to set when this role's agent fails / requests changes.
    pub fn failure_for(&self, role: &str) -> Option<&str> {
        self.role_config(role)?.transition.failure.as_deref()
    }

    pub fn agent_type(&self, role: &str) -> &str {
        self.role_config(role)
            .map(|a| a.agent_type.as_str())
            .unwrap_or("claude")
    }

    pub fn additional_instructions(&self, role: &str) -> &str {
        self.role_config(role)
            .and_then(|a| a.additional_instructions.as_deref())
            .unwrap_or("")
    }

    pub fn max_concurrent_for_role(&self, role: &str) -> usize {
        self.role_config(role)
            .and_then(|a| a.max_concurrent)
            .unwrap_or(self.max_concurrent)
    }

    pub fn caveman_for_role(&self, role: &str) -> bool {
        self.role_config(role)
            .and_then(|a| a.caveman)
            .unwrap_or(self.caveman)
    }

    pub fn model_for_role(&self, role: &str) -> Option<String> {
        self.role_config(role)
            .and_then(|a| a.model.clone())
            .or_else(|| self.model.clone())
    }

    pub fn effort_for_role(&self, role: &str) -> Option<String> {
        self.role_config(role)
            .and_then(|a| a.effort.clone())
            .or_else(|| self.effort.clone())
    }

    pub fn prompt_for_role(&self, role: &str) -> Option<&str> {
        self.role_config(role).and_then(|a| a.prompt.as_deref())
    }

    pub fn fetch_review_context_for_role(&self, role: &str) -> bool {
        self.role_config(role)
            .and_then(|a| a.fetch_review_context)
            .unwrap_or(false)
    }
}
