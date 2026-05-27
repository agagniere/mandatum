#!/usr/bin/env bash
# Generic agent runner — driven entirely by env vars set by the spawner.
#
# Required:
#   MANDATUM_ROLE        — role name (e.g. "coder", "reviewer")
#   MANDATUM_ROLE_PROMPT — role instructions; $MANDATUM_SUCCESS_STATUS and
#                          $MANDATUM_FAILURE_STATUS are substituted at runtime
#
# Optional:
#   MANDATUM_FETCH_REVIEW_CONTEXT=1  — pre-fetch changes_requested activity
#                                      and inject it before calling Claude
#   MANDATUM_SUCCESS_STATUS / MANDATUM_FAILURE_STATUS — transition targets
#   MANDATUM_MODEL / MANDATUM_EFFORT — forwarded to the claude CLI
#   ADDITIONAL_INSTRUCTIONS          — appended to every prompt
#   MANDATUM_ONCE=1                  — exit after one task (spawner mode)

set -euo pipefail

AGENT_ID="${AGENT_ID:-${1:-agent-$(hostname)-$$}}"
PROJECT_DIR="${2:-${PROJECT_DIR:-$(pwd)}}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP_CONFIG="${MCP_CONFIG:-$SCRIPT_DIR/mcp-config.json}"
PROJECT_DIR="$(cd "$PROJECT_DIR" && pwd)"
LOG_DIR="${LOG_DIR:-$SCRIPT_DIR/logs}"
ROLE="${MANDATUM_ROLE:-${AGENT_ID%%-*}}"

# shellcheck source=agents/common-task.sh
source "$SCRIPT_DIR/../common-task.sh"

mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/$ROLE-$AGENT_ID.log"

echo "[$ROLE] Starting agent : $AGENT_ID"
echo "[$ROLE] Project dir    : $PROJECT_DIR"
echo "[$ROLE] MCP config     : $MCP_CONFIG"
echo "[$ROLE] Log file       : $LOG_FILE"
echo "[$ROLE] Press Ctrl-C to stop."
echo ""

register_agent_role "$ROLE"

while true; do
  if stop_requested_rest; then
    echo "[$ROLE/$AGENT_ID] Stop requested. Exiting."
    exit 0
  fi

  echo "[$ROLE/$AGENT_ID] Starting cycle at $(date '+%H:%M:%S')"

  # ── Claim task ──────────────────────────────────────────────────────────────
  task_json="$(claim_next_task "$ROLE" 2>>"$LOG_FILE" || true)"
  task_id="$(jq -r '.task.id // empty' <<<"$task_json" 2>/dev/null || true)"
  if [ -z "$task_id" ]; then
    echo "[$ROLE/$AGENT_ID] No task available." | tee -a "$LOG_FILE"
    if [ "${MANDATUM_ONCE:-0}" = "1" ]; then exit 0; fi
    heartbeat_agent
    sleep 30
    continue
  fi

  branch_name="$(jq -r '.task.branch_name // .git_instructions.suggested_branch // empty' <<<"$task_json")"
  title="$(jq -r '.task.title // "(untitled task)"' <<<"$task_json")"
  description="$(jq -r '.task.description // ""' <<<"$task_json")"

  if [ -z "$branch_name" ]; then
    echo "[$ROLE/$AGENT_ID] Claimed task $task_id but no branch was available." | tee -a "$LOG_FILE"
    heartbeat_agent
    sleep 10
    continue
  fi

  # ── Worktree ────────────────────────────────────────────────────────────────
  worktree_rel=".worktrees/$(safe_worktree_name "$branch_name" "-$ROLE")"
  worktree_dir="$(ensure_worktree "$branch_name" "$worktree_rel" "branch" 2>>"$LOG_FILE" || true)"
  if [ -z "$worktree_dir" ]; then
    echo "[$ROLE/$AGENT_ID] Failed to prepare worktree for $branch_name." | tee -a "$LOG_FILE"
    heartbeat_agent
    sleep 10
    continue
  fi
  record_worktree_setup "$task_id" "$branch_name" "$worktree_rel"

  # ── Review context (changes_requested history) ──────────────────────────────
  REVIEW_SECTION=""
  if [ "${MANDATUM_FETCH_REVIEW_CONTEXT:-0}" = "1" ]; then
    task_detail="$(curl -sf "${MANDATUM_REST_URL:-http://localhost:3001}/api/tasks/$task_id" || true)"
    if [ -n "$task_detail" ]; then
      review_count="$(jq -r '.activity | map(select(.action == "changes_requested")) | length' \
        <<<"$task_detail" 2>/dev/null || echo 0)"
      if [ "$review_count" -gt 0 ]; then
        review_history="$(jq -r '
          .activity
          | map(select(.action == "changes_requested"))
          | to_entries
          | map("Round \(.key + 1) [\(.value.timestamp[:19])]:\n\(.value.detail // "(no detail)")")
          | join("\n\n---\n\n")
        ' <<<"$task_detail" 2>/dev/null || true)"
        REVIEW_SECTION="$(cat <<REVIEW

Review history ($review_count round(s)):

$review_history

REVIEW
)"
      fi
    fi
  fi

  # ── Additional instructions ─────────────────────────────────────────────────
  EXTRA_BLOCK=""
  if [ -n "${ADDITIONAL_INSTRUCTIONS:-}" ]; then
    EXTRA_BLOCK="
Additional instructions:
${ADDITIONAL_INSTRUCTIONS}
"
  fi

  # ── Build prompt ─────────────────────────────────────────────────────────────
  # Substitute transition targets into the role prompt template
  ROLE_INSTRUCTIONS="$(printf '%s' "${MANDATUM_ROLE_PROMPT:-}" | \
    sed "s|\\\$MANDATUM_SUCCESS_STATUS|${MANDATUM_SUCCESS_STATUS:-}|g; \
         s|\\\$MANDATUM_FAILURE_STATUS|${MANDATUM_FAILURE_STATUS:-}|g")"

  PROMPT="$(cat <<EOF
$ROLE_INSTRUCTIONS

Your agent_id is "$AGENT_ID".
The shell already registered you, claimed the task, and prepared your worktree.

Project repo root: $PROJECT_DIR
Your worktree: $worktree_dir
Task ID: $task_id
Branch: $branch_name
Title: $title
Description:
$description
$REVIEW_SECTION$EXTRA_BLOCK
Use the configured MCP server via the existing Claude MCP config.
Do not call register_agent, get_next_task, create_branch, or setup_worktree for this task unless you are explicitly repairing broken local state.
Call heartbeat while working.
EOF
)"

  dump_agent_diagnostics "$ROLE/$AGENT_ID" "$worktree_dir"
  STREAM_TMP="$(mktemp)"
  (
    cd "$worktree_dir"
    claude --dangerously-skip-permissions \
      --mcp-config "$MCP_CONFIG" \
      --output-format stream-json --verbose \
      "${CLAUDE_EXTRA_ARGS[@]}" \
      --print "$PROMPT" 2>&1
  ) | tee "$STREAM_TMP" | claude_stream_filter | tee -a "$LOG_FILE" || true
  agent_run_summary "$STREAM_TMP" | tee -a "$LOG_FILE"
  rm -f "$STREAM_TMP"
  echo ""
  if [ "${MANDATUM_ONCE:-0}" = "1" ]; then
    echo "[$ROLE/$AGENT_ID] One-shot mode: exiting."
    exit 0
  fi
  echo "[$ROLE/$AGENT_ID] Cycle complete. Restarting in 10s..."
  sleep 10
done
