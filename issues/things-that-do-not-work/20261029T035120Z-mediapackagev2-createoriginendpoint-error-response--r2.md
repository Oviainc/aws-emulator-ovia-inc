Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 50acf5c50341569d447ddf04c6c32ab03f586e64

# CreateOriginEndpoint judges its tag refusals only after resolving the channel group and channel, so a request CreateHarvestJob answers 400 is answered 404

_Filed: 2026-10-29T03:51:20Z_
_Operation: CreateOriginEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `50acf5c50341569d447ddf04c6c32ab03f586e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateOriginEndpoint applies the crate's two tag rules — `tag_bounds_rejection` and `reserved_tag_key_rejection` — only AFTER it has resolved the parent channel group and channel in the store. Both rules are about the request alone (they read nothing but `input.tags`), and this very handler states that rule for its other request-only checks and runs those BEFORE any lookup. The sibling create in the same crate, CreateHarvestJob, runs the identical two checks before it even takes the store lock.

Consequence: one request carrying BOTH a reserved `aws:`-prefixed tag key (or a tag map past the bounds) AND a channel-group/channel name that does not exist is answered 404 `ResourceNotFoundException` by CreateOriginEndpoint, while the same pair of faults is answered 400 `ValidationException` by CreateHarvestJob. Two creates in one service report the same malformed request as two different error classes, and the caller is told to create a resource rather than to fix the tag key it may never use.

## Evidence

Both sides are in this crate.

This handler's own rule, applied to its other request-only checks — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_origin_endpoint.rs:301-306: "// The manifest-set rules are about the request alone, so they are decided before\n// the store is consulted and before anything is written" (the check itself at :307, and `speke_role_arn_rejection` at :334).

But the lookups come next — create_origin_endpoint.rs:338: `let Some(group) = store.channel_group_mut(&region, &input.channel_group_name) else { ... ResourceNotFoundException(crate::state::ResourceKind::ChannelGroup.not_found()) ... }` and :346 `let Some(channel) = group.channels.get_mut(&input.channel_name) else { ... ResourceKind::Channel.not_found() ... }` — and only THEN the tag rules: :357 `if let Some(rejection) = crate::state::tag_bounds_rejection("Tags", &tags)` and :364 `if let Some(rejection) = crate::state::reserved_tag_key_rejection("Tags", tags.keys().map(String::as_str))`.

The sibling create orders them the other way — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_harvest_job.rs:269: "// Refusals the request carries on its own face, before the store is read." with `tag_bounds_rejection("Tags", &tags)` at :273 and `reserved_tag_key_rejection("Tags", ...)` at :280, while the store is not even locked until :317 (`let mut store = state.lock();`) and the group is resolved at :318.

The two rules themselves are shared helpers whose docs state they are request-level: state.rs:462 "`keys` are the keys the REQUEST named ... it is the request that is refused".

Test coverage: tests/all/reserved_tag_keys.rs and tests/all/resource_tagging.rs exercise the reserved-key refusal; no test pins the ORDER of the refusal against a missing parent for this operation.

## Steps to Reproduce

1. Start the emulator with the mediapackagev2 service mounted.
2. POST /channelGroup/no-such-group/channel/no-such-channel/originEndpoint with body {"OriginEndpointName":"e1","ContainerType":"CMAF","HlsManifests":[{"ManifestName":"index"}],"Tags":{"aws:owner":"x"}}.
3. Observe 404 ResourceNotFoundException (the channel group is reported missing; the reserved tag key is never judged).
4. POST /channelGroup/no-such-group/channel/no-such-channel/originEndpoint/no-such-endpoint/harvestJob with a body carrying the same reserved tag key {"HarvestedManifests":{...},"ScheduleConfiguration":{...},"Destination":{...},"Tags":{"aws:owner":"x"}}.
5. Observe 400 ValidationException / MEMBER_INVALID for the same request-level fault — the two creates of one service disagree on the class.
