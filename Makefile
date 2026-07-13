CARGO := cargo
CARGO_HOME ?= $(CURDIR)/.cargo-home
API_PORT ?= 8000
BIND_ADDR ?= 127.0.0.1:$(API_PORT)
KUBE_CONTEXT ?= docker-desktop
APP_NAMESPACE ?= saxo
DB_NAMESPACE ?= saxo
IMAGE ?= daytrader-api:local
GIT_SHA ?= $(shell git rev-parse HEAD)
SHARED_NGROK_GATEWAY_DIR ?= ../shared-ngrok-gateway

.PHONY: help install fmt fmt-check test check validate run api scheduler docker-build security-scan deps-dry-run k8s-deploy k8s-status k8s-db-status k8s-stop k8s-logs k8s-port-forward post-deploy-smoke post-deploy-guard diagnostics diagnostics-artifact shared-ngrok-status shared-ngrok-apply

help:
	@printf "%s\n" \
		"Rust runtime:" \
		"  make install              Fetch Rust dependencies into $(CARGO_HOME)" \
		"  make fmt                  Format Rust code" \
		"  make fmt-check            Check Rust formatting" \
		"  make test                 Run Rust unit tests" \
		"  make check                Type-check the Rust app" \
		"  make validate             Run fmt-check, test, and check" \
		"  make run                  Run Axum/Dioxus on $(BIND_ADDR)" \
		"  make scheduler            Run the Rust scheduler process" \
		"  make deps-dry-run         Show available Cargo.lock updates without changing files" \
		"  make security-scan        Run RustSec, Trivy CVE, image, and secret scans" \
		"" \
		"Docker/Kubernetes:" \
		"  make docker-build         Build $(IMAGE)" \
		"  make k8s-deploy           Deploy app to $(APP_NAMESPACE), DB remains in $(DB_NAMESPACE)" \
		"  make k8s-status           Show app pods/services/internal endpoint" \
		"  make k8s-db-status        Show CNPG database resources" \
		"  make k8s-logs             Tail API and scheduler logs" \
		"  make k8s-port-forward     Forward daytrader-frontend to localhost:$(API_PORT)" \
		"  make post-deploy-smoke    Read-only rollout and API smoke check" \
		"  make post-deploy-guard    Smoke check and verify deployed images from last deploy metadata" \
		"  make diagnostics          Collect a read-only operations and trading diagnostic bundle" \
		"  make diagnostics-artifact Collect diagnostics and save a timestamped .diagnostics artifact" \
		"  make shared-ngrok-status  Show shared public ngrok gateway status" \
		"  make shared-ngrok-apply   Apply shared public ngrok gateway from $(SHARED_NGROK_GATEWAY_DIR)" \
		"  make k8s-stop             Remove app resources from $(APP_NAMESPACE)"

install:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) fetch

fmt:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) fmt

fmt-check:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) fmt --check

test:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) test

check:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) check

validate: fmt-check test check

run:
	BIND_ADDR=$(BIND_ADDR) CARGO_HOME=$(CARGO_HOME) $(CARGO) run --bin saxo-rust

api: run

scheduler:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) run --bin saxo-rust -- --scheduler

docker-build:
	docker build --build-arg GIT_SHA=$(GIT_SHA) -f Dockerfile.api -t $(IMAGE) .

deps-dry-run:
	CARGO_HOME=$(CARGO_HOME) $(CARGO) update --dry-run

security-scan:
	CARGO_HOME=$(CARGO_HOME) bash scripts/security_scan.sh

k8s-deploy:
	KUBE_CONTEXT=$(KUBE_CONTEXT) NAMESPACE=$(APP_NAMESPACE) DB_NAMESPACE=$(DB_NAMESPACE) bash scripts/deploy_k8s_docker_desktop.sh

k8s-status:
	kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) get pods,svc,agentendpoint
	kubectl --context $(KUBE_CONTEXT) -n $(DB_NAMESPACE) get cluster,svc,pvc

k8s-db-status:
	kubectl --context $(KUBE_CONTEXT) -n $(DB_NAMESPACE) get cluster,scheduledbackup,backup,pvc
	kubectl --context $(KUBE_CONTEXT) -n $(DB_NAMESPACE) get pods -l cnpg.io/cluster=daytrader-postgres
	docker ps --filter name=daytrader_rustfs --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"

k8s-logs:
	kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) logs deployment/daytrader-api --tail=120
	kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) logs deployment/daytrader-scheduler --tail=120
	kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) logs deployment/daytrader-mcp --tail=120

k8s-port-forward:
	kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) port-forward svc/daytrader-frontend $(API_PORT):8000

post-deploy-smoke:
	KUBE_CONTEXT=$(KUBE_CONTEXT) APP_NAMESPACE=$(APP_NAMESPACE) bash scripts/post_deploy_smoke.sh

post-deploy-guard:
	KUBE_CONTEXT=$(KUBE_CONTEXT) APP_NAMESPACE=$(APP_NAMESPACE) bash scripts/post_deploy_guard.sh

diagnostics:
	KUBE_CONTEXT=$(KUBE_CONTEXT) APP_NAMESPACE=$(APP_NAMESPACE) DB_NAMESPACE=$(DB_NAMESPACE) SHARED_NGROK_GATEWAY_DIR=$(SHARED_NGROK_GATEWAY_DIR) bash scripts/diagnostics_bundle.sh

diagnostics-artifact:
	KUBE_CONTEXT=$(KUBE_CONTEXT) APP_NAMESPACE=$(APP_NAMESPACE) DB_NAMESPACE=$(DB_NAMESPACE) SHARED_NGROK_GATEWAY_DIR=$(SHARED_NGROK_GATEWAY_DIR) DIAGNOSTICS_CAPTURE=1 bash scripts/diagnostics_bundle.sh

shared-ngrok-status:
	$(MAKE) -C $(SHARED_NGROK_GATEWAY_DIR) KUBE_CONTEXT=$(KUBE_CONTEXT) status

shared-ngrok-apply:
	$(MAKE) -C $(SHARED_NGROK_GATEWAY_DIR) KUBE_CONTEXT=$(KUBE_CONTEXT) ENV_FILE=$(CURDIR)/.env apply

k8s-stop:
	-kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) delete ingress daytrader-frontend --ignore-not-found
	-kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) delete agentendpoint saxo-daytrader-internal --ignore-not-found --wait=false
	-kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) delete deployment daytrader-api daytrader-scheduler daytrader-mcp daytrader-frontend --ignore-not-found
	-kubectl --context $(KUBE_CONTEXT) -n $(APP_NAMESPACE) delete service daytrader-api daytrader-frontend daytrader-mcp --ignore-not-found
