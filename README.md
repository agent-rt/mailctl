# mailctl

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org/)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)](#requirements)

**An agent-first command-line email client for Gmail and Outlook/Hotmail.**

mailctl is built to be driven by AI agents and scripts, not humans clicking around. Every command speaks structured JSON, authentication is non-interactive after a one-time setup, and every destructive action is gated, reversible, and audited. It lets an agent read, search, and *safely* manage a real mailbox over IMAP/SMTP.

```console
$ mailctl search "is:unread from:boss" --limit 5
{
  "folder": "INBOX",
  "uidvalidity": 14,
  "messages": [
    { "uid": 4821, "from": "Boss <boss@example.com>", "subject": "Q3 review",
      "date": "Wed, 17 Jun 2026 08:14:11 +0000", "unread": true, "size": 20848, "is_bulk": false }
  ]
}
```

---

## Why mailctl?

Most mail tools are designed for humans. Agents need different things:

- **Token-lean by default.** `search` returns metadata only — no bodies. Fetch a full message with `read` only when needed. Agents don't pay for tens of thousands of tokens of email they won't use.
- **Structured everything.** Stable JSON schemas, stable exit codes, machine-readable errors on `stderr`. Easy to compose across multiple steps.
- **Non-interactive.** No browser pop-ups or keychain prompts mid-run. Authenticate once; the agent runs unattended afterwards.
- **Safe by construction.** Deletion is never permanent, batch operations preview before they act, every change is reversible and logged, and stale message IDs are rejected. See [Safety model](#safety-model).

## Features

- 📥 **Gmail & Outlook/Hotmail** over IMAP (read) + SMTP (send).
- 🔑 **Modern auth**: Gmail via App Password *or* OAuth2, Outlook/Hotmail via OAuth2 — OAuth uses Authorization Code + PKCE with `XOAUTH2`. Short-lived access tokens are cached locally so commands rarely hit the network just to authenticate.
- 🔎 **Search** with a small query DSL (`is:unread`, `from:`, `to:`, `subject:`) translated to IMAP `SEARCH`.
- 🌏 **Correct internationalization**: RFC 2047 encoded-word subjects and modified-UTF-7 folder names are decoded properly (CJK, emoji, etc.).
- 🏷️ **Organize**: move between folders, add/remove Gmail labels, list folders/labels.
- 🗑️ **Manage safely**: preview → confirm trash, restore from trash, audit log, UIDVALIDITY consistency checks.
- 📨 **Bulk detection**: every message is flagged `is_bulk` based on the `List-Unsubscribe` header — a reliable signal for marketing/newsletters.
- 💾 **Local cache (SQLite)**: re-reading a message skips re-downloading it (bodies are immutable, so hits are always correct). `sync` pulls folder metadata into a local store for `search --cached` — fast, zero-network queries. Searches are **real-time by default**; the cache is explicit (`--cached`).
- 🔐 **Secrets stay out of files**: credentials live in the macOS Keychain, or are injected at runtime via a secret manager (see [Secret storage](#secret-storage)).

## Requirements

- **macOS** (uses the system Keychain, the `open` command for OAuth, and `/dev/urandom`). Porting to Linux/Windows is tracked in the roadmap.
- **Rust 2024** (Rust 1.85+). Build with a recent stable toolchain.

## Install

```console
$ git clone https://github.com/agent-rt/mailctl.git
$ cd mailctl
$ cargo build --release
# binary at ./target/release/mailctl — copy it onto your PATH
$ cp target/release/mailctl ~/.local/bin/   # or wherever you keep binaries
```

## Quick start

### Gmail — App Password

Gmail requires 2-Step Verification, then an [App Password](https://myaccount.google.com/apppasswords). IMAP must be enabled in Gmail settings.

```console
$ mailctl auth login --provider gmail --email you@gmail.com
App Password: ****************
$ mailctl search "is:unread" --limit 10
```

### Gmail — OAuth2 (for Workspace)

Many Google Workspace organizations disable App Passwords. In that case, use OAuth with a [Google Cloud Desktop client](#google-cloud-oauth-client-for-gmail) (`client_id` + `client_secret`):

```console
$ mailctl auth login --provider gmail --email you@workspace.com \
    --client-id <your-client-id> --client-secret <your-client-secret>
# A browser opens; sign in and consent. The refresh token is stored in the Keychain.
```

> mailctl picks the auth method by whether `--client-id` is given: with it → OAuth, without it → App Password.

### Outlook / Hotmail (OAuth2)

Personal Microsoft accounts require OAuth2. You register a free [Azure app](#azure-app-registration-for-outlookhotmail) once to get a public client id, then:

```console
$ mailctl auth login --provider hotmail --email you@outlook.com --client-id <your-client-id>
# A browser opens; sign in and consent. The refresh token is stored in the Keychain.
$ mailctl --account you@outlook.com search --limit 10
```

## Commands

All commands accept `--account <email>` (defaults to the default account) and `--folder <name>` (defaults to `INBOX`).

| Command | Description |
| --- | --- |
| `auth login --provider <gmail\|hotmail> --email <e> [--password \| --client-id \| --secret-ref]` | Register an account and store its credential. |
| `auth list` | List configured accounts. |
| `auth logout <email>` | Remove an account and wipe its stored credentials. |
| `folders` | List folders/labels (`{name, selectable}`). |
| `search [query] [--limit N] [--expect-uidvalidity N] [--cached]` | Search; metadata only (token-lean). Real-time by default; `--cached` reads the local store (needs `sync`). |
| `sync` | Incrementally pull a folder's metadata into the local cache (for `search --cached`). |
| `read <uid>` | Read one message's body (`BODY.PEEK` — does **not** mark as read; cached locally). |
| `cache info` / `cache clear` | Inspect or clear the local body cache. |
| `flag <uid> [--read] [--star]` | Set message flags. |
| `move <uids...> --to <folder> [--create] [--expect-uidvalidity N]` | Move messages (reversible). |
| `label <uids...> [--add L] [--remove L] [--expect-uidvalidity N]` | Add/remove Gmail labels (Gmail only). |
| `trash <uids...> [--confirm] [--expect-uidvalidity N]` | Move to trash. Without `--confirm`, previews only. |
| `restore <uids...> [--to <folder>] [--expect-uidvalidity N]` | Restore from trash. |
| `send --to <addr> --subject <s> [--body \| --body-file] [--confirm]` | Send mail. Without `--confirm`, saved as a draft. |

### Query DSL

`search` accepts a small syntax, translated to IMAP `SEARCH`:

| Token | Meaning |
| --- | --- |
| `is:unread` / `is:read` | unseen / seen |
| `is:starred` | flagged |
| `from:x` `to:x` `subject:x` | header substring match |
| any other word | full-text match |

Omit the query to list everything.

## Safety model

An agent managing a real mailbox is high-stakes. mailctl is designed so that no single mistake is catastrophic:

1. **Never permanently deletes.** `trash` moves messages to the Trash/Deleted folder (30-day recovery window on Gmail/Outlook). `EXPUNGE` is never issued.
2. **Preview, then confirm.** `trash` without `--confirm` returns the exact list it *would* delete (with real subjects) and changes nothing. Deletion happens only with `--confirm`.
3. **Reversible.** `restore` moves messages back out of Trash. `move` is undone by moving back.
4. **Audited.** Every mutation is appended to a JSONL audit log *before* it runs, so there is always a record of intent — even if the process dies mid-operation.
5. **Stale-ID protection.** IMAP UIDs are only stable within a `(folder, UIDVALIDITY)` pair. `search` returns the folder's `uidvalidity`; pass it back with `--expect-uidvalidity` and mailctl aborts *before* touching anything if the mailbox has been recreated.

A typical safe agent workflow:

```console
# 1. discover + classify
$ mailctl search "is:unread" --limit 50          # note the uidvalidity, use is_bulk to find newsletters

# 2. preview the destructive action
$ mailctl trash 101 102 103                       # executed:false, shows would_trash

# 3. confirm, pinned to the uidvalidity from step 1
$ mailctl trash 101 102 103 --confirm --expect-uidvalidity 14

# 4. changed your mind? (uids in Trash differ — search the Trash folder first)
$ mailctl --folder "[Gmail]/Trash" search
$ mailctl restore 55
```

## Secret storage

mailctl never writes long-lived credentials to plaintext files.

- **Keychain (default):** App Passwords and OAuth refresh tokens are stored in the macOS Keychain.
- **External secret manager (`--secret-ref`):** set a `secret_ref` per account so the credential is read from an environment variable at runtime instead of the Keychain. Run mailctl wrapped in your secret manager so the value is injected only for that process:

  ```console
  $ mailctl auth login --provider gmail --email you@gmail.com --secret-ref my_mail_secret
  $ secretctl exec --only my_mail_secret -- mailctl search --limit 10
  ```

  This keeps mailctl entirely off the Keychain for that account.

Short-lived OAuth access tokens are cached in a `0600` file under the OS cache directory and reused until shortly before expiry — so most commands authenticate with zero network round-trips and zero Keychain access.

## Azure app registration (for Outlook/Hotmail)

Registering an app is free and needs no Azure subscription.

1. Sign in to the [Microsoft Entra admin center](https://entra.microsoft.com) → **App registrations** → **New registration**.
2. **Supported account types:** *Accounts in any organizational directory and personal Microsoft accounts*.
3. **Redirect URI:** platform **Mobile and desktop applications**, value `http://localhost` (loopback; any port is accepted).
4. Copy the **Application (client) ID** — that's your `--client-id`.

API permissions usually need no pre-configuration for personal accounts: the IMAP/SMTP scopes are requested dynamically and consented at sign-in.

## Google Cloud OAuth client (for Gmail)

For Gmail OAuth (e.g. Workspace accounts without App Passwords):

1. In the [Google Cloud Console](https://console.cloud.google.com/), create (or pick) a project.
2. Enable the **Gmail API** for the project.
3. Configure the **OAuth consent screen** (External or Internal). Add the scope `https://mail.google.com/`. While in *Testing*, add your address as a test user.
4. **Credentials → Create credentials → OAuth client ID → Application type: Desktop app.** This yields a **client id** and **client secret** (Google treats installed-app secrets as non-confidential, but they are required in the token exchange).
5. Use them as `--client-id` / `--client-secret`. The client secret is stored in the Keychain, never in config.

> Workspace admins can also restrict which OAuth apps may access mail; you may need an admin to approve the app for your domain.

## Architecture

```
src/
├── main.rs         command dispatch; uniform JSON output + exit codes
├── cli.rs          clap command tree
├── provider.rs     Gmail / Hotmail endpoints (enum — illegal states unrepresentable)
├── config.rs       per-account metadata (~/.config); no secrets on disk
├── auth.rs         credential backends: Keychain or env (secret manager)
├── oauth.rs        OAuth2 for Gmail & Outlook (Authorization Code + PKCE, loopback), token cache
├── imap_client.rs  IMAP: search/read/flag/move/label/trash/folders + XOAUTH2
├── smtp_client.rs  SMTP send/draft (App Password or XOAUTH2)
├── mime.rs         MIME parsing (mail-parser)
├── cache.rs        SQLite cache: bodies + folder metadata (rusqlite, bundled)
├── audit.rs        JSONL audit log for mutations
└── model.rs        JSON output contracts
```

Design choices: synchronous I/O (a short-lived CLI gains nothing from an async runtime), strongly-typed errors with no `unwrap`/`panic` in normal paths, and provider differences modeled as an `enum` so the protocol layers stay provider-agnostic.

## Roadmap

- [ ] Attachment download
- [ ] HTML and attachment composition for `send`
- [ ] Transient-error retries
- [ ] Linux / Windows support (secret backend + browser launch)

## Contributing

Issues and pull requests are welcome. Please run `cargo fmt`, `cargo clippy`, and `cargo test` before submitting.

## License

Licensed under the [MIT License](LICENSE).
