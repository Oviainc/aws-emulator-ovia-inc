Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 57e940d225b76eac54b5261180d73f867edef087

# CreateChannel judges its tag refusals only after resolving the channel group, so a request CreateHarvestJob answers 400 is answered 404

_Filed: 2026-10-29T02:19:48Z_
_Operation: CreateChannel | Class: error-response_

> _Note: this issue was identified against the code base at commit `57e940d225b76eac54b5261180d73f867edef087`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create in this crate carries the same two request-only tag refusals — `crate::state::tag_bounds_rejection` and `crate::state::reserved_tag_key_rejection` — and the crate's stated rule is that a refusal the request carries on its own face is decided BEFORE the store is read, so a caller that sent a bad request is told what is wrong with the request.

`CreateHarvestJob` implements that rule: both tag checks run before `state.lock()`. `CreateChannel` (and `CreateOriginEndpoint`) do the opposite: they resolve the parent chain first and answer `ResourceNotFoundException` (404), so an identical bad tag map is reported as "no such channel group" instead of the modelled `ValidationException` (400) / `MEMBER_INVALID` that the crate's own refusal builder produces for it. The two sibling creates therefore answer the SAME malformed request with two different classes depending on which operation it was sent to.

This is the same precedence defect already filed in this crate for `TagResource` ("gates the reserved `aws:` tag-key refusal behind the resource lookup, so the same bad request UntagResource answers 400 is answered 404"); `CreateChannel` and `CreateOriginEndpoint` are the two remaining sites of it. Consolidating both symptoms (the reserved-namespace refusal and the tag-bounds refusal) here, since they are one class on one operation.

## Evidence

`CreateChannel`'s own comment states the rule its tag checks then break — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_channel.rs:354-358:
```
        // The cross-member input rules first: they are about the request alone, so they
        // are decided before the store is consulted and before anything is written.
        if let Some(violation) = input_type_violation(&input) {
            return Err(CreateChannelErrorModeled::ValidationException(violation).into());
        }
```
but the parent lookup is answered first — src/ops/create_channel.rs:374-379:
```
        let Some(group) = store.channel_group_mut(&region, &input.channel_group_name) else {
            return Err(CreateChannelErrorModeled::ResourceNotFoundException(
                crate::state::ResourceKind::ChannelGroup.not_found(),
            )
            .into());
        };
```
and only afterwards the tag rules, which need nothing from the store — src/ops/create_channel.rs:394-408:
```
        let tags = input.tags.clone().unwrap_or_default();
        // A create's tag map is the same unbounded-growth site `TagResource` is, so it is
        // judged against the same bounds before anything is stored.
        if let Some(rejection) = crate::state::tag_bounds_rejection("tags", &tags) {
            return Err(CreateChannelErrorModeled::ValidationException(rejection).into());
        }
        ...
        if let Some(rejection) =
            crate::state::reserved_tag_key_rejection("tags", tags.keys().map(String::as_str))
        {
            return Err(CreateChannelErrorModeled::ValidationException(rejection).into());
        }
```

The sibling create states and implements the opposite order — src/ops/create_harvest_job.rs:253-268 and :301-307:
```
        // Refusals the request carries on its own face, before the store is read.
        let tags = input.tags.clone().unwrap_or_default();
        if let Some(rejection) = crate::state::tag_bounds_rejection("Tags", &tags) {
            return Err(CreateHarvestJobErrorModeled::ValidationException(rejection).into());
        }
        ...
        if let Some(rejection) =
            crate::state::reserved_tag_key_rejection("Tags", tags.keys().map(String::as_str))
        {
            return Err(CreateHarvestJobErrorModeled::ValidationException(rejection).into());
        }
```
```
        let mut store = state.lock();
        let Some(group) = store.channel_group_mut(&region, &input.channel_group_name) else {
            return Err(CreateHarvestJobErrorModeled::ResourceNotFoundException(
                crate::state::ResourceKind::ChannelGroup.not_found(),
            )
            .into());
        };
```

The shared builder says the refusal is about the REQUEST, not about the store — src/state.rs:457-459:
```
/// `keys` are the keys the REQUEST named — the keys it would create, replace or delete —
/// rather than the keys the resource ends up holding: a stored map can never hold a
/// reserved key (this gate is what keeps it out), and it is the request that is refused.
```

The third site, with the same inversion — src/ops/create_origin_endpoint.rs:338-368: the group lookup (338) and the channel lookup (346) precede `tag_bounds_rejection` (357) and `reserved_tag_key_rejection` (364).

Test coverage: tests/all/reserved_tag_keys.rs spans the reserved-key rule over `TagResource`, `UntagResource` and tag-on-create for all four resource kinds (header, lines 14-26), but every create row first builds its parent chain (`KEPT` resource at line 40), so no test pins today's 404-before-400 order for a create against a missing parent.

## Steps to Reproduce

1. Boot the emulator. Do NOT create a channel group called `ghost`.
2. POST /channelGroup/ghost/channel with body {"ChannelName": "c", "tags": {"aws:cloudformation:stack-name": "x"}}. Observe 404 `ResourceNotFoundException` with ResourceTypeNotFound CHANNEL_GROUP — the tag key the crate owns a refusal for is never judged.
3. POST /channelGroup/ghost/channel/c/originEndpoint/e/harvestJob with the same `aws:`-prefixed key under Tags (group `ghost` still absent). Observe 400 `ValidationException` / MEMBER_INVALID: the sibling create judges the request's own face first.
4. Repeat step 2 with a 300-character tag value instead of the reserved key: again 404 instead of the 400 `tag_bounds_rejection` produces.
5. Same contrast inside the tagging plane for comparison: DELETE /tags/<arn of a resource that does not exist>?tagKeys=aws:x answers 400, while POST /tags/<same arn> with {"tags":{"aws:x":"y"}} answers 404.
