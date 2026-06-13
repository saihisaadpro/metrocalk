# ADR-005: Self-hosted infrastructure, DevOps by the team

**Date:** 2026-06-12 · **Status:** Accepted · **Supersedes:** the managed-services stack in hosting.md v1.1 (kept as documented exit paths)

## Context

The team works unpaid, values control, and experiences many third-party services as complexity rather than convenience. Phase 1 infrastructure (site, downloads, CI, crash reporting, telemetry) is entirely non-user-facing — failure costs hours, not users. Rust CI on owned hardware is faster and cheaper than hosted runners. The original managed-stack recommendation assumed paid engineering hours; that assumption was wrong for this team.

## Decision

One Hetzner AX42 (~€59/mo) runs everything Phase 1: Caddy (site/docs/downloads), self-hosted GitHub Actions runner, GlitchTip (Sentry-SDK-compatible crash reporting), PostHog hobby (telemetry), Postgres, restic backups to a Storage Box, Uptime Kuma monitoring. Cloudflare free stays in front (DNS, CDN caching of downloads, DDoS, headers) as the one retained third party. Total ~€63–75/mo flat.

**Identity & money are gated, not defaulted:** Stripe (+ Connect) handles all payments/payouts in Phase 2 — never self-hosted. Self-hosted auth is permitted only if the ops-discipline gate passes (3+ months of successful restore tests, monitoring that has caught a real incident, a named owner for security patching); otherwise managed auth.

## Consequences

- Flat, predictable cost; full control; no per-usage surprises; team builds ops muscle before real users arrive.
- Cost moves from invoices to hours: ~half a day/week of ops, more in setup week. If ops time starts eating M0–M6 build time, that is a signal to take an exit path, not push through.
- Backups must be restore-tested monthly — calendar-enforced; the `ops.md` runbook is the gate evidence.
- Single box = single point of failure; acceptable pre-users, add failover (~+€60/mo) when real users depend on uptime.
- GlitchTip over self-hosted Sentry (too heavy) and PostHog hobby (officially unsupported — acceptable for non-user-facing analytics).

## Revisit when

Ops time visibly displaces engine work · the Phase 2 identity/money gate is evaluated · real users depend on uptime (failover decision) · GitHub revives the self-hosted runner surcharge.
