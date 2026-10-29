Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 77670ec594ee56220d3fb04f56c8d32fdb5a5e9f

# ListTagsForResource answers 200 for a cancelled signing profile instead of the documented BadRequestException

_Filed: 2026-10-29T02:26:00Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `77670ec594ee56220d3fb04f56c8d32fdb5a5e9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BadRequestException` is the shape the three tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource` — the only operations bound to `/tags/{resourceArn}`, and the only ones in this model that declare it) use, and the model states a second trigger for it besides a bad ARN/tag: calling a tagging API on a cancelled signing profile.

The two WRITE handlers implement that trigger: both gate on `SigningProfileRecord::accepts_tag_writes()` and refuse a `Canceled` profile with a 400 `BadRequestException` whose `code` names the status. `ListTagsForResource` — the third tagging API, served from the same route and declaring the same error shape — performs no status check at all: it resolves the profile and answers HTTP 200 with the tag map.

The crate narrows the model's sentence from "a tagging API" to "a tagging WRITE" inside the helper's own doc comment (the helper is named `accepts_tag_writes` and documented as "Whether this profile's tags may still be CHANGED"), but the sentence it quotes to justify the gate says nothing about changing: it says the exception occurs when you CALL a tagging API on a cancelled profile. The only AWS sentence offered for the read staying open is `CancelSigningProfile`'s, and that one names a different operation (`ListSigningProfiles`), not the tagging route.

So one of the three operations that share a route and an error set implements the documented refusal in two places and skips it in the third.

## Evidence

Model (the documented failure mode), `smithy_query(query="doc", shape="BadRequestException")`:
  "The request contains invalid parameters for the ARN or tags. This exception also occurs when you call a tagging API on a cancelled signing profile."

The only AWS sentence the crate offers for the read staying open names a DIFFERENT operation, `smithy_query(query="doc", shape="CancelSigningProfile")`:
  "Changes the state of an ACTIVE signing profile to CANCELED. A canceled profile is still viewable with the ListSigningProfiles operation, but it cannot perform new signing jobs."

`ListTagsForResource` is a tagging API: `smithy_query(query="doc", shape="ListTagsForResource")` — "Returns a list of the tags associated with a signing profile resource." — bound to `GET /tags/{resourceArn}` and declaring `BadRequestException (HTTP 400)` / `NotFoundException (HTTP 404)`, the error pair only the three tagging operations declare (`services/service-com-amazonaws-signer-wallabyservice/OPERATIONS.md:104`, `:153`, `:160`).

Two siblings apply the gate, the third does not — same crate:

- `src/ops/tag_resource.rs:223-232`:
  "        // A canceled profile is the one status the modelled bad request names, and
        // the gate reads the status the store holds NOW ...
        if !profile.accepts_tag_writes() {
            let message = crate::state::tag_write_refused_message(&profile.profile_name, &profile.status);
            let code = crate::state::profile_status_error_code(&profile.status);
            return Err(bad_request(message, code));
        }"
- `src/ops/untag_resource.rs:221-229` — the identical gate.
- `src/ops/list_tags_for_resource.rs:193-205` — no gate at all:
  "        let store = state.lock();
        let profile = store
            .profile_for_tagging(&region, &input.resource_arn)
            .map_err(|problem| refused(problem, &input.resource_arn))?;

        Ok(ListTagsForResourceOutput {
            ... tags: Some(profile.reported_tag_map()),
        })"

The narrowing from "a tagging API" to writes is asserted only in the helper's own doc, `src/state.rs:249-258`:
  "/// Whether this profile's tags may still be CHANGED.
   /// `BadRequestException` documents the one status that forbids it: the shape
   /// \"also occurs when you call a tagging API on a cancelled signing profile\" ...
   pub fn accepts_tag_writes(&self) -> bool { self.status != PROFILE_STATUS_CANCELED }"

Test coverage: `tests/all/resource_tags.rs:680-764` (`a profile's STORED status decides which tagging calls it admits`) pins the write refusals (`ProfileCanceled` + `BadRequestException`, lines 765-779) and asserts `list_tags(...).status == 200` for the cancelled row (lines 747-752); its stated reason (`tests/all/resource_tags.rs:22-25`) rests on "Signer documents a canceled profile as still viewable", which is the `ListSigningProfiles` sentence quoted above, not a statement about the tagging route.

## Steps to Reproduce

1. Boot the emulator and create a profile: `PUT /signing-profiles/Canceled_Read` with body `{"platformId":"AWSLambda-SHA384-ECDSA"}`; note the returned `arn`.
2. Tag it: `POST /tags/{urlencoded arn}` with body `{"tags":{"env":"prod"}}` — 200.
3. Cancel it: `DELETE /signing-profiles/Canceled_Read` — 204.
4. Call the write half of the tagging route again: `POST /tags/{urlencoded arn}` with `{"tags":{"env":"dev"}}` — observe HTTP 400 `BadRequestException`, code `ProfileCanceled` (the documented trigger, implemented).
5. Call the read half: `GET /tags/{urlencoded arn}` — observe HTTP 200 with `{"tags":{"env":"prod"}}` instead of the same documented 400 `BadRequestException` for a tagging API called on a cancelled signing profile.
