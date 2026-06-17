# Copilot instructions for this repository

This file collects repository-specific notes to help future Copilot sessions be effective.

---

## Quick build / test / lint commands

Top-level (uses the frontend workspace):
- Dev (runs frontend dev server):
  - npm run dev
- Build frontend distribution:
  - npm run build
- Run the Tauri desktop app in development (starts Rust backend inside Tauri):
  - npm run app:dev
- Build desktop bundle (production):
  - npm run app:build

Frontend (folder: `frontend/`):
- Start dev server: cd frontend && npm run dev
- Build: cd frontend && npm run build
- Lint: cd frontend && npm run lint  (runs `eslint .`)
- Run JS tests (simple runner used in repo):
  - cd frontend && node --test src/utils/*.test.js
  - Run a single test file: cd frontend && node --test src/utils/that.test.js

Rust backend (folder: `rust-backend/`):
- Build: cd rust-backend && cargo build
- Run (standalone binary): cd rust-backend && cargo run --
- Run with special keyring commands:
  - Initialize master key: cd rust-backend && cargo run -- --init-keyring
  - Check keyring access: cd rust-backend && cargo run -- --check-keyring
- Tests: cd rust-backend && cargo test
- (Optional) Lint / static checks: cd rust-backend && cargo clippy

Notes:
- Tauri CLI is installed as a devDependency; npm scripts use `tauri` from node_modules. If you prefer, install `@tauri-apps/cli` globally.

---

## High-level architecture (big picture)

- Desktop application built with Tauri: a single desktop process embeds a React/Vite frontend and spawns the Rust backend thread.
- Frontend: `frontend/` (React + Vite). It communicates with the backend over HTTP.
- Backend: `rust-backend/` (Axum/Tokio, SQLx for SQLite). The backend is compiled into a binary and is launched inside the Tauri process lifecycle — you normally do not run it separately.
- Databases: persistent SQLite files under the `databases/` directory at repository root (or overridden by DATABASE_DIR env var). There is a `general.db` and per-account DBs named `<account_id>.db`.
- Backend networking: the Rust backend binds to an ephemeral loopback port (127.0.0.1:0) and returns the assigned port to the Tauri host. The frontend resolves the actual port via the Tauri `get_backend_port` invoke; code lives at `frontend/src/utils/api.js`.
- Encryption: SQLCipher is supported/expected by default. When encryption is enabled the app uses a master key stored in the OS keyring; the code defers keyring prompt until needed.

---

## Key conventions and repository-specific patterns

- Database layout and naming:
  - Main DB: `databases/general.db` (accounts table, metadata).
  - Per-account DBs: `databases/<account_id>.db` (emails, attachments, folders, etc.).
  - `DATABASE_DIR` environment variable overrides where the app stores database files. Default locations: `~/.guvercin/databases` (preferred) or `./databases` fallback.
- Encryption and migration:
  - Default: encryption enabled (SQLCipher). The code detects whether a DB is encrypted and migrates between plaintext and SQLCipher in some flows.
  - Keyring integration: master key is managed via OS keyring; use the rust-backend `--init-keyring` and `--check-keyring` CLI flags to create/check the key during development.
  - The Rust backend expects SQLCipher support in the bundled sqlite library (libsqlite3-sys with `bundled-sqlcipher` feature). If SQLCipher is missing, connection attempts will fail with an informative error.
- Backend startup and dev workflow:
  - Tauri launches the backend and provides the port to the frontend; look at `frontend/src-tauri/tauri.conf.json` and `frontend/src/utils/api.js` for the handshake.
  - For local development of frontend only, fallback API URL is read from Vite env var `VITE_API_URL`. The frontend default fallback is `http://127.0.0.1:5000` for compatibility with older docs.
- i18n / messages:
  - Message catalog and extraction utilities exist in the frontend (gettext extraction) and Rust (i18n module). Be mindful of the two translation pipelines when modifying strings.
- Tauri config:
  - `frontend/src-tauri/tauri.conf.json` contains Tauri build/dev hooks. The `beforeDevCommand` and `beforeBuildCommand` call the frontend npm scripts; building the Tauri bundle expects the frontend `dist` directory to exist.

---

## Files to consult first (copilot shortcuts)

- README.md — quick overview and top-level scripts
- frontend/package.json — frontend scripts, lint/test commands
- frontend/src-tauri/tauri.conf.json — Tauri build/dev integration
- frontend/src/utils/api.js — how frontend discovers backend port / fallback
- rust-backend/Cargo.toml and `rust-backend/src/` — server, routing, DB initialization, keyring flow
- rust-backend/src/db.rs — database detection, SQLCipher handling, and database directory detection
- BACKEND_SPEC.md — legacy API contract (useful for schema reference, but verify against rust-backend code)

---

If you want this file edited to add commands for additional tools (e.g., CI workflows, docker, or platform-specific build notes), say which area to expand.
