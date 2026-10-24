Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# GetLinks accepts more than 10 LinkIds, which the member documents as its maximum

_Filed: 2026-10-24T05:25:51Z_
_Operation: GetLinks | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The LinkIds filter is documented as "One or more link IDs. The maximum is 10." The handler enforces the documented SiteId/Type/Provider mutual-exclusion rule but applies no bound on the LinkIds list length.

Expected: a request carrying 11 or more linkIds values is refused with the modeled ValidationException (HTTP 400) naming the LinkIds member, before any listing is computed.

Actual: the request is accepted and a 200 page is returned for an arbitrarily long id list.

## Evidence

Model documentation for the member, mirrored in the handler's own contract header: src/ops/get_links.rs line 41 — "`LinkIds` (LinkIdList) — One or more link IDs. The maximum is 10." — a documented constraint LinkIdList does not express as @length, while the same handler already enforces another doc-only rule (the SiteId vs Type/Provider exclusion, lines 149-181). Handler: src/ops/get_links.rs lines 183-238 — `input.link_ids.unwrap_or_default()` is used directly as a filter with no length check. tests/all/link_lifecycle.rs exercises at most two linkIds, so nothing asserts the cap.

## Steps to Reproduce

1. POST /global-networks to mint a global network id
2. GET /global-networks/{id}/links?linkIds=link-00000000000000001&...&linkIds=link-00000000000000011 (11 distinct ids)
3. Observe HTTP 200 instead of the documented refusal (ValidationException, HTTP 400, naming LinkIds)
