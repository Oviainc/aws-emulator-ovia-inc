Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# UpdateOriginEndpoint judges the shared x-amzn-update-if-match precondition last, after its immutable-member checks, where the other three conditional writers judge it first

_Filed: 2026-10-29T01:11:11Z_
_Operation: UpdateOriginEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four operations of this service bind `ETag` to `x-amzn-update-if-match` and resolve the precondition through the one shared rule `crate::state::stale_entity_tag_rejection`. Three of them — `UpdateChannelGroup`, `UpdateChannel` and `CancelHarvestJob` — judge that precondition as the FIRST thing they do once the addressed resource is in hand, before any cross-member or state-dependent rule of their own. `UpdateOriginEndpoint` judges it LAST: the three immutable-member comparisons (`containerType`, `streamNameOutputMode`, `segment.outputTimestampMode`), which are themselves comparisons against the STORED endpoint and therefore in the same class as the precondition, all run before `stale_entity_tag_rejection` is consulted.

So a caller that lost a race AND asks for an immutable change is told two different things depending on which resource it writes: `UpdateChannel` presenting a stale tag together with a member its input type may not carry answers 409 `ConflictException` / `CONFLICTING_OPERATION` ("the resource has been updated by another request between the time you retrieved the ETag and when you made your update request"), while `UpdateOriginEndpoint` presenting a stale tag together with a changed `ContainerType` answers 400 `ValidationException` / `CONTAINER_TYPE_IMMUTABLE` — hiding the fact that the tag the caller holds is no longer current, which is the one thing a conditional writer exists to report. The handler's own comment claims the opposite, that it "judges the precondition and words the refusal exactly as they do".

## Evidence

UpdateOriginEndpoint runs the three stored-value comparisons first and the precondition after — src/ops/update_origin_endpoint.rs:422-464:
```
        if input.container_type != endpoint.container_type {
            return Err(UpdateOriginEndpointErrorModeled::ValidationException(
                immutable_violation(&CONTAINER_TYPE),
...
        // `ETag` is the precondition the caller read from an earlier response: ...
        // which is the ONE rule
        // every conditional writer of this service resolves through
        // (`crate::state::stale_entity_tag_rejection`), so this operation judges the
        // precondition and words the refusal exactly as they do.
        if let Some(conflict) =
            crate::state::stale_entity_tag_rejection(input.e_tag.as_deref(), &endpoint.etag)
        {
            return Err(UpdateOriginEndpointErrorModeled::ConflictException(conflict).into());
        }
```

The three siblings judge it first. src/ops/update_channel.rs:291-331 — precondition, THEN the cross-member rule:
```
        if let Some(conflict) =
            crate::state::stale_entity_tag_rejection(input.e_tag.as_deref(), &stored_etag)
        {
            return Err(UpdateChannelErrorModeled::ConflictException(conflict).into());
        }
...
        if let Some(violation) =
            crate::state::channel_input_type_violation(submitted, &stored_input_type)
        {
            return Err(UpdateChannelErrorModeled::ValidationException(violation).into());
        }
```
src/ops/update_channel_group.rs:212-216 (precondition immediately after the group is resolved, before the write) and src/ops/cancel_harvest_job.rs:220-234 — precondition, THEN the eligibility rule:
```
        if let Some(conflict) =
            crate::state::stale_entity_tag_rejection(input.e_tag.as_deref(), &job.etag)
        {
            return Err(CancelHarvestJobErrorModeled::ConflictException(conflict).into());
        }
        if job.is_terminal() {
            return Err(CancelHarvestJobErrorModeled::ValidationException(
```

The suite that holds the four writers to one answer only ever sends bodies that are otherwise valid, so the combined case is unpinned — tests/all/shared_entity_tag_precondition.rs:12-16:
```
//! The suite drives the rule through all four operations that bind the `ETag` member to
//! `x-amzn-update-if-match` — `UpdateChannelGroup`, `UpdateChannel`,
//! `UpdateOriginEndpoint` and `CancelHarvestJob` — because each resolved the precondition
//! through its own copy of the comparison: a row here holds every one of them to the same
//! answer, whichever resource it writes.
```

## Steps to Reproduce

1. Create a channel group `g`, a channel `c`, and an origin endpoint `e` with {"ContainerType": "TS", "HlsManifests": [{"ManifestName": "index"}]}; keep the ETag the create reported.
2. Commit over it so the kept tag goes stale: PUT /channelGroup/g/channel/c/originEndpoint/e with the same body plus "Description": "moved on" and no If-Match header.
3. PUT /channelGroup/g/channel/c/originEndpoint/e with x-amzn-update-if-match: <the stale tag> and a body whose ContainerType is "CMAF" — observe 400 ValidationException / CONTAINER_TYPE_IMMUTABLE; the stale precondition is never reported.
4. Compare with the channel writer: create a channel, let a second update make its tag stale, then PUT /channelGroup/g/channel/c/ with x-amzn-update-if-match: <the stale tag> and a body carrying MultiviewConfiguration (a member an HLS channel may not carry) — observe 409 ConflictException / CONFLICTING_OPERATION, i.e. the precondition wins there.
