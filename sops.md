# Operating Procedures

## Tech Stack
- Language: Rust (edition 2021)
- Workspace: cargo, two binary members (`herald-server`, `herald-cli`) plus a test-only member (`tests` → `herald-smoke-tests`)
- Server runtime: tokio + axum + Redis (single authoritative backend per deployment)
- CLI runtime: tokio + reqwest + tokio-tungstenite
- Testing: `cargo test --workspace` (inline `#[cfg(test)] mod tests` per source file plus per-file API-drift smoke tests under `tests/smoke/`)
- Hosting: Fly.io (`fly.toml`, `Dockerfile`); rolling deploys, two machines, /health every 30s

## Standards
- All public symbols documented; private items doc'd only when their `why` is non-obvious
- Errors flow through `HeraldError` (server) / `CliError` (cli) — never `panic!` in request paths
- Wire IDs are prefixed (`msg_<hex>`, `fp_<hex>`, `hrl_sk_<hex>`); raw hex is internal-only
- All JSON responses carry an `object` discriminator; list responses use the `{object,data,has_more,queue_depth}` envelope
- Errors share a structured shape: `{error:{type,code,message}}` with `Retry-After` on 429s
- snake_case for all wire fields; Unix integer seconds for timestamps
- No mocking of Redis in tests that exercise queue semantics — use a real Redis (per the pact-style "tests must be runnable" rule, swap for a containerized Redis in CI)

## Verification
- Inline tests cover behaviour; smoke tests under `tests/smoke/` cover API drift (file existence + symbol-presence assertions)
- Every endpoint has at least one inline test in its handler module
- A breaking change to a route shape MUST update SPEC.md and the `/docs` page in the same commit
- Live verification: smoke-curl `proxy.herald.tools` after every fly deploy; fail-and-rollback on a non-2xx from `/health`

## Preferences
- Prefer the standard library and the chosen async stack (tokio, axum, reqwest); avoid pulling new crates for one-off needs
- Keep modules under ~600 lines; if a route module grows past that, split by HTTP method or by resource
- Comments explain *why*, not *what* — well-named identifiers carry the *what*
- Don't add backwards-compatibility shims pre-GA; break cleanly and bump the tag

## Release
- Manual: `cargo test --workspace` → commit → `git push` → `git tag -a vX.Y.Z` → `git push --tags` → `fly deploy --remote-only` → live `/health` and round-trip smoke curl
- No CI/CD on the herald repo today; managed local gate via `cargo test --workspace`
