# Repository Guidelines

## Project Structure & Module Organization
This repository is split by runtime boundary.

- `src/agent/`: Rust agent runtimes, ACP adapters, and normalized event streaming.
- `src/core/`: Rust core logic for sessions, turns, persistence, prompt assembly, and outbound event emission.
- `src/api/`: Rust HTTP ingress for `gateway -> core` traffic.
- `gateway/`: Node.js gateway service for Feishu auth, pairing, session routing, rendering, and delivery.
- `deploy/systemd/`: Linux service unit templates and production env example.
- `deploy/launchd/`: macOS `launchd` agent templates.
- `src/bin/acp_smoke.rs`: manual ACP runtime smoke binary.
- `scripts/`: local start/stop/smoke helpers.
- `docs/`: architecture, design, interfaces, config, data, operations, and API examples.

## Build, Test, and Development Commands
- `cargo check`: validate the Rust core compiles.
- `cargo test`: run Rust unit tests.
- `cargo run --bin otterlink`: start the Rust core on `CORE_BIND`.
- `cd gateway && npm test`: run Node gateway tests.
- `cd gateway && npm start`: start the JS gateway on `BIND`.
- `./scripts/start-longconn.sh`: start core + gateway together.
- `./scripts/stop-longconn.sh`: stop local services.
- `./scripts/test-local-acp.sh`: run Rust tests, gateway tests, and a local health smoke.

## Long-Running Process Rule
Commands that must keep running, such as `./scripts/start-longconn.sh`, `cargo run --bin otterlink`, `cd gateway && npm start`, `launchctl`, or `systemctl` operations, must be executed manually by the user in a real terminal. The assistant execution environment is not persistent: background jobs may be reaped after the command finishes, so assistant-started services are not considered reliable or durable.

## Coding Style & Naming Conventions
Rust uses 4-space indentation, `snake_case` for functions/modules, and `PascalCase` for types. JavaScript in `gateway/` uses CommonJS, 2-space indentation is acceptable, and `camelCase` for functions. Keep platform concepts in `gateway/`; keep agent/session logic in Rust.

## Testing Guidelines
Add Rust tests near the affected module with `#[cfg(test)]`. Add Node tests under `gateway/test/*.test.js` using `node:test`. Cover session routing, protocol conversion, auth/pairing, rendering, and persistence. Keep simulated payloads inside tests; do not add fake-path code to production modules.

## Commit & Pull Request Guidelines
Use short imperative commits such as `refactor core turn protocol` or `move feishu auth into gateway`. PRs should describe boundary changes, config changes, test evidence, and any user-visible Feishu rendering changes.

## Security & Configuration Tips
Do not commit `.run/`, bot credentials, or real `open_id` values. Rust trusts the gateway, so `CORE_BIND` should stay private. Protect `gateway -> core` with `CORE_INGEST_TOKEN` and `core -> gateway` with `GATEWAY_EVENT_TOKEN`.

## Documentation Maintenance
`docs/README.md`, `docs/architecture.md`, `docs/design.md`, `docs/interfaces.md`, `docs/configuration.md`, `docs/data-model.md`, `docs/operations.md`, `docs/api-examples.md`, `docs/acp.md`, `docs/installation.md`, and `docs/macos-installation.md` must be updated in the same change whenever behavior, boundaries, schema, commands, or deployment flow change. Update `README.md` as well when setup or operator workflow changes.
