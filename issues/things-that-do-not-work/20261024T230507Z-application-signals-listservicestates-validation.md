Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# ListServiceStates silently serves MaxResults=0 (or negative) as a page of 1; its MaxResults has no modelled minimum

_Filed: 2026-10-24T23:05:07Z_
_Operation: ListServiceStates | Class: validation_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServiceStates.MaxResults` is modelled with a maximum only (`range {max: 250}`, `@default 20`) — there is NO minimum — so a request carrying `MaxResults: 0` or a negative value is inside the modelled bound and reaches the handler. The handler, however, states the opposite and reads the bound straight off the input; `page_of` then silently clamps it to 1, on an assumption that is false for exactly this operation.

Result: a caller that asks for at most 0 states gets one state plus a `NextToken`, with no error and no indication the bound was changed. The sibling list in the same crate refuses a non-positive `maxResults` explicitly (`ListServiceLevelObjectives`), and the shared `page_of` helper asserts the case cannot arise at all — so the crate holds three mutually exclusive positions on one rule, and this operation is the one where the unchecked path is live.

No test pins the current behaviour: `tests/all/list_service_states.rs` never sends a MaxResults of 0 or below.

## Evidence

Model (smithy_query query="operation", operation="ListServiceStates"):
  `"MaxResults": {"target": "ListServiceStatesMaxResults", "default": 20, "range": {"max": 250}, "type": "integer"}` — a maximum and no minimum (also `OPERATIONS.md:168`: "`MaxResults` (integer, range ..250, default 20, required)").

The handler's false assumption and the unchecked read:
- `src/ops/list_service_states.rs:212` `// \`MaxResults\` carries the model's own \`@default\` of 20 and its \`range ..250\` is` / `:213` `// refused before this handler runs, so the bound is read straight off the input.` / `:218` `    input.max_results.max(0) as usize,`

The shared helper's assumption, which this operation violates:
- `src/state.rs:987` `// A page of nothing could not advance its own cursor, so a caller following \`NextToken\`` / `:988` `// would loop for ever. Every caller's \`MaxResults\` is modelled with a minimum of 1, so` / `:989` `// this only ever clamps a bound no request can state.` / `:990` `let page_size = page_size.max(1);`

The sibling that refuses the same value instead of clamping it:
- `src/ops/list_service_level_objectives.rs:189` `Some(max_results) if max_results < 1 => {` / `:190`-`:194` returns `Scaffolded::validation(... "Member must have value greater than or equal to 1")`.

## Steps to Reproduce

1. POST /service/states with a body of {"StartTime": now-3600, "EndTime": now+3600, "MaxResults": 0} after creating at least two SLOs that discover services in that window.
2. Observe HTTP 200 with exactly one entry in ServiceStates and a NextToken, rather than a refusal or the bound the caller asked for.
3. Repeat with "MaxResults": -5 — same 200 with one entry, since nothing below the modelled maximum is refused and the handler clamps the value.
