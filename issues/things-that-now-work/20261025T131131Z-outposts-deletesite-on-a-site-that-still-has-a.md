Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: outposts.delete-site-conflict-with-linked-outpost-rejected
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts.delete-site-conflict-with-linked-outpost-rejected|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: fafe0142a789ac7a37106edfeaee5b3c191a47ea

# DeleteSite on a Site that still has a linked Outpost is refused with ConflictException (409) instead of ValidationException (400)

> _Note: this issue was identified against the code base at commit `fafe0142a789ac7a37106edfeaee5b3c191a47ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSite

## Description
One cause.

Cause 1 of 1: DeleteSite's dependent-Outpost refusal uses the wrong error shape, HTTP status and message.
Failing check: outposts.delete-site-conflict-with-linked-outpost-rejected

AWS: `DeleteSite` (Smithy: `services/service-com-amazonaws-outposts-outpostsolafservice/smithy-model/com-amazonaws-outposts-outpostsolafservice.json`, `com.amazonaws.outposts#DeleteSite`, DELETE /sites/{SiteId}) models both `ConflictException` (httpError 409, "Updating or deleting this resource can cause an inconsistent state") and `ValidationException` (httpError 400, "A parameter is not valid"). The model does not say which one covers a site that still has a linked Outpost, so this is recorded behavior only: the recording shows AWS refusing that DeleteSite with error code `ValidationException` (HTTP 400) and message `Site <siteId> cannot be deleted because it has one or more outposts.` In that message the fixed text is `Site ` + ` cannot be deleted because it has one or more outposts.`; the only variable part is `<siteId>`, taken from the request's `SiteId` member (the bare `os-...` id the request resolved to, not a literal from any one run). AWS's refusal names no dependent Outpost id and sets no `ResourceId`/`ResourceType`. The surrounding recorded contract that this check also asserts and the emulator does satisfy: the Site survives the refusal (independent GetSite still reads it), a Site with no linked Outpost deletes immediately, and the identical DeleteSite succeeds once the only linked Outpost is deleted — so the divergence is purely in how the refusal is classified, not in the state transition.

Emulator: answers error code `ConflictException` with HTTP 409 for the same call, with `ResourceId` set to the blocking Outpost's id, `ResourceType` = outpost, and a message built by `crate::state::site_dependents_conflict(&site_id, outpost_id)` rather than AWS's fixed text. The handler's own doc comment states this choice deliberately ("the refusal is the modelled `ConflictException` (409), carrying the `ResourceId` / `ResourceType`"), so the fix is a behavioral decision to reverse, not an oversight. Replay output: `AssertionError: expected ValidationException, got ConflictException`.

Why: a client that distinguishes AWS's error classes gets this wrong in both directions. Code that catches `ValidationException` to report "clear the Outposts off this site first" never fires, and the call surfaces as an unhandled error; conversely retry/backoff layers that treat 409 `ConflictException` as a transient concurrent-modification conflict will retry a request that can never succeed until the caller deletes the Outpost, looping instead of reporting. The 400-vs-409 split also changes which generic HTTP error branch a client takes before it ever reads the error code.

Fix: the DeleteSite handler's dependent-check branch in `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/delete_site.rs` — the branch must construct the modeled `ValidationException` (400) with AWS's message template instead of `ConflictException`, and the message helper in `crate::state` must emit the fixed text above with only the site id interpolated (no dependent-Outpost id). Note this is NOT covered by the open issue `20261024T071624Z-outposts-deleteoutpost-error-response.md` ("DeleteOutpost never answers ConflictException"): that is a different operation and the opposite defect (a missing refusal), while this one is a refusal that fires correctly under the wrong error class.

Traces: outposts.delete-site-conflict-with-linked-outpost-rejected; handler branch at `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/delete_site.rs:182-197`.

## Evidence
Check: outposts.delete-site-conflict-with-linked-outpost-rejected (group: overly-permissive; axis:rejection, plane:control).

Expected (recorded AWS): DeleteSite(SiteId=<site with one linked Outpost>) -> HTTP 400, Error.Code = "ValidationException", Error.Message = "Site <siteId> cannot be deleted because it has one or more outposts." (fixed text "Site " / " cannot be deleted because it has one or more outposts."; variable part = the request's SiteId).

Actual (emulator): HTTP 409, Error.Code = "ConflictException", message from crate::state::site_dependents_conflict(site_id, outpost_id), plus ResourceId = blocking OutpostId and ResourceType = "outpost" — fields AWS does not populate for this refusal.

Replay failure:
  File "check_outposts_delete_site_conflict_with_linked_outpost_rejected.py", line 42, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}"
  AssertionError: expected ValidationException, got ConflictException

Everything else the check asserts passes: the baseline Site with no linked Outpost deletes immediately; GetSite on the deleted baseline returns NotFoundException with message "siteId=<id>"; the Site survives the rejected delete; DeleteOutpost then GetOutpost returns NotFoundException "outpostId=<id>"; the repeated DeleteSite then succeeds and GetSite returns NotFoundException. So only the refusal's error class, status and message diverge.

Source confirming the deliberate choice: services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/delete_site.rs lines 170-197 (and the module doc at lines 3-8: "A site an Outpost still belongs to is refused with `ConflictException` (409)").

## Steps to Reproduce
1. Clone the repo and boot the emulator; point a botocore `outposts` client at it.
2. site_id = create_site(Name=<unique>)["Site"]["SiteId"]
3. az = ec2.describe_availability_zones(Filters=[{"Name":"state","Values":["available"]}])["AvailabilityZones"][0]["ZoneName"]
4. outpost_id = create_outpost(Name=<unique>, SiteId=site_id, AvailabilityZone=az)["Outpost"]["OutpostId"]
5. Call delete_site(SiteId=site_id) and inspect the raised ClientError.
   Observed: Error.Code == "ConflictException", HTTP status 409.
   Expected: Error.Code == "ValidationException", HTTP status 400, Error.Message == f"Site {site_id} cannot be deleted because it has one or more outposts."
6. Confirm the state half is already correct: get_site(SiteId=site_id) still returns the Site; delete_outpost(OutpostId=outpost_id) then delete_site(SiteId=site_id) succeeds and get_site raises NotFoundException with message f"siteId={site_id}".

Or replay directly: run_behavioral_suite and observe check outposts.delete-site-conflict-with-linked-outpost-rejected.
