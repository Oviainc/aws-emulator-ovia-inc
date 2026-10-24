Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# UpdateFeed lets the caller rewrite an output's fromAssociation provenance and accepts duplicate output names

_Filed: 2026-10-24T01:21:05Z_
_Operation: UpdateFeed | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

1. `fromAssociation` IS SERVICE-OWNED, NOT CALLER-OWNED. `UpdateOutput$fromAssociation` is documented: "Elemental Inference originally sets this parameter to True if this output was created by AssociateFeed or to False if this output was created by CreateFeed or UpdateFeed. You must not change this value. Therefore, use GetFeed to determine the current value. Then in the UpdateFeed request, if the current value is True, include this parameter with a value of True. If it's False, omit the parameter." The emulator stores whatever the caller sends (`output.from_association.unwrap_or(false)`), with no comparison against the stored output of the same name. Two divergences follow: (a) a caller may set `fromAssociation: true` on an output that never came from an AssociateFeed call, and `GetFeed` then reports it as association-created — a value the real service reserves to itself; (b) a caller that omits the parameter for an output whose stored value is `true` silently flips it to `false`, destroying the provenance `GetOutput$fromAssociation` exists to report ("You will need this value if you use UpdateFeed"). Expected: the service refuses a request that changes the value (HTTP 400 `ValidationException`, which UpdateFeed models) or keeps the stored value; actual: HTTP 200 with the caller's value persisted.

2. DUPLICATE OUTPUT NAMES. As with CreateFeed, UpdateFeed's replacement list is stored unchecked, so a PUT carrying two outputs with the same `name` persists both — a feed state AssociateFeed explicitly refuses (`ConflictException`, "Output names must be unique within a feed") and which makes an output no longer addressable by name.

## Evidence

Spec: `smithy_query(query="doc",shape="UpdateOutput",member="fromAssociation")` — "Elemental Inference originally sets this parameter ... You must not change this value"; `GetOutput$fromAssociation` is the read-back of that service-set fact; `ValidationException` (400) and `ConflictException` (409) are both modeled for UpdateFeed.
Handler: `src/ops/update_feed.rs` lines ~242-258 — `feed.outputs` is rebuilt from `input.outputs` with `output.from_association.unwrap_or(false)` and no lookup of the stored output of the same name, and no duplicate-name check.
Tests: `tests/all/feed_lifecycle.rs:445-454` pins the current behaviour the other way — it sends `"fromAssociation": true` for an output ("meta1") that the feed never had and that no AssociateFeed created, and asserts the response echoes it — so the caller-writable provenance is asserted rather than refused; nothing asserts the stored value is preserved when the member is omitted, and nothing covers duplicate names on UpdateFeed.

## Steps to Reproduce

1. POST /v1/feed with one output named "crop1" (cropping, ENABLED) → note id; GET /v1/feed/{id} shows crop1 with fromAssociation=false
2. POST /v1/feed/{id}/associate with outputs [{name: "subs1", ...}] → GET /v1/feed/{id} shows subs1 with fromAssociation=true
3. PUT /v1/feed/{id} with outputs [{name: "crop1", ..., "fromAssociation": true}, {name: "subs1", ...}] (true on the CreateFeed-made output, omitted on the association-made one) → observe HTTP 200 and GET /v1/feed/{id} reporting crop1 fromAssociation=true and subs1 fromAssociation=false, i.e. both provenance values changed by the caller
4. PUT /v1/feed/{id} with two outputs that share the name "crop1" → observe HTTP 200 and a stored feed holding two identically named outputs
