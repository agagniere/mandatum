use serde_json::Value;
use std::sync::Arc;
use crate::config::MandatumConfig;
use crate::db::{Database, suggest_branch};
use crate::metrics::Metrics;
use crate::sse::SseBroadcaster;
use tracing::{info, warn};

pub struct ToolContext {
    pub db: Database,
    pub broadcaster: SseBroadcaster,
    pub repo_path: Option<String>,
    pub base_branch: String,
    pub metrics: Arc<Metrics>,
    pub config: Option<Arc<MandatumConfig>>,
}

pub async fn handle_tool_call(
    name: &str,
    arguments: Value,
    ctx: &ToolContext,
) -> Result<Value, String> {
    match name {
        "register_agent"    => register_agent(arguments, ctx).await,
        "get_next_task"     => get_next_task(arguments, ctx).await,
        "update_task_status"=> update_task_status(arguments, ctx).await,
        "add_task_comment"  => add_task_comment(arguments, ctx).await,
        "create_task"       => create_task(arguments, ctx).await,
        "list_tasks"        => list_tasks(arguments, ctx).await,
        "get_task"          => get_task(arguments, ctx).await,
        "set_output_path"   => set_output_path(arguments, ctx).await,
        "heartbeat"         => heartbeat(arguments, ctx).await,
        // git tools
        "create_branch"     => create_branch(arguments, ctx).await,
        "record_commit"     => record_commit(arguments, ctx).await,
        "request_review"    => request_review(arguments, ctx).await,
        "get_review_target" => get_review_target(arguments, ctx).await,
        "approve_review"    => approve_review(arguments, ctx).await,
        "request_changes"   => request_changes(arguments, ctx).await,
        "set_pr_url"        => set_pr_url(arguments, ctx).await,
        "setup_worktree"    => setup_worktree(arguments, ctx).await,
        _ => Err(format!("Unknown tool: {}", name)),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

async fn agent_role(db: &Database, agent_id: &str) -> Option<String> {
    db.list_agents().await.ok()
        .and_then(|agents| agents.into_iter().find(|a| a.agent_id == agent_id))
        .map(|a| a.role)
}

fn broadcast(ctx: &ToolContext, event: &str, data: &serde_json::Value) {
    ctx.broadcaster.broadcast(serde_json::json!({"event": event, "data": data}).to_string());
}

// ── original tools ────────────────────────────────────────────────────────────

async fn register_agent(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let role = args["role"].as_str().ok_or("Missing role")?;
    info!(agent_id, role, "agent registered");
    let agent = ctx.db.register_agent(agent_id, role).await.map_err(|e| e.to_string())?;
    broadcast(ctx, "agent_registered", &serde_json::json!(agent));
    Ok(serde_json::json!({
        "message": format!("Agent '{}' registered with role '{}'", agent_id, role),
        "agent": agent
    }))
}

async fn get_next_task(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let role = args["role"].as_str().ok_or("Missing role")?;

    if let Some(task) = ctx.db.get_active_task_for_agent(agent_id).await.map_err(|e| e.to_string())? {
        info!(agent_id, role, task_id = %task.id, "get_next_task: agent already has active task");
        return Ok(serde_json::json!({
            "message": "Agent already has an active task",
            "task": task
        }));
    }

    match ctx.db.get_next_task_for_role(agent_id, role).await.map_err(|e| e.to_string())? {
        None => {
            info!(agent_id, role, "get_next_task: no tasks available");
            ctx.metrics.task_poll_empty(role);
            Ok(serde_json::json!({
                "message": format!("No tasks available for role '{}'", role),
                "task": null
            }))
        }
        Some(task) => {
            info!(agent_id, role, task_id = %task.id, title = %task.title, "task assigned");
            ctx.metrics.task_claimed(role);
            let suggested_branch = suggest_branch(&task.id, &task.title);
            broadcast(ctx, "task_updated", &serde_json::json!(task));
            Ok(serde_json::json!({
                "message": "Task assigned",
                "task": task,
                "git_instructions": {
                    "suggested_branch": suggested_branch,
                    "commands": [
                        format!("git checkout -b {}", suggested_branch),
                        format!("# Work on: {}", task.title),
                        "# After changes: call record_commit with hash and message",
                        "# When done: call request_review"
                    ]
                }
            }))
        }
    }
}

async fn update_task_status(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let status = args["status"].as_str().ok_or("Missing status")?;
    let note = args["note"].as_str();

    let prev_status = ctx.db.get_task(task_id).await.ok().flatten().map(|t| t.status);

    let task = ctx.db.update_task(
        task_id, None, None, Some(status), None, None, None, None, None,
        None, None, None, None, None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    if status != "in_progress" {
        ctx.db.clear_agent_current_task_if_matches(agent_id, task_id)
            .await.map_err(|e| e.to_string())?;
    }

    info!(agent_id, task_id, status, "task status updated");
    let role = agent_role(&ctx.db, agent_id).await;
    let detail = note.map(|n| format!("Status → '{}': {}", status, n))
        .unwrap_or_else(|| format!("Status → '{}'", status));

    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "status_changed", Some(&detail))
        .await.map_err(|e| e.to_string())?;

    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));

    if let Some(ref from) = prev_status {
        if from != status {
            ctx.metrics.task_status_changed(from, status);
        }
    }
    if status == "done" {
        ctx.metrics.task_done(&task.title, &task.created_at);
        if let (Some(ref claimed_at), Some(ref role)) = (&task.claimed_at, &task.assigned_role) {
            if let Ok(claimed) = chrono::DateTime::parse_from_rfc3339(claimed_at) {
                let secs = chrono::Utc::now()
                    .signed_duration_since(claimed.with_timezone(&chrono::Utc))
                    .num_seconds();
                if secs >= 0 {
                    ctx.metrics.task_claim_duration_seconds(role, secs as f64);
                }
            }
        }
    }
    if let Ok(counts) = ctx.db.count_tasks_by_status().await {
        ctx.metrics.queue_sizes(&counts);
    }

    // Auto-merge when task is marked done and server has a repo configured
    if status == "done" {
        if let (Some(ref repo_path), Some(ref branch)) = (&ctx.repo_path, &task.branch_name) {
            let base = ctx.base_branch.clone();
            let merge_msg = format!("Merge '{}': {}", branch, task.title);
            match crate::git::merge_branch(repo_path, branch, &base, &merge_msg).await {
                Ok(hash) => {
                    info!(branch, base, commit = &hash[..hash.len().min(8)], "auto-merge succeeded");
                    let _ = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "merged",
                        Some(&format!("Merged '{}' into '{}' ({})", branch, base, &hash[..hash.len().min(8)])))
                        .await;
                    crate::git::cleanup_task_worktrees(repo_path, branch).await;
                }
                Err(e) => {
                    warn!(branch, base, task_id, error = %e, "auto-merge failed, task moved to blocked");
                    let _ = ctx.db.update_task(
                        task_id, None, None, Some("blocked"), None, None, None, None, None,
                        None, None, None, None, None, None,
                    ).await;
                    let _ = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "merge_failed",
                        Some(&format!("Auto-merge failed: {}", e)))
                        .await;
                    let updated = ctx.db.get_task(task_id).await.ok().flatten();
                    if let Some(ref t) = updated {
                        broadcast(ctx, "task_updated", &serde_json::json!(t));
                    }
                    return Err(format!("Auto-merge failed: {}", e));
                }
            }
        }
    }

    Ok(serde_json::json!({"task": task}))
}

async fn add_task_comment(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let comment = args["comment"].as_str().ok_or("Missing comment")?;
    let role = agent_role(&ctx.db, agent_id).await;
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "comment", Some(comment))
        .await.map_err(|e| e.to_string())?;
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({"message": "Comment added", "activity_id": entry.id}))
}

async fn create_task(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let title = args["title"].as_str().ok_or("Missing title")?;
    let description = args["description"].as_str();
    let priority = args["priority"].as_str().unwrap_or("medium");
    let assigned_role = args["assigned_role"].as_str();
    let branch_name = args["branch_name"].as_str()
        .filter(|s| !s.is_empty());
    let tags: Vec<String> = args["tags"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let raw_dependencies: Vec<String> = args["dependencies"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let mut dependencies = Vec::with_capacity(raw_dependencies.len());
    for dep in &raw_dependencies {
        let resolved = ctx.db.resolve_task_id(dep).await
            .map_err(|e| format!("Invalid dependency '{}': {}", dep, e))?;
        dependencies.push(resolved);
    }

    let task = ctx.db.create_task(Some(agent_id), title, description, priority, assigned_role, branch_name, &tags, &dependencies)
        .await.map_err(|e| e.to_string())?;
    broadcast(ctx, "task_created", &serde_json::json!(task));
    ctx.metrics.task_created();
    Ok(serde_json::json!({"task": task}))
}

async fn list_tasks(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let status = args["status"].as_str().map(|s| s.to_string());
    let assigned_role = args["assigned_role"].as_str().map(|s| s.to_string());
    let assigned_agent_id = args["assigned_agent_id"].as_str().map(|s| s.to_string());
    let tasks = ctx.db.list_tasks_summary(status, assigned_role, assigned_agent_id)
        .await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({"tasks": tasks}))
}

async fn get_task(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    match ctx.db.get_task_with_activity(task_id).await.map_err(|e| e.to_string())? {
        None => Err(format!("Task {} not found", task_id)),
        Some(t) => Ok(serde_json::json!({"task": t})),
    }
}

async fn set_output_path(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let output_path = args["output_path"].as_str().ok_or("Missing output_path")?;
    let task = ctx.db.update_task(
        task_id, None, None, None, None, None, None, Some(output_path), None,
        None, None, None, None, None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;
    let role = agent_role(&ctx.db, agent_id).await;
    ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "output_set",
        Some(&format!("Output: {}", output_path))).await.map_err(|e| e.to_string())?;
    broadcast(ctx, "task_updated", &serde_json::json!(task));
    Ok(serde_json::json!({"task": task}))
}

async fn heartbeat(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let ts = ctx.db.heartbeat(agent_id).await.map_err(|e| e.to_string())?;
    if let Ok(Some(agent)) = ctx.db.get_agent(agent_id).await {
        ctx.metrics.agent_heartbeat(&agent.role);
    }
    Ok(serde_json::json!({"message": "Heartbeat received", "agent_id": agent_id, "timestamp": ts}))
}

// ── git tools ─────────────────────────────────────────────────────────────────

/// Agent declares the branch it created locally.
async fn create_branch(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let branch_name = args["branch_name"].as_str().ok_or("Missing branch_name")?;
    let base_branch = args["base_branch"].as_str().unwrap_or(&ctx.base_branch);

    let task = ctx.db.update_task(
        task_id, None, None, None, None, None, None, None, None,
        Some(branch_name), Some(base_branch), None, None, None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    info!(agent_id, task_id, branch_name, base_branch, "branch created");
    let role = agent_role(&ctx.db, agent_id).await;
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "branch_created",
        Some(&format!("Branch: {} (base: {})", branch_name, base_branch)))
        .await.map_err(|e| e.to_string())?;

    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({
        "message": format!("Branch '{}' recorded for task", branch_name),
        "task": task
    }))
}

/// Agent records a git commit it made locally.
async fn record_commit(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let hash = args["hash"].as_str().ok_or("Missing hash")?;
    let message = args["message"].as_str().ok_or("Missing message")?;

    let commit = ctx.db.add_commit(task_id, Some(agent_id), hash, message)
        .await.map_err(|e| e.to_string())?;

    ctx.metrics.commit_created();
    info!(agent_id, task_id, hash, message, "commit recorded");
    let role = agent_role(&ctx.db, agent_id).await;
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "committed",
        Some(&format!("{} — {}", &hash[..hash.len().min(8)], message)))
        .await.map_err(|e| e.to_string())?;

    // re-fetch to get updated commit_count
    let task = ctx.db.get_task(task_id).await.map_err(|e| e.to_string())?;
    if let Some(ref t) = task {
        broadcast(ctx, "task_updated", &serde_json::json!(t));
    }
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({"message": "Commit recorded", "commit": commit}))
}

/// Move task to in_review, recording the commit that's ready for review.
async fn request_review(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let commit_hash = args["commit_hash"].as_str();
    let note = args["note"].as_str().unwrap_or("Ready for review");

    // Verify the commit actually exists in the repo before accepting the review.
    // This catches hallucinated commit hashes.
    if let (Some(new_hash), Some(ref repo_path)) = (commit_hash, &ctx.repo_path) {
        let check = tokio::process::Command::new("git")
            .args(["-C", repo_path, "cat-file", "-e", &format!("{}^{{commit}}", new_hash)])
            .output()
            .await
            .map_err(|e| format!("git cat-file failed: {}", e))?;
        if !check.status.success() {
            return Err(format!(
                "Commit {} does not exist in the repository. \
                 You must provide a real commit hash from your worktree. \
                 Run `git log --oneline -5` in your worktree to see recent commits.",
                &new_hash[..new_hash.len().min(12)]
            ));
        }
    }

    // When resubmitting after reviewer feedback, enforce that the note contains
    // a "Round N:" line for every prior changes_requested entry.
    {
        let activity = ctx.db.get_activity_for_task(task_id).await.unwrap_or_default();
        let last_blocked = activity.iter().rposition(|e| e.action == "blocked")
            .map(|i| i + 1).unwrap_or(0);
        let feedback_rounds = activity[last_blocked..].iter()
            .filter(|e| e.action == "changes_requested")
            .count();
        if feedback_rounds > 0 {
            let note_lower = note.to_lowercase();
            let rounds_addressed = (1..=feedback_rounds)
                .filter(|n| note_lower.contains(&format!("round {}:", n)))
                .count();
            if rounds_addressed < feedback_rounds {
                return Err(format!(
                    "Your note addresses {}/{} feedback rounds. \
                     Add a 'Round N: fixed <what> in <file>:<line> — test: <test_name>' \
                     line for every round before resubmitting.",
                    rounds_addressed, feedback_rounds
                ));
            }
        }
    }

    // Reject if the coder is resubmitting the exact same commit that was
    // already rejected by a reviewer. They must push new commits first.
    if let Some(new_hash) = commit_hash {
        let current = ctx.db.get_task(task_id).await.map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Task {} not found", task_id))?;
        if let Some(ref existing_hash) = current.latest_commit {
            // Check activity log — if there's a changes_requested entry after
            // the last commit was recorded, this is a stale resubmission.
            let activity = ctx.db.get_activity_for_task(task_id).await
                .unwrap_or_default();
            let last_changes_requested = activity.iter().rposition(|e| e.action == "changes_requested");
            let last_commit_recorded = activity.iter().rposition(|e| e.action == "committed");
            let is_stale_resubmission = existing_hash == new_hash
                && last_changes_requested.is_some()
                && last_changes_requested > last_commit_recorded;
            if is_stale_resubmission {
                return Err(format!(
                    "Commit {} was already reviewed and changes were requested. \
                     You must make new commits that address the reviewer feedback before calling request_review again.",
                    &new_hash[..new_hash.len().min(8)]
                ));
            }
        }
    }

    let role = agent_role(&ctx.db, agent_id).await;
    let next_status = role.as_deref()
        .and_then(|r| ctx.config.as_deref().and_then(|c| c.success_for(r)))
        .unwrap_or("reviewer");
    let task = ctx.db.update_task(
        task_id, None, None, Some(next_status), None, None, None, None, None,
        None, None, commit_hash, None, None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    ctx.db.clear_agent_current_task_if_matches(agent_id, task_id)
        .await.map_err(|e| e.to_string())?;

    info!(agent_id, task_id, commit = ?commit_hash, "review requested");
    let detail = match commit_hash {
        Some(h) => format!("{} (commit: {})", note, &h[..h.len().min(8)]),
        None => note.to_string(),
    };
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "review_requested",
        Some(&detail)).await.map_err(|e| e.to_string())?;

    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({
        "message": "Task moved to in_review",
        "task": task,
        "reviewer_instructions": {
            "branch": task.branch_name,
            "commands": task.branch_name.as_ref().map(|b| vec![
                format!("git fetch origin"),
                format!("git checkout {}", b),
                format!("git log {}..HEAD --oneline", task.base_branch),
                format!("git diff {}", task.base_branch),
            ]).unwrap_or_default()
        }
    }))
}

/// Get the branch and commit info a reviewer needs to check out.
async fn get_review_target(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let task = ctx.db.get_task_with_activity(task_id).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    let commands: Vec<String> = if let Some(ref branch) = task.task.branch_name {
        vec![
            "git fetch origin".to_string(),
            format!("git checkout {}", branch),
            format!("git log {}..HEAD --oneline", task.task.base_branch),
            format!("git diff {} --stat", task.task.base_branch),
            format!("git diff {}", task.task.base_branch),
        ]
    } else {
        vec!["# No branch recorded yet".to_string()]
    };

    // Collect all prior changes_requested entries since the last blocked event,
    // so the reviewer knows exactly what to verify was fixed.
    let last_blocked = task.activity.iter().rposition(|e| e.action == "blocked")
        .map(|i| i + 1)
        .unwrap_or(0);
    let prior_feedback: Vec<&str> = task.activity[last_blocked..].iter()
        .filter(|e| e.action == "changes_requested")
        .filter_map(|e| e.detail.as_deref())
        .collect();

    Ok(serde_json::json!({
        "task": task,
        "review_target": {
            "branch": task.task.branch_name,
            "base_branch": task.task.base_branch,
            "latest_commit": task.task.latest_commit,
            "commit_count": task.task.commit_count,
            "commits": task.commits,
            "commands": commands,
            "prior_feedback": prior_feedback
        }
    }))
}

/// Reviewer approves — moves to testing.
async fn approve_review(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let comment = args["comment"].as_str().unwrap_or("LGTM");

    let role = agent_role(&ctx.db, agent_id).await;
    let next_status = role.as_deref()
        .and_then(|r| ctx.config.as_deref().and_then(|c| c.success_for(r)))
        .unwrap_or("tester");
    let task = ctx.db.update_task(
        task_id, None, None, Some(next_status), None, None, None, None, None,
        None, None, None, None, None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    ctx.db.clear_agent_current_task_if_matches(agent_id, task_id)
        .await.map_err(|e| e.to_string())?;

    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "approved",
        Some(comment)).await.map_err(|e| e.to_string())?;

    ctx.metrics.review_approved();
    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({"message": format!("Review approved, task moved to {}", next_status), "task": task}))
}

/// Reviewer requests changes — requeues for coder, clears reviewer assignment.
const MAX_REVIEW_CYCLES: usize = 5;

async fn request_changes(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let feedback = args["feedback"].as_str().ok_or("Missing feedback")?;

    // Count changes_requested entries since the last time the task was blocked
    // (or from the beginning if it has never been blocked). This means a human
    // can intervene by dragging the task out of blocked — the counter resets.
    let prior_rejections = ctx.db.get_activity_for_task(task_id).await
        .map(|entries| {
            let since = entries.iter().rposition(|e| e.action == "blocked")
                .map(|i| i + 1)
                .unwrap_or(0);
            entries[since..].iter().filter(|e| e.action == "changes_requested").count()
        })
        .unwrap_or(0);

    let role = agent_role(&ctx.db, agent_id).await;
    let failure_role = role.as_deref()
        .and_then(|r| ctx.config.as_deref().and_then(|c| c.failure_for(r)))
        .unwrap_or("coder");
    let (new_status, new_role, message) = if prior_rejections >= MAX_REVIEW_CYCLES {
        (
            "blocked",
            None::<&str>,
            format!(
                "Task blocked after {} review cycles without resolution. Last feedback: {}",
                prior_rejections + 1,
                feedback
            ),
        )
    } else {
        (
            failure_role,
            Some(failure_role),
            format!("Changes requested, task returned to {} and reviewer unassigned", failure_role),
        )
    };

    let task = ctx.db.update_task(
        task_id, None, None, Some(new_status), None, new_role, Some(""), None, None,
        None, None, None, None, None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    ctx.db.clear_agent_current_task(agent_id)
        .await.map_err(|e| e.to_string())?;

    let action = if new_status == "blocked" { "blocked" } else { "changes_requested" };
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), action,
        Some(feedback)).await.map_err(|e| e.to_string())?;

    if new_status == "blocked" {
        ctx.metrics.task_blocked();
    } else {
        ctx.metrics.review_changes_requested();
    }
    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({
        "message": message,
        "task": task,
        "feedback": feedback
    }))
}

/// Record the worktree path an agent is using for isolated checkout.
async fn setup_worktree(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let branch_name = args["branch_name"].as_str().ok_or("Missing branch_name")?;
    let worktree_path = args["worktree_path"].as_str().ok_or("Missing worktree_path")?;
    let base_branch = args["base_branch"].as_str().unwrap_or(&ctx.base_branch);

    let task = ctx.db.update_task(
        task_id, None, None, None, None, None, None, None, None,
        Some(branch_name), Some(base_branch), None, None, Some(worktree_path), None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    let role = agent_role(&ctx.db, agent_id).await;
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "worktree_created",
        Some(&format!("Worktree: {} (branch: {})", worktree_path, branch_name)))
        .await.map_err(|e| e.to_string())?;

    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({
        "message": "Worktree recorded",
        "task": task,
        "setup_commands": [
            format!("git worktree add {} -b {}", worktree_path, branch_name),
            format!("cd {}", worktree_path),
        ]
    }))
}

/// Record a PR/MR URL for a task.
async fn set_pr_url(args: Value, ctx: &ToolContext) -> Result<Value, String> {
    let agent_id = args["agent_id"].as_str().ok_or("Missing agent_id")?;
    let task_id = args["task_id"].as_str().ok_or("Missing task_id")?;
    let pr_url = args["pr_url"].as_str().ok_or("Missing pr_url")?;

    let task = ctx.db.update_task(
        task_id, None, None, None, None, None, None, None, None,
        None, None, None, Some(pr_url), None, None,
    ).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Task {} not found", task_id))?;

    let role = agent_role(&ctx.db, agent_id).await;
    let entry = ctx.db.add_activity(task_id, Some(agent_id), role.as_deref(), "pr_opened",
        Some(pr_url)).await.map_err(|e| e.to_string())?;

    broadcast(ctx, "task_updated", &serde_json::json!(task));
    broadcast(ctx, "activity_added", &serde_json::json!(entry));
    Ok(serde_json::json!({"message": "PR URL recorded", "task": task}))
}

// ── tool definitions ──────────────────────────────────────────────────────────

pub fn tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "register_agent",
            "description": "Register an agent so it appears in the system.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "role": {"type": "string"}
                },
                "required": ["agent_id","role"]
            }
        },
        {
            "name": "get_next_task",
            "description": "Claim the next available task for your role. Returns the task plus git branch instructions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "role": {"type": "string"}
                },
                "required": ["agent_id","role"]
            }
        },
        {
            "name": "create_branch",
            "description": "Record the git branch you created locally for this task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "branch_name": {"type": "string", "description": "e.g. feature/abc12345-add-auth"},
                    "base_branch": {"type": "string", "description": "Base branch (default: main)"}
                },
                "required": ["agent_id","task_id","branch_name"]
            }
        },
        {
            "name": "record_commit",
            "description": "Record a git commit you made locally. Call this after each `git commit`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "hash": {"type": "string", "description": "Full or short commit hash from `git rev-parse HEAD`"},
                    "message": {"type": "string", "description": "Commit message"}
                },
                "required": ["agent_id","task_id","hash","message"]
            }
        },
        {
            "name": "request_review",
            "description": "Mark work complete and move the task to in_review. Returns git commands for the reviewer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "commit_hash": {"type": "string", "description": "The HEAD commit hash being submitted for review"},
                    "note": {"type": "string"}
                },
                "required": ["agent_id","task_id"]
            }
        },
        {
            "name": "get_review_target",
            "description": "Get the branch, commits, and git commands needed to review a task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"}
                },
                "required": ["task_id"]
            }
        },
        {
            "name": "approve_review",
            "description": "Approve the review — moves task to testing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "comment": {"type": "string"}
                },
                "required": ["agent_id","task_id"]
            }
        },
        {
            "name": "request_changes",
            "description": "Request changes from the coder — moves task to backlog for coder and clears the reviewer assignment.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "feedback": {"type": "string", "description": "Specific feedback on what needs to change"}
                },
                "required": ["agent_id","task_id","feedback"]
            }
        },
        {
            "name": "set_pr_url",
            "description": "Record a pull/merge request URL for the task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "pr_url": {"type": "string"}
                },
                "required": ["agent_id","task_id","pr_url"]
            }
        },
        {
            "name": "update_task_status",
            "description": "Move a task to a new status and log the transition.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "status": {"type": "string"},
                    "note": {"type": "string"}
                },
                "required": ["agent_id","task_id","status"]
            }
        },
        {
            "name": "add_task_comment",
            "description": "Add a comment to a task's activity log without changing status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "comment": {"type": "string"}
                },
                "required": ["agent_id","task_id","comment"]
            }
        },
        {
            "name": "create_task",
            "description": "Create a new task (agents can spawn subtasks). Use dependencies to list task IDs that must be Done before this task can be assigned.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "priority": {"type": "string", "enum": ["low","medium","high","critical"]},
                    "assigned_role": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "dependencies": {"type": "array", "items": {"type": "string"}, "description": "Task IDs that must be in Done status before this task can be assigned to an agent"}
                },
                "required": ["agent_id","title"]
            }
        },
        {
            "name": "list_tasks",
            "description": "Query tasks with optional filters. Returns summary fields only (no description). Call get_task with a specific task_id to retrieve the full description and activity log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {"type": "string"},
                    "assigned_role": {"type": "string"},
                    "assigned_agent_id": {"type": "string"}
                }
            }
        },
        {
            "name": "get_task",
            "description": "Get full details of a task including activity log and commits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"}
                },
                "required": ["task_id"]
            }
        },
        {
            "name": "set_output_path",
            "description": "Record the file path of an artifact produced for a task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "output_path": {"type": "string"}
                },
                "required": ["agent_id","task_id","output_path"]
            }
        },
        {
            "name": "heartbeat",
            "description": "Agent sends a heartbeat to show it's alive.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"}
                },
                "required": ["agent_id"]
            }
        },
        {
            "name": "setup_worktree",
            "description": "Record a git worktree path for this task so multiple agents can work on separate branches simultaneously. Returns the `git worktree add` commands to run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "task_id": {"type": "string"},
                    "branch_name": {"type": "string", "description": "Branch name for this worktree"},
                    "worktree_path": {"type": "string", "description": "Path for the worktree, e.g. .worktrees/feature-abc"},
                    "base_branch": {"type": "string", "description": "Base branch (default: main)"}
                },
                "required": ["agent_id","task_id","branch_name","worktree_path"]
            }
        }
    ])
}
