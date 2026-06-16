# Decisions (ADRs)

One file per architectural decision: `NNN-short-name.md` with Date/Status · Context · Decision ·
Consequences · Revisit-when. ADRs are immutable once accepted — supersede, don't rewrite (status-line
updates recording a gate result are allowed).

| # | Decision | Status |
|---|---|---|
| [001](001-flecs-over-bevy-ecs.md) | Flecs (via `flecs_ecs`) over bevy_ecs, behind our own query API | Accepted — M0 GO |
| [002](002-loro-over-custom-wal.md) | Loro CRDT over a custom WAL/ARIES undo system | Accepted — M0 GO |
| [003](003-desktop-first-tauri-exit-gate.md) | Desktop-first, Tauri 2 shell, with the M2 exit gate | Accepted — render leg + IPC passed |
| [004](004-free-engine-token-economy.md) | Free engine + token economy (generation/marketplace/AI) | Accepted (Phase 2) |
| [005](005-self-hosted-ops.md) | Self-hosted ops | Accepted (Phase 2) |
| [006](006-browser-query-backend.md) | Browser query backend = pure-Rust over the Loro projection | Accepted |
| [007](007-m2.1-tauri-gate-result.md) | M2.1 gate: IPC PASS; single-window compositing flagged (later disproven) | Accepted |
| [008](008-shell-composition.md) | Shell composition = single-window (transparent WebView2 over wgpu, OS-composited) | Accepted — confirmed on 2 GPUs |
| [009](009-transport-protocol-loro-framing.md) | Transport = Loro-Protocol-v1 framing + opaque Loro-update payload | Accepted |
| [010](010-editor-projection-architecture.md) | Editor UI = projection of the core (Zustand · JSON Forms · React Flow) | Accepted |
| [011](011-intent-ranking.md) | Intent ranking for binding-by-intent (proximity · affinity · recency) | Accepted |
| [012](012-describe-to-create-resolver.md) | Describe-to-create resolver: tiered local→marketplace→generate; token-overlap local match | Accepted — M3.2 local tier |
| [013](013-live-persistence-replay-log.md) | Live editor persistence = deterministic-seed + replay-log (not Loro-merge-on-start) | Accepted — shipped M2 |
