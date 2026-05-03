# Herald Webhook Relay and Message Queue

## System Context
Herald is a webhook relay and message queue designed to bridge the gap between webhook providers (GitHub, Stripe, etc.) that expect always-on consumers and intermittent AI agents or local services that cannot expose public endpoints. It provides stable URLs for webhook ingestion, encrypts and queues payloads using BYOK (Bring Your Own Key) encryption, and serves them back via HTTP polling or WebSocket when agents are ready to consume.

The system serves multiple stakeholders: AI agents running on laptops or cron jobs, self-hosted operators who want full control, and Herald-tools hosted service customers across free, standard, pro, and enterprise tiers. The architecture is designed as a single static binary with cross-platform support and pluggable storage backends.

## Consequence Map
1. **Critical**: Storage backend conflicts serving stale/inconsistent message state - could cause duplicate processing, financial double-charges, or missed critical events
2. **Critical**: Free-tier abuse degrading paid-tier service - revenue loss and SLA violations for paying customers
3. **High**: BYOK encryption promise violation - plaintext exposure would breach fundamental security guarantee
4. **High**: Rate limit failures causing webhook provider retries - could amplify load and cascade failures
5. **Medium**: Agent processing failures with improper visibility timeout - message loss or endless redelivery loops
6. **Medium**: Prompt injection via malicious webhook payloads - could compromise agent systems
7. **Low**: ACK loss causing duplicate delivery - handled by required agent idempotency

## Failure Archaeology
The system acknowledges fundamental distributed systems limitations: exactly-once delivery is impossible (Two Generals Problem, FLP impossibility), requiring agents to implement idempotent processing. Previous attempts at webhook forwarding were abandoned for v1 due to complexity. The decision to make provider signature verification agent-side rather than relay-side prevents Herald from needing to understand every provider's authentication scheme while preserving the ability to verify signatures from exact raw bytes.

## Dependency Landscape
**Core Infrastructure**: Redis (primary hosted storage), PostgreSQL/SQLite/Filesystem (alternative backends), nginx (edge layer, TLS, rate limiting), tokio + axum (async runtime)
**External**: Webhook providers, TLS 1.2+ infrastructure, API key authentication systems
**Touches**: Agent systems, self-hosted environments, multi-tier hosted infrastructure
**Touched By**: Webhook providers via HTTP POST, agents via HTTP polling/WebSocket, operators via CLI tools

## Boundary Conditions
**In Scope**: Webhook ingestion, encryption, queueing, pull-based consumption, content-addressable deduplication, tier-based rate limiting, FIFO ordering within endpoints, at-least-once delivery
**Out of Scope**: v1 excludes webhook forwarding (push mode), message priority within endpoints, multi-region support, payload inspection/routing/transformation, provider signature verification at edge
**Constraints**: Single static binary, MIT license, zero-cost abstractions, predictable latency, true BYOK where Herald never sees plaintext post-encryption

## Success Shape
A solution that provides webhook stability without webhook complexity. It should feel like a Unix tool: does one thing well, composes cleanly, fails predictably. The encryption model should be genuinely private (BYOK), the delivery model should be pull-based for agent control, and the storage model should be pluggable for deployment flexibility. Performance characteristics should be predictable with no GC pauses.

## Done When
- HTTP POST to Herald generates content-addressable message ID, encrypts payload, enqueues, returns 200 + ID
- Agents drain queues via HTTP polling or WebSocket with FIFO ordering within endpoint
- Content-addressable fingerprinting prevents duplicate processing within retention window
- BYOK encryption ensures Herald never accesses plaintext body after encryption step
- Visibility timeout prevents concurrent processing, with dead letter queue for retry failures
- Rate limiting enforced at ingestion edge and queue depth per tier
- Headers and body stored/served as distinct fields with appropriate encryption levels
- API follows conventions: prefixed identifiers, object discriminators, consistent error responses
- herald-cli daemon invokes configured handlers for incoming messages

## Trust and Authority Model
The system operates on a tiered trust model where different data classifications require different soak periods and human oversight. Webhook bodies containing potentially sensitive data (financial, PII, auth tokens) require 6-48 hour soak periods before deployment changes. The ingestion layer has authority over webhook validation and rate limiting. The queue layer owns message ordering and delivery guarantees. The encryption layer owns key management and plaintext isolation. Human gates trigger for all financial, auth, and compliance data changes, plus any low-trust scenarios involving authoritative components.

## Component Topology
The system consists of an ingestion service that receives webhooks and generates message IDs, an encryption service that handles BYOK encryption without seeing plaintext, a queue service that maintains FIFO ordering and visibility timeouts, a consumption service that serves via HTTP/WebSocket, and a CLI daemon that connects consumers to Herald instances. Data flows from webhook providers through ingestion to encryption to queueing to consumption to agents, with rate limiting applied at ingestion and queue depth monitoring throughout.