---
name: idea
description: Run one idea end to end as a single flow — wayfinder map, research, grilling, specs, tickets — pausing only at human gates and carrying a handoff packet between stages so nothing is re-derived. Prototype of the future factory flow command; keep its stages 1:1 with that command's states.
disable-model-invocation: true
---

# Idea

A loose idea arrives; landed tickets come out. This skill chains the existing
skills into **one flow with human gates**, so the user approves direction at
fixed points and never does plumbing between them.

The issue tracker and triage label vocabulary should have been provided to you. If not, tell the user to run `/setup-matt-pocock-skills`.

## The handoff packet

Every stage writes forward. The packet lives in the map body under
`## Handoff packet` — one short block, updated at each gate — and every spec
quotes it (a "Sequenced from …" line), so tickets inherit the *why* through
their parent spec link without anyone re-reading closed tickets.

```markdown
## Handoff packet

- Destination: <one line>
- Locked decisions: <order, posture, identity — added by grilling>
- Scope in: <what this flow produces>
- Scope out: <explicit non-goals>
- Terms: <glossary terms crystallized so far>
- Fog: <still-unspecifiable items, graduating as the flow advances>
```

Keep it under fifteen lines. Detail lives in the linked issues, never in the packet.

## The flow

Run the stages in order. **Gates pause for the human; everything else is
mechanical and needs no permission.** Never resolve more than one
non-research decision ticket per session; research tickets may run in
parallel (subagents where available, inline otherwise).

1. **Chart.** Grill for the destination (human answers, never the agent),
   then create the `wayfinder:map` issue with Destination / Notes / Handoff
   packet / Decisions-so-far / Fog, plus the first `wayfinder:research` and
   `wayfinder:grilling` child tickets. Wire sub-issue links; block the
   grilling ticket on the research. Gate: destination lock.
2. **Research.** Resolve each research ticket (parallel allowed): post the
   findings as a resolution comment, close the ticket, append one gist line
   to the map's Decisions-so-far. No gate — resolving *is* the work.
3. **Grill.** Work the grilling ticket live with the human (order, posture,
   identity, scope). Record the resolution, close the ticket, move decided
   fog into the packet's Locked decisions, crystallize terms into
   `GLOSSARY.md`. Gate: order/scope lock.
4. **Spec, per spec in the locked order.** For each spec: check the seams
   with the human, then synthesize with to-spec (no interview beyond the
   seams check) and publish labelled `spec` + `ready-for-agent`. Do NOT
   freeze — freezing is the human's separate act; hand the spec over
   freeze-ready. After each spec's tickets land (next stage), pause and ask
   whether to continue to the next spec. Gate: seams check.
5. **Tickets, per spec.** Quiz the human on the slice breakdown
   (granularity, edges, merge/split), then publish approved tickets as
   sub-issues of the spec with native blocking edges, `Depends on` lines,
   `ticket` on all and `ready` only on unblocked ones. Do NOT close or
   modify the parent spec. Gate: breakdown approval.
6. **Close out.** Index all specs on the map, graduate fog that became
   tickets, leave the rest. The map stays open while specs are unfrozen or
   tickets are unticketed; it is done when the route is walked.

## Rules carried forward

- Refer to maps and tickets by name, never bare numbers.
- Map/ticket labels: `wayfinder:map`, `wayfinder:research`,
  `wayfinder:grilling`, `wayfinder:task` — never triage labels on these.
- Specs: `spec` (+ `ready-for-agent`); frozen bodies are untouchable —
  review findings go in comments, and any edit needs a re-freeze.
- Tickets: `ready-for-agent` + `ticket` always, `ready` only when the issue
  may run immediately; staged work is `ticket`-without-`ready` plus a
  `Depends on` line.
- A frozen spec or a closed decision is a record: quote it, never rewrite it.

## Future binary mapping (factory flow)

This skill is the prototype. When the flow becomes a command, these stages
MUST stay 1:1 with its states, gates become its prompt points, and the
packet becomes its `status --json` payload:

`intake → charting → researching → grilling → speccing → awaiting-freeze(human) → slicing → ticketed`

The dispatcher owns everything after `ticketed`. If a stage is added or
removed on either side, update the other the same day.
