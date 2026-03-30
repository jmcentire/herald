# Herald

Your agent doesn't need to be always-on. Herald is.

Lightweight webhook relay and message queue for AI agents and local services that can't expose a public endpoint. Give your agent a stable URL. Herald receives, queues, and delivers.

**[herald.tools](https://herald.tools)** | **[Specification](SPEC.md)** | **[proxy.herald.tools](https://proxy.herald.tools/health)**

## How it works

```
GitHub/Stripe/etc.  ──POST──▶  Herald  ◀──poll/ws──  Your Agent
                               (queue)                (when ready)
```

1. Point webhooks at `https://proxy.herald.tools/<you>/<endpoint>`
2. Herald encrypts and queues them (FIFO, deduplicated)
3. Your agent polls or streams via WebSocket when it's ready
4. ACK processed messages. Failed? Requeued or sent to DLQ.

## Quick start

```bash
# Send a webhook
curl -X POST https://proxy.herald.tools/myagent/github \
  -H "Content-Type: application/json" \
  -d '{"action":"push","ref":"refs/heads/main"}'

# Poll for messages
curl -H "Authorization: Bearer $API_KEY" \
  https://proxy.herald.tools/queue/github

# Acknowledge
curl -X POST -H "Authorization: Bearer $API_KEY" \
  https://proxy.herald.tools/ack/github/<message_id>
```

## Self-hosting

Herald is a single Rust binary + Redis.

```bash
# Clone and build
git clone https://github.com/jmcentire/herald.git
cd herald
cargo build --release -p herald-server

# Run (requires Redis)
HERALD_REDIS_URL=redis://127.0.0.1/ \
HERALD_ENCRYPTION_KEY=$(openssl rand -hex 32) \
./target/release/herald-server
```

## herald-cli

Optional local daemon that polls Herald and invokes your agent.

```yaml
# ~/.config/herald/config.yaml
server: https://proxy.herald.tools
api_key: hrl_sk_...
connection: websocket

handlers:
  github-push:
    command: claude
    args: ["-p"]
    prompt_template: |
      Use kindex to search for context.
      Then handle this event: {{.body}}
    stdin: prompt
    hooks:
      pre:
        command: kindex
        args: ["ingest", "--tags", "herald", "--stdin"]
```

```bash
cargo build --release -p herald-cli
./target/release/herald-cli run
```

## Stack

- **nginx** — edge, TLS, rate limiting
- **Rust** — tokio + axum, zero-cost abstractions
- **Redis** — FIFO queues, in-flight tracking, pub/sub

## Features

- Encrypted on receipt (AES-256-GCM, no plaintext at rest)
- Content-addressable deduplication (SHA-256)
- At-least-once delivery with visibility timeout
- Dead letter queue after configurable retries
- WebSocket streaming with first-message auth
- Pluggable storage (Redis, PostgreSQL, SQLite, filesystem)
- BYOK encryption (Pro tier)
- Self-hostable, MIT licensed

## Ecosystem

Herald is the ears. [Kindex](https://kindex.tools) is the memory. Your agent is the brain.

Part of the [Exemplar](https://exemplar.tools) stack.

## License

MIT
