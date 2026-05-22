#!/usr/bin/env bash
# Launch one of each agent type in parallel, all working in PROJECT_DIR.
# Usage: ./run-all.sh [project-dir]
#   or:  PROJECT_DIR=/path/to/repo ./run-all.sh
#
# project-dir defaults to cwd if not specified.

set -euo pipefail

PROJECT_DIR="${1:-${PROJECT_DIR:-$(pwd)}}"
PROJECT_DIR="$(cd "$PROJECT_DIR" && pwd)"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Mandatum agents"
echo "  Project : $PROJECT_DIR"
echo "  Scripts : $SCRIPT_DIR"
echo "  Press Ctrl-C to stop all agents."
echo ""

cleanup() {
  echo ""
  echo "Stopping all agents..."
  trap - INT TERM
  kill 0
}
trap cleanup INT TERM

"$SCRIPT_DIR/run-coder.sh"    "coder-1"    "$PROJECT_DIR" 2>&1 | sed 's/^/[coder]    /' &
"$SCRIPT_DIR/run-reviewer.sh" "reviewer-1" "$PROJECT_DIR" 2>&1 | sed 's/^/[reviewer] /' &
"$SCRIPT_DIR/run-tester.sh"   "tester-1"   "$PROJECT_DIR" 2>&1 | sed 's/^/[tester]   /' &
"$SCRIPT_DIR/run-docs_writer.sh"     "docs-1"     "$PROJECT_DIR" 2>&1 | sed 's/^/[docs]     /' &

wait
