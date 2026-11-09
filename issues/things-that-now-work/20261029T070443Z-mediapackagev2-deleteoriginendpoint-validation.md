Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3a384b4a12755a7fa8fedde9fe2fff5778558e98

# DeleteOriginEndpoint deletes an endpoint that still holds harvest jobs, silently destroying them instead of the children-attached refusal both sibling deletes make

_Filed: 2026-10-29T07:04:43Z_
_Operation: DeleteOriginEndpoint | Class: validation_

> _Note: this issue was identified against the code base at commit `3a384b4a12755a7fa8fedde9fe2fff5778558e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteOriginEndpoint` removes the endpoint record unconditionally. Harvest jobs are persisted ON the endpoint (`OriginEndpoint::harvest_jobs`), so removing the endpoint destroys every harvest job it holds — each of which is an independently addressable resource in this crate's own containment hierarchy, with its own ARN, ETag, status and tag map. No refusal is produced, and the caller is never told that anything but the endpoint disappeared: a later `GetHarvestJob` or `ListTagsForResource` on the job's ARN answers a not-found for the ORIGIN ENDPOINT, as if the job had never existed.

This contradicts the rule this crate states for itself and applies at every other level of the same hierarchy:

* `crate::state`'s module doc states deleting a parent that still has children is refused, and `ResourceKind`'s doc names the harvest job as the fourth level of that hierarchy (channel group -> channel -> origin endpoint -> harvest job);
* `DeleteChannelGroup` refuses while the group holds channels and `DeleteChannel` refuses while the channel holds origin endpoints, both through the crate's one shared builder `children_attached(parent, child)`, which is generic over `ResourceKind` and already supports `(OriginEndpoint, HarvestJob)` (`ResourceKind::HarvestJob` exists, with its own noun/plural);
* the model declares `ConflictException` (HTTP 409) for `DeleteOriginEndpoint`, and this handler can never produce it — it is the one delete of the three that has no path to the 409 its two siblings answer for exactly this case.

Note the contrast inside the state module itself: the attached policy's field doc explicitly says the endpoint's delete takes the policy with it ("deleting the endpoint takes its policy with it"), because a policy is a sub-resource. The `harvest_jobs` field carries no such statement — it is a child collection, not a sub-resource — yet the delete treats it as one.

## Evidence

The rule the crate states for itself — `services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:5-7`:
```
//! MediaPackage v2 is a three-level containment hierarchy — channel group -> channel
//! -> origin endpoint — so the store nests the same way: a channel cannot exist
//! without its group, and deleting a parent that still has children is refused.
```
and `state.rs:2437-2438`, which puts the harvest job in that hierarchy:
```
/// One kind of resource in mediapackagev2's containment hierarchy: channel group ->
/// channel -> origin endpoint -> harvest job.
```
The shared refusal builder, `state.rs:2506-2511`:
```
/// The 409 a delete answers while the resource it names still holds children.
...
pub fn children_attached(parent: ResourceKind, child: ResourceKind) -> ConflictException {
```
Both siblings call it. `src/ops/delete_channel_group.rs:169-180`:
```
// Deleting a group that still has channels would orphan them (and everything
// beneath them), so the service refuses while any child is left. ...
if !group.channels.is_empty() {
    return Err(DeleteChannelGroupErrorModeled::ConflictException(
        crate::state::children_attached(
            crate::state::ResourceKind::ChannelGroup,
            crate::state::ResourceKind::Channel,
```
`src/ops/delete_channel.rs:176-186`:
```
// Deleting a channel that still has origin endpoints would orphan them, so the
// service refuses while any child is left — the same refusal, one level down.
if !channel.origin_endpoints.is_empty() {
    return Err(DeleteChannelErrorModeled::ConflictException(
        crate::state::children_attached(
            crate::state::ResourceKind::Channel,
            crate::state::ResourceKind::OriginEndpoint,
```
The handler that skips it, `src/ops/delete_origin_endpoint.rs:166-176` — the ENTIRE body after the store lock:
```
// `@idempotent`: the operation's promise is that the endpoint is gone when it
// returns, so an absent endpoint — or an absent parent — is already that state
// and succeeds without an error.
if let Some(channel) = store
    .channel_group_mut(&region, &input.channel_group_name)
    .and_then(|group| group.channels.get_mut(&input.channel_name))
{
    channel
        .origin_endpoints
        .remove(&input.origin_endpoint_name);
}
```
What that `remove` takes with it, `state.rs:1209-1210`:
```
/// Every harvest job created against this endpoint, keyed by job name.
pub harvest_jobs: BTreeMap<String, HarvestJobRecord>,
```
contrasted with the sub-resource beside it, `state.rs:1203-1208`:
```
/// `Put*` replaces it wholesale, `Get*` reports it and `Delete*` detaches it — so it
/// is persisted ON the endpoint: deleting the endpoint takes its policy with it, ...
pub policy: Option<EndpointPolicy>,
```
The modelled error that becomes unreachable (smithy_query operation=DeleteOriginEndpoint): errors include `{"shape": "ConflictException", "errorKind": "client", "httpError": 409}`.

Test coverage: `tests/all/resource_tags.rs:567-583` deletes an origin endpoint that holds NO harvest job (`create_hierarchy("doomed", Level::Endpoint, ...)`) and asserts 200, so no existing test pins a 200 for the endpoint-with-jobs case; `tests/all/delete_channel_multiview_source.rs` and `tests/all/error_bodies.rs:408-462` pin the `RESOURCE_IN_USE` shape the two sibling deletes answer.

## Steps to Reproduce

1. CreateChannelGroup `grp`, CreateChannel `chn` (default HLS input type), CreateOriginEndpoint `ep` with one HLS manifest named `index`.
2. CreateHarvestJob against grp/chn/ep with `harvestJobName=job1` and a scheduleConfiguration whose endTime is well in the future; note the 200 and the job's ARN.
3. `DELETE /channelGroup/grp/channel/chn/originEndpoint/ep` — observe 200 and an empty body; nothing reports that `job1` was destroyed with it.
4. `GET /channelGroup/grp/channel/chn/originEndpoint/ep/harvestJob/job1` — observe 404 with `ResourceTypeNotFound: ORIGIN_ENDPOINT`: the job record, its tags, its ETag and its status are gone.
5. `GET /tags/<the job ARN from step 2>` — observe 404; the job can no longer be read or untagged.
6. For the contrast one level up: with an endpoint present, `DELETE /channelGroup/grp/channel/chn/` answers 409 `ConflictException` / `RESOURCE_IN_USE` from `children_attached`, and the channel is left stored.

## Resolution

The report's central claim does not hold: the Evidence section above says "the model declares
`ConflictException` (HTTP 409) for `DeleteOriginEndpoint`", and it does not.
`smithy_query(query="operation", operation="DeleteOriginEndpoint")` lists exactly
`AccessDeniedException` (403), `InternalServerException` (500), `ThrottlingException` (429) and
`ValidationException` (400) for that operation, while the same query for `DeleteChannel` and
`DeleteChannelGroup` lists `ConflictException` (409) on both — which is why those two refuse while
a child is left and this one does not. The operation's own documentation states the containment
rule one level UP ("You must delete all endpoints from a channel before you can delete the
channel") and states no obstacle below the endpoint.

The rule, as the model states it: a delete answers the children-attached 409 only at the levels
whose model declares `ConflictException` for that delete; a delete whose modelled error set
declares none is unconditional and idempotent, and removes the resource together with the records
persisted ON it — here the endpoint's policy and its harvest jobs, neither of which is addressable
except under the endpoint's own path (every harvest-job operation binds a URI below it, and the
job's ARN spells the same containment), so neither can outlive it.

The emulator's behaviour was therefore already correct and is unchanged. What changed is the
documentation that had stated the rule more widely than the model does — the `state` module doc
and the `OriginEndpoint::harvest_jobs` field doc, plus the handler's header and the comment at the
removal — and a new suite, `tests/all/delete_origin_endpoint_cascade.rs`, that pins the rule: a
table of eight rows spanning what the endpoint holds when the delete arrives (none, one, two,
three and five harvest jobs; caller-named and service-named; a cancelled job beside live ones; a
job whose scheduled window is already open; with and without an attached policy), each asserting
the modelled 200, the idempotent repeat, the 404s for the endpoint, its policy and each job (plus
each job's minted ARN through the tagging front end), that the group's `ListHarvestJobs` then
reports none of them, that the channel and group survive, and that an endpoint created again under
that name carries no policy and no job; a sibling-endpoint case showing the removal is scoped to
the endpoint named; and a per-level case driving all three deletes with a child present (409
naming "channels", 409 naming "origin endpoints", 200), then showing both refused deletes succeed
once the endpoint is gone.
