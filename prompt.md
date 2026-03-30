# Herald Webhook Relay System

## System Context
Herald is an asynchronous webhook relay and message queue system that provides stable HTTP endpoints for receiving webhooks, encrypts payloads immediately, and serves them to agents via pull-based delivery. The system operates as a pure relay - it never inspects, transforms, or routes on payload contents, only receives, hashes, encrypts, queues, and serves bytes.

Users include AI agents (Claude Code, local LLMs, CLI tools), local services behind firewalls, and webhook providers (GitHub, Stripe, etc.). The system serves multiple tiers: Free (1 endpoint, 100 msg/day), Standard ($12/month, 10 endpoints), Pro ($49/month, unlimited endpoints with BYOK), and Enterprise (custom).

## Consequence Map
1. **CRITICAL**: Message loss or ordering violations break at-least-once delivery guarantees, potentially causing missed business events or duplicate processing
2. **HIGH**: Encryption key compromise exposes all stored webhook payloads, violating customer trust and compliance requirements
3. **HIGH**: Queue depth overflows cause HTTP 507 responses, dropping inbound webhooks from critical providers
4. **MEDIUM**: Visibility timeout bugs cause message redelivery storms, overwhelming downstream agents
5. **MEDIUM**: Rate limiting failures allow abuse of free tiers or cause legitimate traffic rejection
6. **LOW**: Head-of-line blocking degrades agent processing performance but doesn't break functionality

## Failure Archaeology
The system evolved from architectural decisions made on 2026-03-30, implementing a two-repo structure (herald/ MIT, herald-tools/ proprietary) and pull-based delivery model. Key lessons learned:
- WebSocket query param authentication proved insecure; moved to first-message auth
- Single hash system had collision risks; implemented dual hash (fingerprint + message_id)
- Agent-side signature verification enables zero-trust BYOK model but requires agent implementation
- BYOK headers encryption with service keys provides acceptable security/usability balance

## Dependency Landscape
**Upstream**: nginx (TLS termination, rate limiting), webhook providers (GitHub, Stripe, etc.), external services posting to Herald endpoints

**Core**: Rust/tokio/axum application server, Redis (FIFO queues), pluggable storage backends (filesystem, SQLite, Redis, PostgreSQL), API key authentication, deduplication index

**Downstream**: AI agents via HTTP polling or WebSocket, herald-cli daemon, agent handler processes

**External**: Account management for hosted service, audit logging (Pro+ tiers), tier-based subdomain routing

## Boundary Conditions
**Scope**: Pure relay service with at-least-once delivery, FIFO ordering within endpoints, content-addressable deduplication, encryption at rest, pull-based agent delivery

**Non-goals**: Not a tunnel (ngrok), push gateway (Hookdeck), or cloud queue service. No payload parsing, transformation, or routing. No webhook forwarding or push delivery in v1. No multi-region or message priority in v1.

**Constraints**: Herald-cli remains separate binary, no SDKs required for core service, agent-side signature verification only, key management for BYOK is customer responsibility

## Success Shape
A reliable webhook relay that preserves message ordering and content while providing strong encryption and flexible delivery modes. Solutions should maintain the pure relay abstraction, support pluggable storage backends for different deployment scenarios, and enable zero-trust BYOK encryption. The system must handle tier-based isolation, graceful degradation under load, and provide clear failure modes for agent debugging.

## Done When
- Messages delivered at-least-once with configurable visibility timeout and DLQ after max retries
- Content-addressable deduplication using SHA-256 fingerprints within retention window
- All payloads encrypted at rest with service-managed or BYOK keys
- FIFO delivery order maintained within single endpoint
- Both HTTP polling and WebSocket streaming APIs functional
- Rate limiting and queue depth limits enforced per tier
- Headers and body stored/served separately
- WebSocket first-message authentication implemented
- Batch ACK support for high-volume scenarios
- Herald-cli executes handlers with stdin delivery and exit code ACK/NACK

## Trust and Authority Model
The system operates with a trust floor of 0.10 and authority override floor of 0.40, using decay lambda 0.05 for trust degradation. Data is classified into five tiers: PUBLIC (1h soak), PII (6h soak), FINANCIAL (24h soak), AUTH (48h soak), and COMPLIANCE (72h soak). Higher tiers require longer canary soaks and human approval gates.

Authority is distributed across components based on data domain ownership. The auth service owns authentication tokens and user credentials, the queue manager owns message ordering and delivery state, and the encryption service owns key management and payload protection. Human gates are required for all FINANCIAL, AUTH, and COMPLIANCE changes, plus low-trust authoritative operations and unresolvable conflicts.

The system uses taint locking for PII, FINANCIAL, AUTH, and COMPLIANCE tiers, with conflict resolution requiring 0.20 trust delta threshold. Target soak validation requires 1000 requests minimum across all tiers.

## Component Topology
The system consists of seven core components: an API Gateway handling ingestion and authentication, a Queue Manager maintaining FIFO ordering and delivery state, an Encryption Service managing payload and key security, a Storage Backend providing pluggable persistence, an Agent API serving polling and WebSocket delivery, a CLI Daemon executing local handlers, and an Auth Service managing user credentials and API keys.

Data flows from external webhooks through the API Gateway (HTTP POST with authentication), to the Encryption Service (immediate payload encryption), to the Queue Manager (FIFO queuing with message IDs), to Storage Backend (persistence), and finally to Agent API (pull-based delivery via HTTP or WebSocket). The CLI Daemon connects to Agent API and executes local handlers with stdin payload delivery. All inter-component communication uses HTTP/gRPC protocols with TLS encryption.