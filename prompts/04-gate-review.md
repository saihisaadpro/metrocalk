# Prompt 04 — M0 Gate Review: consolidate the three spikes

> Use with `00-orchestrator.md` (v2) as system prompt, AFTER spikes 01–03 have all reported. Effort `max` — this is the highest-judgment session of M0: it converts three reports into the decisions M1 is built on.

---

<task>
Consolidate the results of the three M0 spikes (`/spikes/loro`, `/spikes/flecs`, `/spikes/wasm` — their READMEs, CONSTRAINTS.md, and the progress.md entries) into final gate decisions, reconciled ADRs, and a concrete M1 work breakdown. The intent: M1 must start on settled ground — no "we'll decide later" carried forward.
</task>

<scope>
In scope: reading spike outputs, re-running a spike benchmark ONLY if two reports contradict each other, updating ADRs/docs, writing the M1 breakdown.
Explicitly out of scope: new feature code, new benchmarks beyond contradiction checks, redesigning anything that passed its gate, relitigating ADR-004/005 (business/ops decisions are not this session's business).
</scope>

<review_steps>
1. **Verify each gate against its written criteria** — the numbers in the spike reports vs the success criteria in prompts 01–03. A gate passes on measured numbers, not vibes. Quote the numbers in your output.
2. **Cross-spike reconciliation** — the interactions matter more than the individual results:
   - Did `flecs_ecs` and `loro` compile for wasm32 (spike ③)? A failure triggers the revisit clauses of ADR-001/002 and changes the browser-funnel assumptions of ADR-004 — handle explicitly, don't bury it.
   - Do Loro's mutation-throughput numbers (spike ①) and Flecs' query-under-mutation numbers (spike ②) compose? Sketch the worst-case M1 frame: ECS mutation + Loro commit + cache invalidation + query re-run — is the <16 ms editor budget still plausible end-to-end? Show the arithmetic.
   - Memory: do both libraries' footprints at 5k entities coexist comfortably in a desktop budget?
3. **Settle every ADR status** — 001, 002, 003 (browser leg) each end this session as: confirmed (gate result noted on the status line) or superseded by a new ADR naming the fallback. No "pending" survives.
4. **Update the plan** — if any gate failed, plan §7's M1–M2 rows and §2's stack table change accordingly; mark superseded sections rather than deleting.
5. **Write the M1 work breakdown** — append to progress.md "Next": the ordered list of M1 tasks (monorepo, wrapper API, registry, stress scene, CI), each with its acceptance test, sized so each is one focused session. Flag which M1 task absorbs any spike fallout (e.g., wrapper API design constraints discovered in the flecs ergonomics exhibit).
</review_steps>

<success_criteria>
Done when: every gate has a quoted-numbers verdict · zero ADRs in pending state · the end-to-end frame-budget arithmetic is written down · progress.md "Now" points at the first M1 task and "Next" lists the rest with acceptance tests · architecture.md "Open questions" section contains only questions that genuinely remain open.
</success_criteria>

<verification>
Re-read the three spike READMEs once more AFTER drafting your verdicts, hunting specifically for numbers that contradict your conclusions. Adversarial pass: write one paragraph titled "the strongest case this review is wrong" — if it's persuasive, investigate before finalizing.
</verification>

<definition_of_done>
☐ gate verdicts with quoted numbers · ☐ wasm32 compile results reconciled against ADR-001/002/004 · ☐ frame-budget arithmetic written · ☐ all ADR statuses settled · ☐ plan §2/§7 updated if needed · ☐ M1 breakdown in progress.md with acceptance tests · ☐ adversarial paragraph written · ☐ working tree clean, committed.
</definition_of_done>
