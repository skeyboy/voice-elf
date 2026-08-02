.PHONY: dev server web web-deploy-watch web-public web-dev-public web-public-status web-public-stop setup-models build test

dev:
	@echo "Run 'make server' and 'make web' in separate terminals"

server:
	cargo run --bin voice-elf-server

web:
	cd web && npm run dev

web-deploy-watch:
	cd web && npm run deploy:watch

web-public:
	./scripts/public-tunnel.sh start production

web-dev-public:
	./scripts/public-tunnel.sh start dev

web-public-status:
	./scripts/public-tunnel.sh status all

web-public-stop:
	./scripts/public-tunnel.sh stop all

setup-models:
	./scripts/setup-local-models.sh

build:
	cd web && npm run build
	cargo build --release --bin voice-elf-server

test:
	cargo test --workspace
	cd web && npm run build
