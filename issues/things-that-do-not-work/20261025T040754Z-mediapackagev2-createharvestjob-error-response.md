Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# CreateHarvestJob's ValidationException messages bypass the crate's one refusal form, and the unpublished-manifest refusal echoes the caller's manifest name

_Filed: 2026-10-25T04:07:54Z_
_Operation: CreateHarvestJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate centralises how a refusal is worded: `crate::state::validation_message` is documented as the single place EVERY `ValidationException` the service raises is built, so the whole surface answers one form — `Validation error encountered at request.<member>: <rule>` — and the already-fixed message-rule work established that a refusal's text carries none of the caller's own values.

CreateHarvestJob's three handler-side refusals are built inline instead, with no member path and no prefix:

* `END_TIME_EARLIER_THAN_START_TIME` -> `"ScheduleConfiguration.EndTime is earlier than ScheduleConfiguration.StartTime"`
* `INVALID_HARVEST_JOB_DURATION` -> `"ScheduleConfiguration describes a zero-length harvest window"`
* `HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT` -> `format!("HarvestedManifests names {missing}, which this origin endpoint does not publish")`

The third one is the worse of the two defects: `missing` is a `ManifestName` taken straight out of the caller's own request (`unpublished_manifest` returns the requested name), so this message interpolates a caller-supplied identifier into a refusal body — exactly the rule the crate's refusal suite states the service never breaks, and exactly the rule a previous fix removed from the not-found/conflict builders. The first two merely diverge from the form.

The status (400) and the `ValidationExceptionType` discriminators are correct and are what the existing tests assert; only the `Message` is produced by the wrong rule, so no test catches it. The same inline construction appears at `cancel_harvest_job.rs:219-234` (`HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION`), so a fix should route all four through `crate::state::validation_message` / `crate::state::validation_failed` with the member path the request named (e.g. `scheduleConfiguration.endTime`, `harvestedManifests`) and a request-independent detail.

This is distinct from the open trace issue on CreateHarvestJob (missing S3-destination validation and `ValidationExceptionType`/`x-amzn-errortype` serialization): that one is about a refusal that is never raised and about members that are never serialized, this one is about the text of refusals that ARE raised.

## Evidence

The rule, in this crate's own state module — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:2260-2268:
```
/// How mediapackagev2 states an input failure: the fixed prefix, the member path the
/// failure is at (in lowerCamel, as the wire names it), then what is wrong with it.
///
/// Every `ValidationException` this service raises goes through here, so the whole
/// surface reports a refusal in one form — `Validation error encountered at
/// request.<member>: <rule>` — rather than one form at the request boundary and another
/// from the handlers. `member_path` is `None` for a rule that is about the request as a
/// whole rather than about one member.
pub fn validation_message(member_path: Option<&str>, detail: &str) -> String {
```

The handler that breaks it — src/ops/create_harvest_job.rs:243-271:
```
        if end < start {
            return Err(CreateHarvestJobErrorModeled::ValidationException(
                ValidationException {
                    message: Some(
                        "ScheduleConfiguration.EndTime is earlier than \
                         ScheduleConfiguration.StartTime"
                            .to_string(),
...
                        "ScheduleConfiguration describes a zero-length harvest window"
```
and src/ops/create_harvest_job.rs:302-316 — the caller's value in the body:
```
        if let Some(missing) = unpublished_manifest(&input.harvested_manifests, endpoint) {
            return Err(CreateHarvestJobErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "HarvestedManifests names {missing}, which this origin endpoint does \
                         not publish"
                    )),
```
with `missing` coming from the request: src/ops/create_harvest_job.rs:404-413 `/// The first manifest name in `requested` that `endpoint` does not publish` ... `fn unpublished_manifest(requested: &HarvestedManifests, ...) -> Option<String>`.

How every other refusal in the crate is built — src/ops/put_channel_policy.rs:191-199:
```
        if !crate::state::is_policy_document(&input.policy) {
            return Err(PutChannelPolicyErrorModeled::ValidationException(
                crate::state::validation_failed(
                    "policy",
                    INVALID_POLICY_REQUIREMENT,
                    "INVALID_POLICY",
                ),
```
and src/ops/update_origin_endpoint.rs:272-279 `fn immutable_violation` -> `crate::state::validation_message(Some(member.member), member.detail)`.

The request-independence rule the crate pins elsewhere — tests/all/refusal_text_and_entity_tags.rs:7-18:
```
//! 1. A REFUSAL'S TEXT IS REQUEST-INDEPENDENT. Whatever a caller asked for, a refusal
//!    carries the fixed sentence for the kind of resource and the rejection class, and
//!    none of the caller's own values ...
//!    (The one request-derived message form this service has is the constraint violation,
//!    `Validation error encountered at request.<member>: <rule>`, ... it is built from the member and the bound,
//!    not from the caller's identifiers.)
```
and tests/all/error_bodies.rs:14-17: "The sentence depends only on WHAT was refused ... never on the identifiers the caller sent. So no refusal message contains a name from the request".

Test coverage: tests/all/rejection_rule_and_negative_paths.rs:1215-1245 exercises all three CreateHarvestJob refusals but asserts only `ValidationExceptionType` (`answer.string("ValidationExceptionType")`), never the `Message` — so the divergence is untested and would stay untested after a fix unless the message is asserted.

## Steps to Reproduce

1. Create a channel group, channel and origin endpoint with `HlsManifests: [{"ManifestName": "index"}]`.
2. POST `/channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob` with `HarvestedManifests.HlsManifests = [{"ManifestName": "not-published"}]` and a valid schedule window.
3. Observe the 400 body: `Message` is `"HarvestedManifests names not-published, which this origin endpoint does not publish"` — it both omits the `Validation error encountered at request.<member>: ` prefix every other refusal in this crate carries and echoes the caller's manifest name.
4. Repeat with `ScheduleConfiguration.EndTime` earlier than (then equal to) `StartTime` and observe the same missing prefix on the `END_TIME_EARLIER_THAN_START_TIME` / `INVALID_HARVEST_JOB_DURATION` bodies.
5. Compare with any policy or immutable-member refusal (e.g. PutChannelPolicy with `Policy: "not-json"`), which answers `Validation error encountered at request.policy: ...`.
