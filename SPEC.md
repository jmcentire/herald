# Herald Specification

**Version**: 0.1.0-draft
**License**: MIT
**Status**: Draft

Herald is an asynchronous webhook relay and message queue for AI agents and local services that cannot expose public endpoints. It receives HTTP POST requests at stable URLs, generates content-addressable message IDs, encrypts payloads on receipt, and holds them in a FIFO queue. Agents drain the queue via HTTP polling or WebSocket when ready.

Herald is self-hostable. The hosted instance runs at `proxy.herald.tools`.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Technology Stack](#technology-stack)
3. [Repositories](#repositories)
4. [URL Structure](#url-structure)
5. [Message Lifecycle](#message-lifecycle)
6. [Redis Data Model](#redis-data-model)
7. [Storage Backends](#storage-backends)
8. [Encryption Model](#encryption-model)
9. [Delivery Semantics](#delivery-semantics)
10. [Dead Letter Queue](#dead-letter-queue)
11. [Replay Protection](#replay-protection)
12. [Headers and Body Separation](#headers-and-body-separation)
13. [Rate Limiting](#rate-limiting)
14. [Tier Separation and Infrastructure](#tier-separation-and-infrastructure)
15. [Account Tiers](#account-tiers)
16. [Herald CLI (Local Daemon)](#herald-cli-local-daemon)
17. [Kindex Integration](#kindex-integration)
18. [Security Model](#security-model)
19. [API Surface](#api-surface)
20. [Competitive Positioning](#competitive-positioning)
21. [Resolved Decisions](#resolved-decisions)

---

## Architecture Overview

```
External Service                        Herald                           Local Agent
(GitHub, Stripe, etc.)                  (relay)                          (CLI, LLM, script)

  POST /customer/endpoint ──────▶  [ Receive ]
                                   [ Hash body → message ID ]
                                   [ Store headers separately ]
                                   [ Encrypt body ]
                                   [ Enqueue (FIFO per endpoint) ]
                                   [ Return 200 + message ID ]

                                   [ Queue ]  ◀── Poll (HTTP GET)
                                              ◀── Stream (WebSocket)
                                                                    ──▶  [ Agent receives msg ]
                                                                         [ Decrypts body ]
                                                                         [ Verifies signature ]
                                                                         [ Processes ]
                                   [ ACK ◀──────────────────────────────  ACK message ID ]
                                   [ Remove from active queue ]
```

Herald is a relay. It does not inspect, route on, transform, or act on payload contents. It receives bytes, hashes them, encrypts them, queues them, and serves them. This constraint is load-bearing: it enables BYOK encryption where Herald genuinely never sees plaintext, and it keeps the attack surface minimal.

---

## Technology Stack

```
┌─────────────────────────────────────────┐
│  nginx                                  │  Edge: TLS termination, rate limiting
├─────────────────────────────────────────┤
│  herald-server (Rust)                   │  Application: tokio + axum
├─────────────────────────────────────────┤
│  Redis                                  │  Storage: FIFO queues, in-flight tracking
└─────────────────────────────────────────┘
```

### nginx

Edge layer. TLS termination, rate limiting (`limit_req_zone`), and request size enforcement happen here before anything touches the application. Free-tier and paid-tier subdomains route to separate upstream pools.

### Rust

`tokio` + `axum` for the async runtime and HTTP/WebSocket server. Zero-cost abstractions, no GC pauses, predictable latency. Axum is the framework choice — cleaner API than actix-web for a greenfield project, first-class tower middleware ecosystem, native WebSocket support.

Herald-cli is also Rust. Single static binary, cross-compiled for macOS (aarch64, x86_64) and Linux (aarch64, x86_64). Distributed via Homebrew, cargo, and GitHub releases.

### Redis

Primary storage for the hosted service. Native list and hash primitives map directly to the queue model:

- `LPUSH` / `BRPOPLPUSH` for reliable FIFO with atomic in-flight tracking
- `EXPIRE` for retention enforcement per tier
- `INCR` / `EXPIRE` for rate limiting counters
- Pub/Sub for WebSocket fan-out (notify connected agents of new messages)

**Persistence:** AOF (appendonly) + RDB snapshots. Single instance with replicas for MVP. Redis Cluster when horizontal scaling is needed.

---

## Repositories

### `herald/` (MIT License)

The core relay server and CLI client. Self-hostable. Anyone can run their own Herald instance.

Contents:
- `herald-server` — the relay service (accepts POSTs, manages queues, serves to agents)
- `herald-cli` — the local daemon (polls/streams from a Herald instance, invokes local handlers)
- Storage backend adapters (filesystem, Redis, SQLite, PostgreSQL)
- Configuration, tests, documentation

### `herald-tools/` (Proprietary)

The hosted product at herald.tools.

Contents:
- Landing page and documentation site
- Account management (signup, billing, API key management, endpoint configuration)
- Hosted relay at `proxy.herald.tools` (runs `herald-server` with nginx + Redis)
- Tier enforcement, usage metering, billing integration

---

## URL Structure

### Hosted (proxy.herald.tools)

```
POST https://proxy.herald.tools/<customer_id>/<endpoint_name>
```

- `customer_id`: unique account identifier (URL-safe, assigned at signup)
- `endpoint_name`: user-defined name for the endpoint (e.g., `github`, `stripe`, `orchestrator`)

### Free Tier Subdomain Separation

```
POST https://free.proxy.herald.tools/<customer_id>/<endpoint_name>
```

Free-tier traffic routes to separate infrastructure. This allows:
- Dedicated hardware allocation for paid tiers
- Different rate limiting, queue depth, and retention configurations
- Isolation of free-tier abuse from paid-tier reliability

### Self-Hosted

The operator configures their own domain and URL structure. The `/<customer_id>/<endpoint_name>` path convention is the default but configurable.

---

## Message Lifecycle

### 1. Ingestion

Herald receives an HTTP POST at `/<customer>/<endpoint>`.

**Steps:**
1. Validate the request against rate limits and payload size constraints.
2. Compute two hashes:
   - `fingerprint = SHA-256(raw_body_bytes)` — content-addressable, used for deduplication.
   - `message_id = SHA-256(endpoint + received_at_nanos + raw_body_bytes)` — unique per delivery, used as primary key.
3. Check `fingerprint` against the deduplication index for this endpoint. If it exists, return early (see collision handling below).
4. Separate HTTP headers from body. Store as distinct fields.
5. Encrypt the body (see [Encryption Model](#encryption-model)).
6. Assign `received_at` timestamp (UTC, nanosecond precision).
7. Enqueue the message to the endpoint's FIFO queue.
8. Return `HTTP 200` with `{ "message_id": "<hash>", "fingerprint": "<hash>", "received_at": "<timestamp>" }`.

**Collision handling:** If `fingerprint` already exists in the active queue for this endpoint, this is a duplicate delivery from the provider. Return `HTTP 200` with `{ "fingerprint": "<hash>", "deduplicated": true }`. Do not enqueue again. Deduplication window = retention period. The `message_id` is always unique (includes timestamp), so it serves as a safe primary key even if two different endpoints receive identical payloads.

### 2. Storage

Messages sit in the queue until fetched and acknowledged, or until retention expires.

Each message record:
```
message_id:    SHA-256(endpoint + received_at_nanos + body) — unique primary key
fingerprint:   SHA-256(raw_body_bytes) — content-addressable, used for dedup
endpoint:      customer_id/endpoint_name
headers:       raw HTTP headers (stored separately, encrypted with service key)
body:          encrypted body bytes
encryption:    "service" | "byok"
key_version:   BYOK key version identifier (null for service-managed)
received_at:   UTC timestamp (nanosecond precision)
visibility:    "visible" | "invisible" (during processing)
deliver_count: number of times fetched without ACK
```

### 3. Delivery

Agent fetches messages via HTTP polling or WebSocket stream.

**Poll:** `GET /queue/<endpoint>?limit=10`
- Returns oldest visible messages, up to `limit`.
- Marks returned messages as invisible for `visibility_timeout` seconds (default: 300, configurable).
- If the agent does not ACK within the timeout, the message becomes visible again (redelivery).

**WebSocket:** `WS /stream/<endpoint>`
- Persistent connection. Messages pushed as they arrive.
- Same visibility timeout and ACK semantics.

### 4. Acknowledgment

Agent sends ACK for each processed message:
```
POST /ack/<endpoint>/<message_id>
```

ACK removes the message from the active queue. On tiers with retention, the message moves to a read-only archive (queryable but not re-delivered).

### 5. Expiration

Messages not acknowledged within the retention window are permanently deleted.

---

## Redis Data Model

The core data structure maps directly to Redis primitives:

```
queue:{customer_id}:{endpoint}      # Main FIFO (LPUSH in, RPOP out)
inflight:{customer_id}:{endpoint}   # Messages popped but not ACKed
dead:{customer_id}:{endpoint}       # Exceeded retry threshold (DLQ)
meta:{message_id}                   # Hash: payload, headers, timestamp, retry count, fingerprint
dedup:{customer_id}:{endpoint}      # Set of fingerprints for deduplication window
rate:{customer_id}                  # Counter with EXPIRE for rate limiting
```

### Queue Operations

**Enqueue (inbound POST):**
1. `SISMEMBER dedup:{cid}:{ep} {fingerprint}` — check dedup
2. `SADD dedup:{cid}:{ep} {fingerprint}` — register fingerprint
3. `HSET meta:{message_id} body {encrypted} headers {encrypted} received_at {ts} deliver_count 0 fingerprint {fp}`
4. `EXPIRE meta:{message_id} {retention_seconds}`
5. `LPUSH queue:{cid}:{ep} {message_id}`
6. `PUBLISH notify:{cid}:{ep} {message_id}` — wake WebSocket listeners

**Fetch (agent poll):**
1. `BRPOPLPUSH queue:{cid}:{ep} inflight:{cid}:{ep}` — atomic move from queue to in-flight
2. `HGETALL meta:{message_id}` — retrieve payload
3. `HINCRBY meta:{message_id} deliver_count 1`
4. Set a TTL key `visibility:{message_id}` with `EXPIRE {visibility_timeout}` — reaper watches these

**ACK:**
1. `LREM inflight:{cid}:{ep} 1 {message_id}` — remove from in-flight
2. `DEL visibility:{message_id}` — cancel reaper
3. If retention tier: move `meta:{message_id}` to archive. Otherwise: `DEL meta:{message_id}`

**NACK / Reaper (visibility timeout expired):**
1. `LREM inflight:{cid}:{ep} 1 {message_id}` — remove from in-flight
2. Check `deliver_count` against `max_retries`
3. If under threshold: `RPUSH queue:{cid}:{ep} {message_id}` — back to queue tail
4. If over threshold: `LPUSH dead:{cid}:{ep} {message_id}` — move to DLQ

### Reaper Process

A background task (tokio interval) scans for expired `visibility:{message_id}` keys. When a key expires (Redis keyspace notification on `__keyevent@0__:expired`), the reaper executes the NACK flow above. This is the at-least-once guarantee — if an agent dies mid-processing, the message returns to the queue automatically.

### Retention Enforcement

A periodic task runs `SCARD dedup:{cid}:{ep}` and purges entries + meta keys older than the tier's retention window. The dedup set uses sorted sets (`ZADD` with timestamp scores) for efficient range-based expiration when needed at scale.

---

## Storage Backends

Herald supports pluggable storage. The backend is selected at startup via configuration.

| Backend    | Use Case | Tradeoffs |
|------------|----------|-----------|
| Filesystem | Single-node, low-volume, development | Simple, no dependencies. No concurrent access safety without advisory locks. |
| SQLite     | Single-node, moderate volume | ACID, good read performance, single-writer limitation. |
| Redis      | Multi-node, low-latency, high-throughput | Fast, native list/stream primitives. Requires Redis deployment. Data durability depends on persistence config. |
| PostgreSQL | Multi-node, high-durability, production | Full ACID, row-level locking, `SKIP LOCKED` for concurrent consumers. Highest operational overhead. |

The storage interface is abstract:
```
enqueue(endpoint, message) → message_id
fetch(endpoint, limit, visibility_timeout) → [message]
ack(endpoint, message_id) → bool
nack(endpoint, message_id) → bool
dlq_move(endpoint, message_id) → bool
purge_expired(endpoint, retention_seconds) → count
```

Self-hosters choose what fits their deployment. The hosted service (`proxy.herald.tools`) uses Redis with AOF + RDB persistence. PostgreSQL is available as a backend for operators requiring stronger durability guarantees.

---

## Encryption Model

All tiers encrypt on receipt. Plaintext payloads are never stored at rest.

### Service-Managed Encryption (Free, Standard)

- Herald generates a per-account AES-256-GCM key, stored in the server's key management layer.
- Body is encrypted on receipt with the account's service key.
- On delivery, Herald decrypts the body before returning it to the agent.
- Headers are encrypted separately with the same service key.
- **Herald has access to plaintext during delivery.** This is the tradeoff for zero-config encryption.

### Bring-Your-Own-Key Encryption (Pro, Enterprise)

- Account holder provides a public key (RSA-4096 or X25519) via account settings.
- On receipt, Herald encrypts the body with the customer's public key. Herald cannot decrypt it.
- On delivery, Herald returns the encrypted blob. The agent decrypts locally with its private key.
- **Herald never sees the plaintext body after the encryption step.** The body is in RAM only during the brief ingestion window (hash computation + encryption). See [The RAM Visibility Window](#the-ram-visibility-window).

### Headers Under BYOK

Under BYOK, headers present a design choice:

- Headers often contain provider signatures (e.g., `X-Hub-Signature-256`) needed for verification.
- Headers are typically small and low-sensitivity.
- Encrypting headers with BYOK means the agent must decrypt before it can even check content-type.

**Decision:** Headers are always encrypted with the service key, even under BYOK. The body is encrypted with the customer's key. This means:
- Herald can serve headers in cleartext on delivery (it holds the service key).
- The agent receives headers (readable) + body (encrypted with its public key).
- The agent decrypts the body, then uses the headers to verify the provider signature against the decrypted body.

This resolves the BYOK/signature-verification paradox: signatures are verified agent-side, against decrypted-and-reconstructed raw bytes, using headers that traveled alongside the encrypted body.

---

## Delivery Semantics

### At-Least-Once (not exactly-once)

Exactly-once delivery is impossible in distributed systems (Two Generals Problem, FLP impossibility). Herald guarantees at-least-once delivery.

**What this means:**
- A message will be delivered at least once.
- If an ACK is lost (agent processed successfully but ACK packet dropped), the message will be redelivered after the visibility timeout.
- Agents MUST implement idempotent processing.

**Herald's idempotency support:**
- Every message has a content-addressable `message_id` (SHA-256 of body).
- Agents should track processed `message_id` values and skip duplicates.
- Ingestion-time deduplication: identical payloads to the same endpoint within the retention window are deduplicated at ingest.

### Visibility Timeout

When a message is fetched, it becomes invisible to other consumers for `visibility_timeout` seconds. This prevents multiple agents from processing the same message concurrently.

- Default: 300 seconds (5 minutes).
- Configurable per endpoint: 30s to 43200s (12 hours).
- If the agent needs more time: send a heartbeat to extend the timeout.
- If the agent crashes: message becomes visible again after timeout, incrementing `deliver_count`.

### Ordering

Messages within a single endpoint are delivered in FIFO order. However:

- Redelivered messages (after visibility timeout) may arrive out of original order.
- Concurrent consumers fetching from the same endpoint will each get different messages, but strict global ordering is not guaranteed under concurrency.

**Head-of-line blocking mitigation:** Each endpoint is an independent queue. Use multiple endpoints to partition workloads (e.g., `github-push`, `github-pr`, `github-issue` instead of a single `github` endpoint). This allows fast events to drain independently of slow ones.

---

## Dead Letter Queue

Every endpoint has an associated DLQ.

**When messages move to DLQ:**
- `deliver_count` exceeds `max_retries` (default: 3, configurable per endpoint).
- A message is explicitly NACKed with `permanent: true`.

**DLQ behavior:**
- DLQ messages are queryable: `GET /dlq/<endpoint>?limit=10`
- DLQ messages can be replayed: `POST /dlq/<endpoint>/<message_id>/replay`
- DLQ messages follow the same retention policy as the active queue.
- DLQ depth is surfaced in account metrics.

---

## Replay Protection

Webhooks are static HTTP POSTs. An intercepted request can be replayed indefinitely if the signature remains valid.

**Herald's protections:**
1. **Ingestion deduplication:** Content-addressable hashing means identical payloads are deduplicated within the retention window.
2. **Timestamp metadata:** Herald attaches `received_at` to every message. Agents should compare this against the provider's timestamp (if present in headers/body) and reject stale messages.
3. **Recommended agent-side validation:** Reject messages where `provider_timestamp` is older than 5 minutes, accounting for network transit but not queue latency. Use `received_at` (Herald's timestamp) to distinguish "delayed in queue" from "replayed from the internet."

**Note:** Content-addressable deduplication handles exact replays. It does not protect against modified replays (same semantic content, different bytes). Agents relying on provider signatures are protected against modification.

---

## Headers and Body Separation

Headers and body are stored and served as distinct fields.

### Why Separate

- Headers contain provider metadata (signatures, content-type, user-agent, idempotency keys).
- Body contains the payload (potentially large, potentially sensitive).
- Under BYOK, body is encrypted with the customer's key; headers are encrypted with the service key.
- Separating them enables Herald to route, deduplicate, and serve metadata without touching the body.

### Tier Access

| Field   | Free | Standard | Pro | Enterprise |
|---------|------|----------|-----|------------|
| Body    | Yes  | Yes      | Yes | Yes        |
| Headers | No   | Yes      | Yes | Yes        |

**Free tier receives body only.** This is a deliberate constraint:
- Free tier has no signature verification (no headers = no signature to check).
- Keeps the Free tier simple: fetch body, process, ACK.
- Upgrading to Standard unlocks headers, enabling signature verification and richer metadata.

**Self-hosted:** All features available regardless of tier (it's your instance).

---

## Rate Limiting

Rate limiting operates at two layers:

### Edge Rate Limiting (Ingestion)

Applied to inbound POST requests before they touch the queue.

| Tier       | Messages/Day | Burst Rate     |
|------------|-------------|----------------|
| Free       | 100         | 10/minute      |
| Standard   | 10,000      | 100/minute     |
| Pro        | 500,000     | 5,000/minute   |
| Enterprise | Custom      | Custom         |

Exceeding the rate limit returns `HTTP 429 Too Many Requests` with `Retry-After` header.

### Queue Depth Limiting

Maximum messages in the active queue per endpoint:

| Tier       | Max Queue Depth |
|------------|----------------|
| Free       | 100            |
| Standard   | 10,000         |
| Pro        | 100,000        |
| Enterprise | Custom         |

If the queue is full, new messages are rejected with `HTTP 507 Insufficient Storage`.

### Self-Hosted

Rate limiting is configurable. The operator can use application-level limits (built into Herald), reverse proxy limits (nginx, Caddy), or external rate limiting (Cloudflare, API gateway). Herald exposes configuration for all built-in limits.

---

## Tier Separation and Infrastructure

### Subdomain-Based Routing

The hosted service uses subdomains to separate tiers onto different infrastructure:

```
free.proxy.herald.tools   →  Free tier (shared, best-effort)
proxy.herald.tools        →  Standard, Pro, Enterprise (dedicated, SLA-backed)
```

This enables:
- **Different hardware allocation:** Free on smaller instances, paid on dedicated.
- **Different configuration:** Free with aggressive rate limits and short retention; paid with tuned performance.
- **Isolation:** Free-tier abuse (accidental or intentional) cannot degrade paid-tier service.
- **Independent scaling:** Each tier scales based on its own load profile.

### Self-Hosted

Single deployment. No tier separation. The operator configures limits and resources to match their needs.

---

## Account Tiers

### Free

- 1 endpoint
- HTTP polling only
- 100 messages/day, 10/minute burst
- 7-day retention
- Body only (no headers)
- Encrypted at rest (service-managed key)
- No signature verification
- No SLA
- Subdomain: `free.proxy.herald.tools`

### Standard ($12/month)

- 10 endpoints
- HTTP polling + WebSocket
- 10,000 messages/day, 100/minute burst
- 30-day retention
- Body + headers
- Encrypted at rest (service-managed key)
- Webhook signature verification
- Retry logic (configurable, default 3 retries)
- Dead letter queue
- Basic delivery guarantees

### Pro ($49/month)

- Unlimited endpoints
- WebSocket + priority polling
- 500,000 messages/day, 5,000/minute burst
- 90-day retention
- Body + headers
- BYOK encryption
- Delivery receipts
- Retry logic + DLQ
- Webhook signature verification
- Audit log
- Priority support

### Enterprise

- Custom volume, retention, SLA
- Private deployment available
- Custom rate limits
- Dedicated infrastructure

---

## Herald CLI (Local Daemon)

`herald-cli` is an optional, open-source binary that completes the relay circuit. It runs locally, connects to a Herald instance, and invokes configured handlers when messages arrive.

### Design Principles

- Single static binary. No runtime dependencies.
- Config-driven. YAML configuration file.
- OS-supervised: launchd (macOS), systemd (Linux). Does not self-daemonize.
- Payload delivered via stdin. Never interpolated into shell arguments.
- Reports ACK/NACK back to Herald based on handler exit code.

### Configuration

```yaml
# ~/.config/herald/config.yaml

server: https://proxy.herald.tools
api_key: hrl_sk_...

connection: websocket  # or "poll" with interval
poll_interval: 10s     # ignored if websocket

max_concurrent: 3      # max parallel handler invocations

handlers:
  github-push:
    command: claude
    args: ["-p"]
    prompt_template: |
      Use kindex to search for context on this repository.
      Then handle this GitHub push event:

      {{.body}}
    stdin: prompt       # rendered template piped to stdin
    timeout: 300s
    on_failure: nack    # "nack" (requeue) or "nack_permanent" (DLQ)

  stripe-payment:
    command: python
    args: ["handle_payment.py"]
    stdin: body         # raw body piped to stdin
    timeout: 60s
    env:
      HERALD_MESSAGE_ID: "{{.message_id}}"
      HERALD_ENDPOINT: "{{.endpoint}}"

    hooks:
      pre:
        command: kindex
        args: ["ingest", "--tags", "herald,stripe", "--stdin"]
        stdin: body     # ingest raw payload into kindex before processing
      post:
        command: kindex
        args: ["add", "--tags", "herald,stripe"]
        stdin: summary  # handler's stdout captured and sent to kindex
```

### Template Variables

| Variable       | Value |
|----------------|-------|
| `{{.body}}`    | Decrypted message body |
| `{{.headers}}` | JSON object of HTTP headers (Standard+ only) |
| `{{.message_id}}` | Content-addressable hash |
| `{{.endpoint}}` | Endpoint name |
| `{{.received_at}}` | Herald receipt timestamp |

### Execution Flow

1. Connect to Herald (WebSocket or start poll loop).
2. Receive message.
3. Run `pre` hook (if configured). If pre hook fails, NACK.
4. Render `prompt_template` (if configured) or use raw body.
5. Spawn handler process. Pipe rendered content to stdin.
6. Wait for exit (up to `timeout`).
7. Exit 0 → ACK. Nonzero → NACK (or NACK permanent, per config).
8. Run `post` hook (if configured). Post hook failure is logged but does not affect ACK.
9. Respect `max_concurrent` — queue locally if at limit.

### Security

- **No shell interpolation.** Payload is never part of command arguments. Stdin only.
- **Signature verification.** If headers are available, herald-cli can verify provider signatures before invoking the handler (configurable per endpoint with shared secret).
- **Timeout enforcement.** Handlers that exceed timeout are killed (SIGTERM, then SIGKILL after grace period).
- **No root.** Runs as the invoking user. Handlers inherit user permissions.

---

## Kindex Integration

Kindex integration is not a Herald feature. It is a herald-cli configuration pattern. Herald and kindex have no code-level dependency on each other.

### Read Path (Context Injection)

The `prompt_template` in handler config injects kindex instructions into the agent's prompt. The agent already has kindex MCP tools. Herald-cli just templates the string.

### Write Path (Passive Knowledge Accumulation)

Pre-hooks or post-hooks call kindex CLI to ingest webhook payloads or handler output. This allows kindex to passively learn from the event stream:

- **Pre-hook ingest:** Every inbound webhook is indexed before the agent processes it.
- **Post-hook capture:** The agent's stdout (summary, decision, result) is captured and indexed.

### The Product Narrative

Herald delivers the event. Kindex provides the context. The agent processes with full history. Each component works independently. Together they form an autonomous agent stack.

---

## Security Model

### Transport

- TLS 1.2+ on all inbound and outbound connections. No plaintext HTTP.
- Self-hosted operators manage their own TLS (Let's Encrypt, reverse proxy, etc.).

### Authentication

**Inbound (webhook providers → Herald):**
- No authentication required. Webhook providers POST to a URL. Herald accepts anything that passes rate limiting.
- This is intentional: webhook providers authenticate via their own signature mechanisms, verified agent-side.

**Outbound (agent → Herald):**
- API key authentication. Bearer token in `Authorization` header.
- Per-account API keys, rotatable via account management.
- API keys are scoped: read (poll/stream/ACK), write (enqueue — for testing), admin (endpoint management).

### The RAM Visibility Window

Herald processes the raw body in memory during ingestion:
1. Read raw bytes from the HTTP request.
2. Compute SHA-256 hash.
3. Encrypt body (service key or BYOK public key).
4. Write encrypted body to storage.
5. Zero the plaintext buffer.

**Between steps 1 and 4, the plaintext exists in RAM.** This is unavoidable — Herald must read the bytes to hash and encrypt them. Under BYOK, this is the only moment Herald has access to plaintext. We are explicit about this because "your data is encrypted" should not imply "we never see it." We see it for the duration of ingestion. We do not log it, inspect it, route on it, or persist it in cleartext.

**For true end-to-end encryption where the relay never sees plaintext:** The webhook provider must encrypt before sending. This is outside Herald's control and not currently supported by major providers (GitHub, Stripe, etc.).

### Webhook Signature Verification

Provider signatures (e.g., `X-Hub-Signature-256`, `Stripe-Signature`) are verified **agent-side**, not at the relay edge.

**Why not verify at the edge?**
- Edge verification requires the provider's shared secret to be stored on Herald's servers.
- Under BYOK, this contradicts the zero-trust model.
- Under service-managed encryption, it's viable but creates a single point of compromise for all provider secrets.

**How it works:**
1. Herald stores headers separately (encrypted with service key).
2. On delivery, agent receives headers (decrypted by Herald) + body (decrypted by Herald or by agent under BYOK).
3. Agent extracts the signature header.
4. Agent computes HMAC-SHA256 over the raw body using its locally-stored provider secret.
5. Agent compares. Match = authentic. Mismatch = drop.

**Byte preservation requirement:** Herald must preserve the exact raw bytes of the body through the encrypt/decrypt cycle. Any transformation (JSON parsing, whitespace normalization, encoding conversion) will break signature verification. Herald stores and serves raw bytes. It does not parse payload contents.

### Prompt Injection and SSRF

Webhook payloads are untrusted input. When an AI agent processes a webhook, the payload enters the agent's context window. A malicious or compromised webhook provider could craft payloads containing prompt injection attacks.

**Herald's position:** This is an agent-side responsibility, not a relay-side responsibility. Herald is a message transport. It does not inspect or sanitize payloads (and under BYOK, it cannot).

**Recommended agent-side mitigations:**
- Run handlers in sandboxed environments with minimal permissions.
- Do not grant webhook-triggered agents write access to critical systems without human-in-the-loop confirmation.
- Use separate agent identities for webhook processing vs. privileged operations.
- Validate payload structure before acting on content.

### Audit Log

Available on Pro and Enterprise tiers. Records:
- All inbound messages (message_id, endpoint, timestamp, source IP, size).
- All deliveries (message_id, timestamp, consumer identity).
- All ACK/NACK events.
- API key usage.
- Endpoint configuration changes.

---

## API Surface

### Inbound (Webhook Providers)

```
POST /<customer_id>/<endpoint_name>
  Body: raw webhook payload
  Headers: provider headers (forwarded and stored)
  → 200 { message_id, received_at }
  → 429 Too Many Requests
  → 507 Queue Full
  → 413 Payload Too Large
```

### Agent (Polling)

```
GET /queue/<endpoint_name>?limit=10&visibility_timeout=300
  Auth: Bearer <api_key>
  → 200 { messages: [{ message_id, body, headers?, received_at, deliver_count }] }
  → 204 No Content (queue empty)

POST /ack/<endpoint_name>/<message_id>
  Auth: Bearer <api_key>
  → 200 { acknowledged: true }

POST /ack/<endpoint_name>
  Auth: Bearer <api_key>
  Body: { "message_ids": ["<id1>", "<id2>", ...] }
  → 200 { acknowledged: ["<id1>", "<id2>"], failed: [] }

POST /nack/<endpoint_name>/<message_id>?permanent=false
  Auth: Bearer <api_key>
  → 200 { requeued: true } or { dlq: true }

POST /heartbeat/<endpoint_name>/<message_id>?extend=300
  Auth: Bearer <api_key>
  → 200 { visibility_timeout_extended: true }
```

### Agent (WebSocket)

```
WS /stream/<endpoint_name>
  Auth: first-message authentication (see below)
  Client → Server: { type: "auth", api_key: "hrl_sk_..." }   # MUST be first frame
  Server → Client: { type: "auth_ok" } or { type: "auth_error", reason: "..." }
  Server → Client: { type: "message", message_id, body, headers?, received_at }
  Client → Server: { type: "ack", message_id }
  Client → Server: { type: "nack", message_id, permanent: false }
  Client → Server: { type: "heartbeat", message_id }
```

**First-message auth:** API keys are never sent in query parameters (they leak into server logs, proxy logs, and browser history). The client opens a WebSocket connection, sends an auth frame as the first message, and receives either `auth_ok` or `auth_error`. No messages are delivered until authentication succeeds. Connections that do not authenticate within 5 seconds are closed.

### Dead Letter Queue

```
GET /dlq/<endpoint_name>?limit=10
  Auth: Bearer <api_key>
  → 200 { messages: [...] }

POST /dlq/<endpoint_name>/<message_id>/replay
  Auth: Bearer <api_key>
  → 200 { replayed: true, new_message_id: ... }
```

### Registration

```
POST /register
  Body: { "customer_id": "my-agent" }
  → 201 { customer_id, api_key: "hrl_sk_...", created: true }
  → 200 { customer_id, api_key: "hrl_sk_...", created: false }  # idempotent
  → 400 Bad Request (empty or invalid customer_id)
```

Programmatic account creation. Returns an API key for polling/ack operations.
Idempotent: calling with the same `customer_id` returns the existing key.
No authentication required — the returned API key is the credential.

### Account Management (herald-tools only)

```
POST   /account/endpoints          — create endpoint
GET    /account/endpoints          — list endpoints
DELETE /account/endpoints/<name>   — delete endpoint
PUT    /account/keys/byok         — upload BYOK public key
POST   /account/keys/rotate       — rotate API key
GET    /account/usage             — usage metrics
GET    /account/audit             — audit log (Pro+)
```

### Payload Size Limits

| Tier       | Max Body Size |
|------------|--------------|
| Free       | 64 KB        |
| Standard   | 1 MB         |
| Pro        | 10 MB        |
| Enterprise | Custom       |

Exceeding the limit returns `HTTP 413 Payload Too Large`.

---

## Competitive Positioning

Herald exists in a populated market. Its differentiation is narrow and intentional.

### What Herald Is Not

- **Not a tunnel.** ngrok, Cloudflare Tunnel, and Pinggy are synchronous pipes. If the agent is offline, the webhook fails. Herald queues.
- **Not an enterprise webhook gateway.** Hookdeck, Svix, and Convoy are push-based delivery platforms with complex routing, transformation, and fan-out. Herald is pull-based and does not transform payloads.
- **Not a cloud queue.** SQS, Pub/Sub, and Cloud Tasks require cloud accounts, IAM configuration, and provider SDKs. Herald is a single binary with a stable URL.

### What Herald Is

A **pull-based, encrypted, self-hostable webhook queue** with a companion CLI daemon for local agent activation. It is opinionated:

- **Pull, not push.** The agent decides when to consume.
- **Encrypt on receipt.** No plaintext at rest, ever.
- **Content-addressable.** Built-in deduplication.
- **Self-hostable.** MIT licensed. Bring your own storage backend.
- **Agent-native.** Designed for things that aren't always on — CLI tools, local LLMs, cron jobs, development agents.

### Key Competitors

| Tool | Model | Queue? | Pull? | Self-Host? | Encryption? | Agent-Oriented? |
|------|-------|--------|-------|-----------|-------------|----------------|
| smee.io | EventSource stream | No | No | No | No | No |
| ngrok | Tunnel | No | No | No | Transit only | No |
| Hookdeck | Push gateway | Yes | CLI only | No | At rest | No |
| Webhook Relay | Push + tunnel | Yes | No | No | Transit | No |
| AgentWebhook | Pull relay | Yes | Yes | No | Unknown | Yes |
| Handoff | Agent messaging | Yes (Redis) | Yes (SSE) | Yes | No | Yes |
| AWS API GW + SQS | Cloud queue | Yes | Yes | N/A | At rest (KMS) | No |
| **Herald** | **Pull relay** | **Yes** | **Yes** | **Yes (MIT)** | **At rest + BYOK** | **Yes** |

Herald's unique position: the only self-hostable, pull-based, encrypted webhook queue with a local daemon designed for AI agent activation. The closest competitor is Handoff (open-source, agent-oriented, pull-based), but Handoff lacks encryption, content-addressable deduplication, and the local daemon activation model.

---

## Resolved Decisions

Decisions resolved via adversarial review (Advocate, 6-persona, 22 findings):

1. **WebSocket authentication:** Resolved. First-message auth is the only supported pattern. API keys are never sent in query parameters. Connections that do not authenticate within 5 seconds are closed.

2. **Message ID vs. fingerprint:** Resolved. Two hashes per message: `fingerprint` (SHA-256 of body, for dedup) and `message_id` (SHA-256 of endpoint + timestamp + body, for unique primary key). Prevents the theoretical collision where two endpoints receiving identical payloads would share an ID.

3. **Batch ACK:** Resolved. `POST /ack/<endpoint>` accepts `{ "message_ids": [...] }` for batch acknowledgment. Needed at Pro-tier volumes (500K messages/day).

4. **BYOK headers transparency:** Resolved. Headers are encrypted with service key even under BYOK. This is documented explicitly in the Encryption Model section. Users expecting BYOK to cover all data should understand that headers remain Herald-readable to enable signature verification and content-type serving.

5. **BYOK key rotation:** Resolved. Each encrypted message stores a `key_version` identifier (opaque string, set by Herald when the customer uploads a new public key). Customer uploads a new public key via `PUT /account/keys/byok` — Herald assigns it the next version and starts encrypting new messages with it. Existing messages retain their original `key_version`. On delivery, the `key_version` is included in the message envelope so the agent knows which private key to use for decryption. Herald only ever holds public keys. Key management (which private key corresponds to which version) is entirely the customer's responsibility.

---

## Open Questions

1. **Multi-region:** Should the hosted service support geographic routing? Not in v1.
2. **Webhook forwarding (push mode):** Some users may want Herald to push to a URL instead of waiting for a pull. Out of scope for v1 — this is what Hookdeck does.
3. **Message priority:** Within an endpoint, should some messages be deliverable before others? Not in v1. Use multiple endpoints for priority separation.
