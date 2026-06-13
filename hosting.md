# Hosting — Self-Hosted First (v2)

**Date:** 2026-06-12 (v2 — DevOps-by-team track adopted, ADR-005) · Prices verified online June 2026.

## The decision

Team works for free and wants control; too many third parties is its own complexity. So: **self-host everything non-user-facing on one dedicated server from day 1.** Phase 1 needs only a website, downloads, CI, and crash reporting — none of it touches users' accounts or money, so the failure cost of owning it ourselves is low, and Rust CI is actually *better* on our own hardware. The one place self-hosting is deferred to a gate, not a default: **identity + money in Phase 2** (auth, token purchases, payouts). Stripe is unavoidable regardless; self-hosted auth is allowed *if* the team passes the ops-discipline gate below.

## The stack (one box)

| Piece | Choice | Notes | Cost |
|---|---|---|---|
| Server | **Hetzner AX42** (Ryzen, 64 GB ECC, 2×512 GB NVMe) | Runs everything below; unmetered 1 Gbit traffic covers installer downloads | ~€59/mo |
| DNS + CDN + DDoS | **Cloudflare free** in front | Free caching of downloads worldwide; COOP/COEP headers; the only third party kept | €0 |
| Web | **Caddy** | Site, docs (Astro Starlight), downloads, auto-TLS, headers | €0 |
| CI | **Self-hosted GitHub Actions runner** on the box (GitHub Free plan) | 16 threads + local cache demolish hosted-runner times for Rust; GitHub's self-hosted surcharge is postponed — recheck if revived | €0 |
| Crash reporting | **GlitchTip** (not self-hosted Sentry — that's a heavy multi-service beast) | Speaks the Sentry SDK wire protocol: same Rust/browser SDKs, just our DSN. Handles millions of events/mo on one node | €0 |
| Telemetry | **PostHog hobby** (docker-compose) | Fine at our scale; unsupported by PostHog officially — acceptable, it's not user-facing | €0 |
| Data | **Postgres** (one instance, all services) | | €0 |
| Backups | **Hetzner Storage Box** + restic, nightly, **restore tested monthly** | Backups that aren't restore-tested don't exist | ~€4/mo |
| Monitoring | Uptime Kuma + node exporter | Alerts to the team chat | €0 |

**Total: ~€63–75/mo flat.** Comparable to the managed stack's $46–120, but flat, predictable, and fully ours. The price is paid in team hours: budget ~half a day/week of ops once it's running, more during setup week.

## Phase 2 gate — identity & money (the one reservation)

Before token packs and accounts launch, two rules:
1. **Stripe** for all payments and creator payouts (Stripe Connect) — never self-hosted, not negotiable, plus the legal review of the token economy (ADR-004).
2. **Auth gate:** the team may self-host auth (Supabase self-hosted or Keycloak on the same box) *only if* by then: monthly restore tests have passed 3+ months running · monitoring/alerting has caught at least one real incident · someone owns security patching as a standing duty. Fail any → use managed auth (Supabase cloud, $25/mo) and feel no shame; it's one container swapped later.

Phase 2 also adds (still self-friendly): collab WebSocket service (a Rust binary on the same box — it's our own code anyway) · generation providers (API COGS, scales with token revenue — unavoidable third party) · a second box or failover IP when real users depend on uptime (~+€60/mo).

## Setup tasks (ordered, ~1 week part-time)

1. Order AX42, base hardening: SSH keys only, ufw, fail2ban, unattended-upgrades, docker. *(half day)*
2. Cloudflare free: DNS for metrocalk.com, proxy on, COOP/COEP + cache rules for `/downloads`. *(1 h)*
3. Caddy + skeleton site/docs deployed via git push. *(half day)*
4. GitHub Free repo + self-hosted runner with rust-cache; add the wasm-tripwire job from Spike ③. *(half day)*
5. GlitchTip via docker-compose; wire Rust + browser DSNs at M2–3. *(2 h)*
6. PostHog hobby via docker-compose; define telemetry event names (`bind_started`, `bind_completed`, `undo_invoked`…) before M3. *(2 h)*
7. restic → Storage Box, nightly cron; **calendar reminder: monthly restore test.** *(2 h)*
8. Uptime Kuma + alerts. *(1 h)*
9. Write `ops.md` runbook: how to deploy, restore, rotate keys, who's on point. *(2 h — this is the gate evidence)*

## Exit paths (kept warm, no lock-in)

Every piece has a managed escape hatch if ops time starts eating build time: site → Vercel/Cloudflare Pages · crash → Sentry SaaS (same SDKs, change DSN) · telemetry → PostHog cloud · CI → hosted runners · auth → Supabase cloud. The reverse migration (managed → self-hosted) was the v1 plan of this file; both directions stay cheap because nothing here is exotic.

## Sources

[Hetzner AX42](https://www.hetzner.com/dedicated-rootserver/ax42/) · [Hetzner AX line](https://www.hetzner.com/dedicated-rootserver/matrix-ax/) · [GlitchTip vs self-hosted Sentry (2026)](https://danubedata.ro/blog/self-host-sentry-glitchtip-error-tracking-2026) · [GitHub Actions pricing changes](https://resources.github.com/actions/2026-pricing-changes-for-github-actions/) · [Rust CI on dedicated hardware](https://corrode.dev/blog/tips-for-faster-ci-builds/) · [Stripe Connect](https://stripe.com/connect)
