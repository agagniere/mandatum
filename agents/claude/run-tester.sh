#!/usr/bin/env bash
# Runs a tester agent in a continuous loop.
# Usage: ./run-tester.sh [agent-id] [project-dir]
#   or:  PROJECT_DIR=/path/to/repo ./run-tester.sh [agent-id]

set -euo pipefail

AGENT_ID="${AGENT_ID:-${1:-tester-$(hostname)-$$}}"
PROJECT_DIR="${2:-${PROJECT_DIR:-$(pwd)}}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP_CONFIG="${MCP_CONFIG:-$SCRIPT_DIR/mcp-config.json}"
PROJECT_DIR="$(cd "$PROJECT_DIR" && pwd)"
LOG_DIR="${LOG_DIR:-$SCRIPT_DIR/logs}"

# shellcheck source=agents/common-task.sh
source "$SCRIPT_DIR/../common-task.sh"

mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/tester-$AGENT_ID.log"

echo "[tester] Starting agent : $AGENT_ID"
echo "[tester] Project dir    : $PROJECT_DIR"
echo "[tester] MCP config     : $MCP_CONFIG"
echo "[tester] Log file       : $LOG_FILE"
echo "[tester] Press Ctrl-C to stop."
echo ""

register_agent_role "tester"

while true; do
  if stop_requested_rest; then
    echo "[tester/$AGENT_ID] Stop requested. Exiting."
    exit 0
  fi

  echo "[tester/$AGENT_ID] Starting test cycle at $(date '+%H:%M:%S')"

  task_json="$(claim_next_task "tester" 2>>"$LOG_FILE" || true)"
  task_id="$(jq -r '.task.id // empty' <<<"$task_json" 2>/dev/null || true)"
  if [ -z "$task_id" ]; then
    echo "[tester/$AGENT_ID] No task available." | tee -a "$LOG_FILE"
    if [ "${MANDATUM_ONCE:-0}" = "1" ]; then exit 0; fi
    heartbeat_agent
    sleep 30
    continue
  fi

  branch_name="$(jq -r '.task.branch_name // empty' <<<"$task_json")"
  if [ -z "$branch_name" ]; then
    echo "[tester/$AGENT_ID] Claimed task $task_id but no branch was recorded." | tee -a "$LOG_FILE"
    heartbeat_agent
    sleep 10
    continue
  fi

  worktree_rel=".worktrees/$(safe_worktree_name "$branch_name" "-test")"
  worktree_dir="$(ensure_worktree "$branch_name" "$worktree_rel" "branch" 2>>"$LOG_FILE" || true)"
  if [ -z "$worktree_dir" ]; then
    echo "[tester/$AGENT_ID] Failed to prepare test worktree for $branch_name." | tee -a "$LOG_FILE"
    heartbeat_agent
    sleep 10
    continue
  fi

  record_worktree_setup "$task_id" "$branch_name" "$worktree_rel"

  title="$(jq -r '.task.title // "(untitled task)"' <<<"$task_json")"
  description="$(jq -r '.task.description // ""' <<<"$task_json")"

  EXTRA_BLOCK=""
  if [ -n "${ADDITIONAL_INSTRUCTIONS:-}" ]; then
    EXTRA_BLOCK="
Additional instructions:
${ADDITIONAL_INSTRUCTIONS}
"
  fi

  PROMPT="$(cat <<EOF
You are a QA testing agent. Your agent_id is "$AGENT_ID".
The shell already registered you, claimed the task, and prepared your worktree.

Project repo root: $PROJECT_DIR
Your test worktree: $worktree_dir
Task ID: $task_id
Branch: $branch_name
Title: $title
Description:
$description
$EXTRA_BLOCK
Use the configured MCP server via the existing Claude MCP config.
Do not call register_agent, get_next_task, create_branch, or setup_worktree for this task unless you are explicitly repairing broken local state.
Run tests and make any necessary test-related commits in "$worktree_dir".
Call record_commit for any commits you make.
Call set_output_path for the main test file or files you touched.
If tests pass, call update_task_status with status "${MANDATUM_SUCCESS_STATUS:-docs_writer}" and a concise summary.
If tests fail, call update_task_status with status "${MANDATUM_FAILURE_STATUS:-coder}" and the failure details.
Call heartbeat while working.
EOF
)"

  dump_agent_diagnostics "tester/$AGENT_ID" "$worktree_dir"
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
    echo "[tester/$AGENT_ID] One-shot mode: exiting."
    exit 0
  fi
  echo "[tester/$AGENT_ID] Cycle complete. Restarting in 10s..."
  sleep 10
done
