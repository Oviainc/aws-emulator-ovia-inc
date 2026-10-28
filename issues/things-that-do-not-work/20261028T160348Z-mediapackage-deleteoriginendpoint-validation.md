Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a24b8bb497f295cf2fd83b42be71d5dbb9740df2

# An origin endpoint is deleted while stored harvest jobs still name it, breaking the cross-store consistency rule DeleteChannel enforces

_Filed: 2026-10-28T16:03:48Z_
_Operation: DeleteOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `a24b8bb497f295cf2fd83b42be71d5dbb9740df2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states and enforces a cross-store consistency rule in one direction and skips it in the other.

DeleteChannel refuses to delete a channel that still has origin endpoints, and its comment states the reason: "This is also what keeps the two stores consistent — no stored endpoint ever names a channel this service no longer holds, which `CreateOriginEndpoint` already refuses." The helper it reads, `origin_endpoints_of`, repeats the same rule.

The harvest-job store holds exactly the same kind of reference, and CreateHarvestJob refuses it exactly the same way: an `originEndpointId` naming no stored endpoint of this Region is refused 422 ("A HarvestJob harvests ONE origin endpoint: an `OriginEndpointId` naming no stored endpoint OF THIS REGION has nothing to harvest"), and the job's `channel_id` is copied out of the STORED endpoint at submission time. But DeleteOriginEndpoint removes the endpoint unconditionally — it reads nothing about harvest jobs — so after one DELETE the store holds harvest jobs whose `originEndpointId` resolves to nothing, which is precisely the state CreateHarvestJob refuses to create and DeleteChannel exists to prevent.

Consequences a caller sees: DescribeHarvestJob/ListHarvestJobs answer 200 reporting an `originEndpointId` (and a `channelId`) that both answer 404 on their own routes; a job still `IN_PROGRESS` keeps its clock-owed transition and settles against an endpoint that no longer exists. And because DeleteChannel is unblocked once the endpoints are gone, the channel can then be deleted too, leaving harvest jobs naming a channel this service no longer holds — the very thing DeleteChannel's comment claims its refusal prevents.

## Evidence

The rule, stated and enforced for channel -> endpoint, `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/delete_channel.rs:154-170`:

        // An OriginEndpoint packages exactly one Channel, so a channel that still has
        // endpoints cannot be deleted: its endpoints are removed first. This is also what
        // keeps the two stores consistent — no stored endpoint ever names a channel this
        // service no longer holds, which `CreateOriginEndpoint` already refuses.
        ...
        let endpoints = state.origin_endpoints_of(&region, &input.id);
        if endpoints > 0 {
            return Err(crate::state::has_attached_children(
                crate::state::CHANNEL_KIND,
                crate::state::ORIGIN_ENDPOINT_SHAPE,
                endpoints,
            )
            .into());
        }

and the helper's own doc, `src/state.rs:2249-2253`:

    /// How many stored origin endpoints OF `region` package `channel_id`.
    /// `DeleteChannel` reads it: a channel that still has endpoints cannot be deleted,
    /// which is what keeps the two stores consistent (no endpoint ever names a channel
    /// this service no longer holds).

The same reference, refused on the way in, `src/ops/create_harvest_job.rs:287-295`:

        // A HarvestJob harvests ONE origin endpoint: an `OriginEndpointId` naming no
        // stored endpoint OF THIS REGION has nothing to harvest, so the job is not
        // created. ...
        let Some(endpoint) = state.origin_endpoint(&region, &input.origin_endpoint_id) else {
            return Err(crate::state::member_not_found("originEndpointId").into());
        };

and the stored job keeps both ids, `src/state.rs:1971-1974`:

        /// The channel the harvested endpoint packages, read from the STORED endpoint at
        /// submission time rather than from the request, which carries no channel id.
        pub channel_id: String,
        pub origin_endpoint_id: String,

The handler that skips the rule, `src/ops/delete_origin_endpoint.rs:149-154` — the whole body after the lock:

        let mut state = state.lock();

        if state.remove_origin_endpoint(&region, &input.id).is_none() {
            return Err(crate::state::origin_endpoint_not_found(&input.id).into());
        }
        Ok(DeleteOriginEndpointOutput {})

No dependency read, although the store is in hand and the refusal builder (`crate::state::has_attached_children`, `src/state.rs:320`) already exists.

Test coverage: nothing pins the current behaviour. `tests/all/channel_lifecycle.rs:1010-1031` exercises the channel-has-endpoints refusal and then the endpoint deletions; `tests/all/harvest_jobs.rs` never deletes an origin endpoint (a crate-wide grep for DELETE in tests/all shows no `/origin_endpoints/{id}` delete in any harvest-job suite), so no test states a reason for leaving the harvest-job reference dangling.

## Steps to Reproduce

1. CreateChannel with Id="harvest-channel".
2. CreateOriginEndpoint with Id="harvest-endpoint", ChannelId="harvest-channel", hlsPackage={}, startoverWindowSeconds=86400 (so the harvest precondition holds).
3. CreateHarvestJob with Id="job-1", OriginEndpointId="harvest-endpoint", a valid startTime/endTime window and an assumable s3Destination.roleArn — observe 200.
4. DELETE /origin_endpoints/harvest-endpoint — observe 202, the endpoint is gone, and no refusal although a stored harvest job still names it (contrast DeleteChannel, which refuses while children exist to keep exactly this kind of reference resolvable).
5. GET /harvest_jobs/job-1 — observe 200 with originEndpointId="harvest-endpoint" and channelId="harvest-channel", while GET /origin_endpoints/harvest-endpoint answers 404.
6. DELETE /channels/harvest-channel — observe 202 (no endpoints left to block it), leaving job-1 naming a channel the service no longer holds.
