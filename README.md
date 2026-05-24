# tenzro-edge

**A Pi-class runtime for participating in the Tenzro Network.**

`tenzro-edge` makes a Pi-class node a first-class participant in the [Tenzro
Network](https://tenzro.com): identity, wallet, multi-VM, inference, model
providing, storage, training, settlement, agentic workflows.

Tenzro already publishes a Rust SDK (`tenzro-sdk-rust`). What `tenzro-edge`
adds is the **Pi-shaped layer**: rustls + aws-lc-rs TLS (no OpenSSL), curated
minimal Tokio features, token persistence with auto-refresh, offline-buffer
+ replay, flaky-link retry tuning (LTE/satellite-friendly), env-var creds for
unattended boot, and a CLI that mirrors `minima-attest`'s shape.

```bash
tenzro-edge login                           # one-shot login, persists token
tenzro-edge status                          # token TTL + last sync
tenzro-edge replay                          # flush offline-buffered events
```

---

## Where it sits

```
github.com/furcateai/
├── furcate-protocol                     wire-format specs + schemas
├── furcate-inference                    edge inference kernel
├── furcate-mesh                         LAN peer fabric for edge nodes
├── minima-attest                        Rust client for anchoring hashes on a local Minima node
├── tenzro-edge        ← you are here    runtime for participating in the Tenzro Network
├── prvnz-edge                           runtime for issuing PRVNZ Digital Product Passports
├── furcate-pi-hat                       Pi 5 HAT hardware support (GPIO, 1-Wire, OPC UA triggers)
└── furcate-pi-minima                    supervisor for running a Minima full node on a Pi
```

`tenzro-edge` is the Tenzro-specific participation crate in this set. It provides reference impls for the Tenzro Network surface (identity, settlement, inference, model providing, storage, agent invocation) that any consumer of `furcate-inference` + `furcate-mesh` can wire in.

## What it does — and why

Tenzro is a **global** network — identity, wallet, multi-VM coordination, inference, model providing, storage, training, settlement, agent payments (AP2), DIDs (TDIP). Furcate is the **edge** plane — Pi-class nodes that may be offline, intermittent, or LAN-only.

`tenzro-edge` is the bridge: it lets a Pi participate in Tenzro's global plane at full surface area despite Pi-class constraints (1 GB RAM, intermittent LTE, ARM, unattended boot).

In the Furcate trait surface (`furcate-inference-core`), this crate provides reference impls of:

| Trait | Tenzro module |
|---|---|
| `Attester` | `tenzro-sdk::identity` (TDIP DIDs) + `attest` |
| `ReceiptSink` | `tenzro-sdk::settlement` + `nanopayment` |
| `RemoteInferenceProvider` | `tenzro-sdk::inference` |
| `ModelSource` | `tenzro-sdk::model_providing` |
| `StorageBackend` | `tenzro-sdk::storage` |
| `AgentRegistry` | `tenzro-sdk::agent` + `skill` + `tool` |
| `AgentInvoker` | `tenzro-sdk::agent_payments` + `ap2` |
| `WorkSettlement` | `tenzro-sdk::settlement` + `nanopayment` |

In `furcate-mesh-core`:

| Trait | Tenzro module |
|---|---|
| `DiscoveryBackend` | `tenzro-sdk::provider::list_providers()` |
| `WorkBroker` | `tenzro-sdk::task` marketplace + settlement |

A Furcate node configured with `tenzro-edge` enabled gets WAN reach, model providers, agent invocation, and on-network settlement — all fail-soft, never on the critical path.

## Pi-class concerns we handle

- **TLS backend**: rustls + aws-lc-rs (FIPS-eligible), no OpenSSL. Saves 8+ MB of binary, removes a C toolchain dep.
- **Tokio features**: curated minimum (`rt`, `net`, `time`, `sync`) instead of `full`.
- **Token persistence**: `~/.furcate/tenzro/token.json` (`0600`), auto-refresh on expiry, env-var password sourcing for unattended boot (`TENZRO_PASSWORD`).
- **Offline buffer + replay**: queue receipts/events to redb when the link is down, flush on reconnect.
- **Flaky-link retry**: LTE/satellite-tuned exponential backoff + circuit breaker.
- **CLI**: `tenzro-edge login / status / replay / commit / verify` — mirrors `minima-attest` CLI shape.
- **DPoP**: RFC 9449 + RFC 7638 thumbprint key handling on-device (autonomous-agent friendly).

## Crate layout

```
crates/
├── tenzro-edge-core    # Kernel-trait impls (Attester, Sink, RemoteInference,
│                        ModelSource, StorageBackend, AgentRegistry,
│                        AgentInvoker, WorkSettlement, DiscoveryBackend,
│                        WorkBroker) — re-exports from tenzro-sdk-rust
│                        with Pi-tuned defaults
└── tenzro-edge-cli     # `tenzro-edge` binary
```

## Quick start

```bash
# Build
cargo build --workspace

# First-time login (interactive)
cargo run -p tenzro-edge-cli -- login

# Or unattended via env
export TENZRO_PASSWORD=...
cargo run -p tenzro-edge-cli -- login --account my-pi-01

# Check token + sync status
cargo run -p tenzro-edge-cli -- status

# Flush any offline-buffered events
cargo run -p tenzro-edge-cli -- replay
```

In `furcate.toml`:

```toml
[attesters.tenzro]
type = "tenzro"
endpoint = "https://rpc.tenzro.network"
password_env = "TENZRO_PASSWORD"

[receipt_sinks.tenzro]
type = "tenzro-settlement"
attester = "tenzro"

[model_sources.tenzro]
type = "tenzro-provider"

[agent_invoker.tenzro]
type = "tenzro-agent"
```

## Tenzro SDK pinning

`tenzro-edge` pins `tenzro-sdk-rust` to a specific rev until the SDK reaches a tagged release:

```toml
tenzro-sdk = { git = "https://github.com/tenzro/tenzro-sdk-rust", rev = "536363b41c2a6e62ee387dcf6e26bbdcc3ade660" }
```

Pin is updated on a deliberate rev-bump cadence, not floating.

## What this is **not**

- Not a fork of `tenzro-sdk-rust`. We depend on upstream, not fork it.
- Not the full Tenzro surface for general-purpose hosts — Tenzro's own SDK is that. This is only the Pi-class participation layer.
- Not a wrapper for one Tenzro module. It bridges *all* of Tenzro's surface that Furcate's traits map to.

## Fail-soft guarantees

Every trait impl is **fail-soft** (warn-and-continue, never propagate to the agent loop). If Tenzro is unreachable:

- `Attester::sign` → returns an error variant; the receipt is still written locally and queued for replay
- `ReceiptSink::write` → queued in offline buffer, flushed on reconnect
- `RemoteInferenceProvider::infer` → caller falls back to local engine or LAN mesh
- `DiscoveryBackend::peers` → empty stream; mDNS continues to find LAN peers
- `WorkBroker::offer` → rejects; caller's higher-priority broker handles it

## Status

- Version: **0.1.0** (scaffold)
- Trait impl skeletons in place; real wiring lands in v0.1.x
- Pi-class defaults (rustls + curated Tokio + offline buffer) wired

## Versioning

- This crate releases **independently** of the `furcate-inference` / `furcate-mesh` kernel (own cadence)
- Pins `furcate-inference-core` to a specific major version
- Pins `tenzro-sdk-rust` to a specific rev

MSRV, 1.0 timing, and deprecation windows are roadmap decisions and are not set here.

## Sibling repos

- [`furcate-protocol`](https://github.com/furcateai/furcate-protocol) — wire-format specs + schemas (Tenzro proof shape is in `specs/02-attestation.md` under `kind = "tenzro"`)
- [`furcate-inference`](https://github.com/furcateai/furcate-inference) — edge inference kernel (this crate implements 8 traits from `furcate-inference-core`)
- [`furcate-mesh`](https://github.com/furcateai/furcate-mesh) — LAN peer fabric (this crate implements 2 traits from `furcate-mesh-core`)
- [`minima-attest`](https://github.com/furcateai/minima-attest) — Rust client for anchoring hashes on a local Minima node
- [`prvnz-edge`](https://github.com/furcateai/prvnz-edge) — runtime for issuing PRVNZ Digital Product Passports (composes `minima-attest` + `tenzro-edge`)
- [`furcate-pi-hat`](https://github.com/furcateai/furcate-pi-hat) — Pi 5 HAT hardware support
- [`furcate-pi-minima`](https://github.com/furcateai/furcate-pi-minima) — supervisor for running a Minima full node on a Pi

## Upstream

- Tenzro Network: <https://tenzro.com>
- Tenzro SDK (Rust): <https://github.com/tenzro/tenzro-sdk-rust>

## License

Apache License 2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
