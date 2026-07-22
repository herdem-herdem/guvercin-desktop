## guvercin

Important: This software is licensed under the Apache License 2.0 with a Commons Clause condition. Commercial use, selling, or sub-licensing the software is strictly prohibited.

### Backend (Rust / Axum)

- The backend is now completely written in **Rust** and is located under the `rust-backend` folder.
- Key technologies used:
  - Axum (HTTP server)
  - Tokio (async runtime)
  - SQLx (SQLite access)
  - IMAP (to connect to IMAP servers)

### Desktop App (Single Process UX)

The app is configured as a **single Tauri desktop application**:
- Frontend starts inside Tauri.
- Rust backend starts automatically inside the same desktop app process lifecycle.
- You do **not** need to run backend separately.

#### Run (development)

```bash
npm run app:dev
```

#### Build desktop bundle

```bash
npm run app:build
```

Notes:
- Backend HTTP API is still served internally on `127.0.0.1:5000` by the Tauri-embedded backend thread.
- No separate `cargo run` for backend is required during normal desktop app usage.

#### Database

- Database files are kept under the `databases/` folder at the project root:
  - `general.db`: account configuration tables.
  - `<account_id>.db`: separate email/attachment/folder/reference tables for each user.
- When the Rust backend first runs, it automatically creates the necessary tables (to be compatible with the schema on the Python side).

#### IMAP Authorization

- The `imap_client` module within `rust-backend` tests authorization by connecting to the IMAP server and logging in with the username/password.
- This behavior re-implements the functionality of the previous Python `imap_client.py` using Rust.

### Frontend

- The frontend code is in the `frontend` folder (React/Vite).
- It communicates with the backend over HTTP; having the Rust backend running is sufficient.

### Google / Gmail sign-in (OAuth2)

Gmail accounts are supported through the standard IMAP/SMTP pipeline using OAuth2
(`XOAUTH2`). "Continue with Google" runs a PKCE loopback flow: the system browser
opens Google's consent screen and the redirect is caught on a local
`http://127.0.0.1:<random-port>` address — no tokens ever pass through the UI.

The app **ships with a default OAuth client**, so Google sign-in works out of
the box — end-users don't need to configure anything. For an installed / desktop
app this is expected: Google treats such clients as non-confidential, since the
client secret necessarily ships inside the distributed binary.

**Using your own client (optional, for forks).** The default is defined in
`rust-backend/src/oauth.rs`; override it without editing that file by setting
environment variables (loaded from `.env` in development, or baked in at build
time via `option_env!`):

1. In the [Google Cloud Console](https://console.cloud.google.com/) create a
   project, enable the **Gmail API**, and create an OAuth client of type
   **Desktop app**.
2. Copy `.env.example` to `.env` (git-ignored) and set:

   ```bash
   GOOGLE_CLIENT_ID=your_client_id
   GOOGLE_CLIENT_SECRET=your_client_secret
   ```

Resolution order per value: environment variable → `option_env!` build-time
value → shipped default.

No redirect URI needs to be registered — the loopback address is dynamic, which
the "Desktop app" client type allows.
