Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# GetSites accepts more than 10 SiteIds, which the member documents as its maximum

_Filed: 2026-10-24T05:25:51Z_
_Operation: GetSites | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The SiteIds filter is documented as "One or more site IDs. The maximum is 10." The handler applies no bound on the list length.

Expected: a request carrying 11 or more siteIds values is refused with the modeled ValidationException (HTTP 400) naming the SiteIds member, before any listing is computed.

Actual: the request is accepted and a 200 page is returned for an arbitrarily long id list.

## Evidence

Smithy model documentation for the member (smithy_query query=doc shape=GetSitesRequest member=SiteIds): "One or more site IDs. The maximum is 10." — a documented constraint SiteIdList does not express as @length. Handler: src/ops/get_sites.rs lines 140-175 — `input.site_ids.unwrap_or_default()` is used directly as a filter with no length check. No test in tests/all/ asserts the cap (tests/all/global_network_lifecycle.rs only exercises 0-2 ids).

## Steps to Reproduce

1. POST /global-networks to mint a global network id
2. GET /global-networks/{id}/sites?siteIds=site-00000000000000001&...&siteIds=site-00000000000000011 (11 distinct ids)
3. Observe HTTP 200 instead of the documented refusal (ValidationException, HTTP 400, naming SiteIds)
