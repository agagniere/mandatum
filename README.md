# Mandatum — Multi-Agent Task Tracker

A self-contained task tracking system for coordinating AI coding agents. Exposes a full MCP server over HTTP/SSE so multiple AI agents can connect simultaneously and coordinate work via a shared kanban board. Mandatum watches the task queue and spawns agents automatically when work is available.

## Architecture

```
┌─────────────┐     REST API      ┌──────────────────────┐
│  React UI   │ ◄───────────────  │    Rust Server       │
│  (port 5173)│     SSE events    │    (port 3001)       │
└─────────────┘                   │                      │
                                  │  MCP / JSON-RPC 2.0  │
                    ┌─────────────►    (port 3002)       │
                    │             │                      │
              AI Agents           │  Agent Spawner       │
         (Claude / Codex)         │  (background task)   │
                                  └──────────┬───────────┘
                                             │
                                         SQLite DB
                                         (tasks.db)
```

The server spawns agent processes automatically when tasks become available. Each agent handles one task and exits; the spawner fires again on the next poll cycle if more work remains.

## Prerequisites

- **Rust** toolchain (stable) — [rustup.rs](https://rustup.rs)
- **A JS package manager** — npm/Node.js 18+ ([nodejs.org](https://nodejs.org)), [bun](https://bun.sh), pnpm, or yarn
- **`claude` CLI** on `PATH` for Claude agents — [Claude Code](https://claude.ai/claude-code)
- **`codex` CLI** on `PATH` for Codex agents (optional)

> **:warning: Security warning**
>
> All agent runner scripts launch their CLI with permission checks disabled:
> - Claude agents use `claude --dangerously-skip-permissions` — no per-tool-call approval, the model can read/write/execute anything the user running the script can.
> - Codex agents use `codex --dangerously-bypass-approvals-and-sandbox` — same idea plus no OS sandbox.
>
> This allows autonomous operation but means an agent can execute arbitrary commands, modify files outside the worktree, exfiltrate data, or be hijacked by a prompt-injection payload in a task description or repository file. **Only run mandatum against repositories you trust, ideally inside a VM or container.** Review the prompts in `agents/claude/run-*.sh` to see exactly what each role is told to do.

## Installation

Rust and JS dependencies are fetched automatically on first build:

```bash
export MANDATUM_TARGET_REPO=/path/to/your/project

# Optional: Default is npm.
export JAVASCRIPT_RUNTIME=bun # or pnpm or yarn

make build -j2   # build Rust and UI in parallel
```

Add `.worktrees/` to the `.gitignore` of the repo agents will work in:

```bash
echo '.worktrees/' >> $MANDATUM_TARGET_REPO/.gitignore
```

## Quick Start

### 1. Configure

Edit `mandatum.yaml` at the repo root:

```yaml
project_dir: /path/to/your/project   # git repo agents work in
agents_dir: agents                    # path to agents/ scripts directory

max_concurrent: 5   # max agents per role running simultaneously
caveman: true       # terse agent responses — reduces token usage

# agents: ordered list of pipeline roles.
# The list order defines the board column order.
# Each role's name is its queue status — tasks move between roles via transitions.
agents:
  - role: coder
    type: claude    # "claude" or "codex"
    transition:
      success: reviewer
      failure: blocked
  - role: reviewer
    type: claude
    transition:
      success: tester
      failure: coder
  - role: tester
    type: claude
    transition:
      success: docs_writer
      failure: coder
  - role: docs_writer
    type: claude
    transition:
      success: done
```

### 2. Build and run

```bash
make serve
```

Open **http://localhost:3001**.

> **First build:** `make build` compiles Rust dependencies (1–2 min). Subsequent builds are fast.

### 3. Create tasks

Use the **+ New Task** button in the UI, or `POST /api/tasks` directly. As soon as a task enters the queue, the spawner detects it and launches the appropriate agent.

For larger features, use the **planner agent** to break work down into tasks with correct roles, priorities, and dependencies:

```bash
# Interactive session — describe what you want to build
agents/claude/run-planner.sh

# Pre-load a plan document and discuss it interactively
agents/claude/run-planner.sh plan.md

# Non-interactive — read a plan file and create all tasks immediately
agents/claude/run-planner.sh --auto plan.md
```

When chatting with the planner, you can ask it to optimise the dependency graph for parallelism ("make as much run in parallel as possible") or sequence ("keep it linear, one task at a time"). Dependencies control which tasks the spawner can start — a task with unresolved dependencies won't be claimed until all its dependencies reach `done`.

### Server flags

| Flag | Default | Description |
|------|---------|-------------|
| `--ui <path>` | `ui/dist` if present | Directory of built React app to serve |
| `--db <path>` | `tasks.db` | SQLite database path |
| `--repo <path>` | — | Git repo agents work in; enables auto-merge on task completion |
| `--base-branch <name>` | `master` | Branch to merge completed tasks into |
| `--config <path>` | `mandatum.yaml` if present | YAML agent spawner config |
| `--rest-port <port>` | `3001` | REST API and UI port |
| `--mcp-port <port>` | `3002` | MCP/SSE port |

---

## How agents run

Every task is worked on through a two-layer execution model:

1. **Bash runner script** (`agents/claude/run-<role>.sh`) — claims one task from the queue, sets up a git worktree, constructs a prompt, and invokes the LLM CLI.
2. **`claude` (or `codex`) CLI** — receives the prompt and the MCP config, performs the work, and calls back to mandatum via MCP tools (`record_commit`, `request_review`, …).

The same bash script is the unit of work for both orchestration modes below — they only differ in *who* runs it and *how many times*.

| Mode | Driven by | Script behaviour | Use case |
|------|-----------|------------------|----------|
| **Spawner** | The Rust server (`server/src/spawner.rs`) polls the queue every 5 s and runs the script once per available task with `MANDATUM_ONCE=1` | One-shot: claim → run → exit | Normal operation when `mandatum.yaml` is configured. Enforces `max_concurrent`, streams logs to the UI. |
| **Manual** | You run the script yourself (`agents/claude/run-coder.sh …`) | Loops continuously: claim → run → sleep 10 s | Debugging a single role, running agents on a different machine, bypassing the spawner's role config. |

The two modes can coexist — manually-launched agents register through the same MCP calls and appear alongside spawner-driven ones in the UI. The full chain per task is:

```
Spawner (Rust)  →  run-<role>.sh  →  claude --print PROMPT  →  MCP calls back to mandatum
                       │
                       └── or you, in a terminal (manual mode)
```

---

## Agent Spawner

The server's built-in spawner polls the task queue every 5 seconds. When unclaimed tasks exist for a role, it spawns the configured agent script and streams its output.

### How it works

1. Spawner counts unclaimed tasks per role.
2. Spawns up to `max_concurrent - currently_running` new agents for that role.
3. Each spawned agent handles one task (`MANDATUM_ONCE=1`) then exits.
4. Output is streamed line-by-line via SSE (`agent_log` events) and written to `logs/agents/<agent-id>.log`.
5. When the agent exits, the counter decrements and the next poll may spawn again.

### Viewing agent logs

Click the terminal icon (⊟) on any agent card in the **Agents** panel to open a live log modal showing both historical and streaming output.

The log file is also readable directly:

```bash
cat logs/agents/coder-abc123de.log
# or via API:
curl http://localhost:3001/api/agents/coder-abc123de/log
```

### Configuration reference

All fields in `mandatum.yaml`:

```yaml
project_dir: .           # git repo agents work in; --repo CLI flag overrides
agents_dir: agents       # path to agents/ directory
max_concurrent: 5        # global max agents per role (default: 5)
caveman: true            # terse response mode — drops filler, reduces tokens (default: true)
model: sonnet            # global default claude model (optional)
effort: medium           # global default effort level (optional)

# agents: ordered list defining the pipeline and board column order.
# Terminals 'backlog', 'done', and 'blocked' are always present — don't list them.
agents:
  - role: coder
    type: claude                      # "claude" or "codex"
    transition:
      success: reviewer               # status set on success
      failure: blocked                # status set on failure (omit to stay in role)
    additional_instructions: ""       # appended to every agent prompt for this role
    max_concurrent: 3                 # override global max for this role
    caveman: false                    # override global caveman setting for this role
    model: opus                       # override global model for this role
    effort: high                      # override global effort for this role
  - role: reviewer
    type: codex
    transition:
      success: tester
      failure: coder
  - role: tester
    type: claude
    transition:
      success: done
      failure: coder
```

### Sandboxed agents (Docker)

The spawner can run each agent inside an ephemeral container instead of as a host subprocess. This isolates the dangerous `claude --dangerously-skip-permissions` (and codex equivalent) from your laptop's filesystem, processes, and credentials — the blast radius is the container.

**Setup:**

1. Build the agent image:
   ```bash
   make agent-image          # builds mandatum-agent:latest, ~500 MB
   ```
2. Flip the runtime in `mandatum.yaml`:
   ```yaml
   runtime: docker           # default: bash
   docker_image: mandatum-agent:latest   # override if you tag it differently
   ```
3. Tell the spawner how to get claude credentials. Two ways, in `mandatum.yaml`:

   ```yaml
   # Shell command run once per container spawn — stdout becomes the
   # container's ANTHROPIC_AUTH_TOKEN. Use it for short-lived bearer tokens.
   auth_token_helper: "ddtool auth token rapid-ai-platform --datacenter us1.ddbuild.io"

   # Static headers forwarded as ANTHROPIC_CUSTOM_HEADERS (multi-line OK).
   # Required by some gateways for routing/identity; missing → HTTP 400.
   anthropic_custom_headers: |
     source: claude-code
     org-id: 2
     provider: anthropic
     claude-code: true
   ```

   Alternatively, export `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, or `ANTHROPIC_CUSTOM_HEADERS` in the shell that starts `mandatum-server` — the spawner forwards those env vars when the YAML fields are absent. The spawner does **not** read `~/.claude/settings.json`.

**How it works:**
- Per spawned task the server runs `docker run --rm --name <agent-id> … mandatum-agent:latest bash /<agents-dir>/<type>/run-<role>.sh`.
- The target repo and the `agents/` directory are bind-mounted at the same absolute paths inside the container as on the host, so the worktree paths git records resolve in both contexts.
- The container reaches mandatum REST/MCP via `host.docker.internal:3001/3002`.
- Container stdout/stderr is streamed line-by-line like the bash runtime — the UI's live log view works unchanged.

### Reverse proxy for VPN-routed Anthropic gateways

Container networking can't reach a host's VPN-routed endpoints (e.g. an internal `ai-gateway.…ddbuild.io`). The spawner therefore always points containers at `http://host.docker.internal:3003`, and a small host-side reverse proxy re-issues each call over the host's network.

```bash
# Terminal 1 — proxy (requires `mitmproxy` / `mitmdump` on PATH)
make proxy                                  # listens on :3003
# customise via env vars:
PROXY_PORT=4000 PROXY_UPSTREAM=https://api.anthropic.com make proxy

# Terminal 2 — server, with credentials exported as above
make serve
```

If your upstream is the public `api.anthropic.com` (no VPN), the proxy still works fine — it just adds a local hop. To skip it entirely you'd need to make the spawner stop forcing `ANTHROPIC_BASE_URL`.

**What you still need on the host:**
- Docker (or Docker Desktop) installed and running.
- The target repo on a path that Docker can bind-mount (Docker Desktop users: ensure the path is under a shared directory).
- `mitmdump` on PATH if you're using `make proxy`.
- Nothing else — `claude`, `git`, `jq`, `uv` all live inside the image.

---

## Running Agents Manually

The spawner handles agent lifecycle automatically when `mandatum.yaml` is configured. You can still run agents manually — useful for debugging or running agents against a different server:

```bash
# All four roles at once (Claude)
agents/claude/run-all.sh /path/to/your/project

# Individual roles
agents/claude/run-coder.sh       [agent-id] [project-dir]
agents/claude/run-reviewer.sh    [agent-id] [project-dir]
agents/claude/run-tester.sh      [agent-id] [project-dir]
agents/claude/run-docs_writer.sh [agent-id] [project-dir]

# Same scripts for Codex
agents/codex/run-coder.sh     [agent-id] [project-dir]
# etc.
```

Manually-run scripts loop continuously — claim a task, complete it, sleep 10 s, repeat. Set `MANDATUM_ONCE=1` to exit after one task (what the spawner uses). Set `ADDITIONAL_INSTRUCTIONS` to inject extra text into the agent prompt.

---

## Seeding Sample Data

```bash
make seed
```

Inserts 8 tasks spread across multiple statuses and priorities, and registers 5 agents. Refresh the UI to see them.

---

## Build for Production

```bash
make build
```

Produces:
- `server/target/release/mandatum-server` — standalone binary
- `ui/dist/` — static React app (served directly by the binary)

### Single-process deployment

```bash
make serve
# or:
./server/target/release/mandatum-server --config mandatum.yaml
```

The server auto-detects `ui/dist/` and `mandatum.yaml` in the current directory, so no extra flags are needed when run from the repo root.

Open **http://localhost:3001** — both the API and the UI are served from the same port.

---

## Git Workflow

Each task maps to a git branch. The full lifecycle with the default pipeline:

```
backlog  (human assigns to coder)
  → [coder]      reviewer  (request_review)
  → [reviewer]   tester (approve_review) or coder (request_changes)
  → [tester]     docs_writer (pass) or coder (fail)
  → [docs_writer] done
  → [server]     auto-merge into base branch (if --repo configured)
```

The exact transitions are defined in `mandatum.yaml` — `request_review`, `approve_review`, and `request_changes` read the `transition.success` / `transition.failure` of the calling agent's role. The `backlog` column holds tasks that haven't entered the pipeline yet; drag a task into the first role's column (or set `assigned_role` at creation) to start it.

### Branch naming

`get_next_task` suggests a branch name automatically:

```
feature/<first-8-chars-of-id>-<slugified-title>
```

e.g. `feature/a1b2c3d4-implement-user-authentication`

### Git worktrees

Each agent gets an isolated git worktree so multiple agents can work simultaneously on the same repo without interfering:

```
.worktrees/
  feature__a1b2c3d4-add-auth/          ← coder
  feature__a1b2c3d4-add-auth-review/   ← reviewer
  feature__a1b2c3d4-add-auth-test/     ← tester
  feature__a1b2c3d4-add-auth-docs/     ← docs writer
```

All share the same `.git` object store — no duplication of history. Add `.worktrees/` to your `.gitignore`.

### Reviewer feedback loop

If a reviewer calls `request_changes`, the task returns to the coder with feedback attached. The coder must address every feedback round before `request_review` is accepted again. The server enforces this: the resubmission note must include `Round N:` lines for each round of changes requested.

### Auto-merge

When a task moves to `done` and `--repo` is configured, the server automatically merges the task branch into the base branch and cleans up all worktrees for that branch.

---

## Connecting AI Agents via MCP

Agents connect to the MCP server at port 3002. The runner scripts handle this automatically. For manual or custom agents:

```bash
# Claude Code CLI
claude mcp add --transport http task-tracker http://localhost:3002

# Codex
codex mcp add task-tracker --url http://localhost:3002
```

JSON config:

```json
{
  "mcpServers": {
    "task-tracker": {
      "url": "http://localhost:3002"
    }
  }
}
```

Legacy SSE clients: `http://localhost:3002/sse`

### Extending agents with project-scoped plugins

Mandatum's `--mcp-config` is merged with any MCP servers, plugins, skills, and slash commands enabled at project scope in the **target repo** (the one the agents work in). This applies in headless `--print` mode too, so plugins installed in the target repo are automatically available to every mandatum agent — no changes to mandatum needed.

#### Example: integrating [Speky](https://github.com/agagniere/speky) for specifications

In the target repo, install speky as a project-scoped plugin:

```bash
claude plugin marketplace add agagniere/speky --scope project
claude plugin install speky@speky --scope project
```

This registers the plugin in `.claude/settings.json`. Commit the file so every contributor — and every mandatum agent worktree — picks it up. The plugin bundles:

- Two MCP servers (`speky`, `speky-selfspec`) exposing the spec model as tools
- Workflow skills that guide Claude through onboarding a project and writing test plans

Once installed, every mandatum agent (planner, coder, reviewer, tester, docs writer) can query and update specs, and the skills will activate automatically when relevant. Projects without speky are unaffected — the plugin simply isn't enabled there.

> **Prerequisite:** speky requires `uv` ≥ 0.8.0 on `PATH`.

---

## MCP Tools Reference

### Task lifecycle

| Tool | Role | Effect |
|------|------|--------|
| `register_agent` | all | Register agent_id + role |
| `get_next_task` | all | Claim next task for role; returns task + git instructions |
| `update_task_status` | all | Manually move task to any status |
| `create_task` | all | Create a new task (useful for subtasks) |
| `list_tasks` | all | Query tasks with optional filters |
| `get_task` | all | Full task details including activity log |
| `heartbeat` | all | Update last_seen; prevents stale reaping |
| `add_task_comment` | all | Log a comment without changing status |
| `set_output_path` | all | Record artifact file path |

### Git workflow

| Tool | Role | Effect |
|------|------|--------|
| `setup_worktree` | all | Record worktree path; returns `git worktree add` commands |
| `create_branch` | coder | Record branch name |
| `record_commit` | coder, tester, docs | Record commit hash + message |
| `request_review` | coder | Move to coder's `transition.success`; validates commit exists |
| `get_review_target` | reviewer | Returns branch, commits, prior feedback |
| `approve_review` | reviewer | Move to reviewer's `transition.success` |
| `request_changes` | reviewer | Move to reviewer's `transition.failure`; attach feedback |
| `set_pr_url` | coder | Record PR/MR URL |

### Role → queue status

Each role's **name is its own queue status**. A task waiting for the `reviewer` role has `status = "reviewer"`. The `assigned_agent_id` field distinguishes a task being actively worked on from one waiting in the queue. Terminals `backlog`, `done`, and `blocked` are not roles.

---

## REST API Reference

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/info` | Server info: repo path, base branch, pipeline stage list |
| GET | `/api/tasks` | List tasks (`?status=`, `?role=`, `?agent_id=`) |
| GET | `/api/tasks/:id` | Task with full activity log and commits |
| POST | `/api/tasks` | Create task |
| PATCH | `/api/tasks/:id` | Update task fields |
| DELETE | `/api/tasks/:id` | Delete task |
| POST | `/api/tasks/:id/reset` | Reset task to its role's queue status |
| GET | `/api/tasks/:id/commits` | List commits for a task |
| POST | `/api/tasks/reap` | Manually reap all stale tasks |
| GET | `/api/activity` | Last 100 activity entries |
| GET | `/api/agents` | All registered agents |
| POST | `/api/agents/:id/stop` | Request graceful stop after current task |
| DELETE | `/api/agents/:id/stop` | Cancel stop request |
| GET | `/api/agents/:id/log` | Agent log lines as JSON array |
| GET | `/api/stats` | Task counts by status and role |
| GET | `/events` | SSE stream for real-time UI updates |

### SSE event types

| Event | Trigger |
|-------|---------|
| `task_created` | New task added |
| `task_updated` | Task fields changed |
| `task_reaped` | Stale task reset by watchdog |
| `activity_added` | New activity log entry |
| `agent_registered` | Agent registered |
| `agent_updated` | Agent stop flag changed |
| `agent_log` | Line of output from a spawned agent |

---

## Stale Agent Detection

The server resets tasks whose assigned agent hasn't sent a heartbeat in **10 minutes**, moving them back to their role's queue status (the role name itself). The background watchdog runs every 60 seconds.

Trigger manually:

```bash
curl -X POST http://localhost:3001/api/tasks/reap
curl -X POST http://localhost:3001/api/tasks/<id>/reset
```

The UI shows a stale warning on agent cards that haven't been seen in 5 minutes, and a **Reset** button inside the task modal.
