# JS package manager — npm, bun, pnpm, and yarn all work
JAVASCRIPT_RUNTIME    ?= npm
MANDATUM_TARGET_REPO  ?= $(shell pwd)

CARGO      := cargo
JS_INSTALL := $(JAVASCRIPT_RUNTIME) install
# Deno users would override this one with deno task build
JS_BUILD   := $(JAVASCRIPT_RUNTIME) run build

export PROJECT_DIR := $(MANDATUM_TARGET_REPO)

DOCKER_IMAGE := mandatum-agent:latest

# Reverse-proxy port for agent containers to reach VPN-restricted upstreams
# (e.g. https://ai-gateway.us1.ddbuild.io) via the host network.
PROXY_PORT     ?= 3003
PROXY_UPSTREAM ?= https://ai-gateway.us1.ddbuild.io

.PHONY: build build-server build-ui serve seed clean agents agent-image reset-db proxy

build: build-server build-ui
	@echo "Build complete ✅"

build-server: | server/Cargo.toml
	$(CARGO) build --release --manifest-path $|

build-ui:
	cd ui && $(JS_INSTALL) && $(JS_BUILD)

# Build everything and run as a single process (server serves the UI)
serve: build
	./server/target/release/mandatum-server \
		--ui ui/dist \
		--repo $(MANDATUM_TARGET_REPO) \
		--config $(MANDATUM_TARGET_REPO)/mandatum.yaml

# Insert sample tasks and agents into the DB
seed:
	@echo "Seeding database..."
	cd server && bash seed.sh

# Run all four agent types in parallel (requires `claude` CLI on PATH)
# MANDATUM_TARGET_REPO defaults to cwd — set it to the repo the agents should work in
agents:
	bash agents/claude/run-all.sh

# Build the container image used when mandatum.yaml has `runtime: docker`.
agent-image:
	docker build -t $(DOCKER_IMAGE) agents/

# Drop the task database. Stop the server first.
reset-db:
	$(RM) tasks.db tasks.db-wal tasks.db-shm

# Run a reverse proxy so containerised agents can reach VPN-only upstreams via
# the host's network. Required when `runtime: docker` + a VPN-routed gateway.
# Requires `mitmdump` (mitmproxy) on PATH.
proxy:
	mitmdump --mode reverse:$(PROXY_UPSTREAM) --listen-port $(PROXY_PORT)

# Clean build artifacts
clean:
	cd server && $(CARGO) clean
	cd ui && $(RM) -r dist node_modules
