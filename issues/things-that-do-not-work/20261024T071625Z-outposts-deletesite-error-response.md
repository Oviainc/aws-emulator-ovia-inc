Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# DeleteSite refuses a site that still has Outposts with 400 ValidationException instead of the modelled 409 ConflictException

_Filed: 2026-10-24T07:16:24Z_
_Operation: DeleteSite | Class: error-response_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the site named by `DeleteSite` still has one or more Outposts, the handler answers the modelled `ValidationException` (HTTP 400) with the message "Site {siteId} cannot be deleted because it has one or more outposts." — a deliberate choice documented in the handler's own comment.

That is the wrong error class for a dependent-resource refusal: the request itself is well formed and every parameter is valid; what blocks it is the state of the resource. The model declares `ConflictException` (HTTP 409) on `DeleteSite`, documented "Updating or deleting this resource can cause an inconsistent state", and that exception carries `ResourceId`/`ResourceType` precisely so the service can name the resource the conflict is about. A site has no other state that could ever conflict, so with this handler `DeleteSite`'s `ConflictException` is unreachable.

Expected: HTTP 409 `ConflictException` with `ResourceId` = the site id and `ResourceType` naming the site. Actual: HTTP 400 `ValidationException`, which an SDK/retry policy and a client's conflict branch read as a permanently malformed request rather than as "clear the dependents and try again".

## Evidence

Spec: `DeleteSite` declares `ConflictException` (HTTP 409) with `ResourceId`/`ResourceType` members, plus `ValidationException` (400) for bad parameters; a site's only dependents are its Outposts, so the conflict error exists for exactly this case.

Handler: `src/ops/delete_site.rs` lines ~166-185 — the dependent check builds `DeleteSiteErrorModeled::ValidationException` and the comment states the 400-over-409 choice explicitly.

Tests: `tests/all/error_classification.rs::deleting_a_site_with_dependents_is_refused_as_a_bad_request` (line ~68) pins the 400/ValidationException behaviour, so the current classification is asserted, not accidental — the issue is that the asserted classification is the wrong one.

## Steps to Reproduce

1. CreateSite {"Name":"conflict site"} and keep the SiteId.
2. CreateOutpost {"Name":"blocker","SiteId":<SiteId>,"AvailabilityZone":"us-east-1a"}.
3. DELETE /sites/{SiteId} — observe HTTP 400 with error code ValidationException instead of HTTP 409 ConflictException carrying ResourceId=<SiteId> / ResourceType for the site.
