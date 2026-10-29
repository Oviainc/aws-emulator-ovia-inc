Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# TagResource gates the reserved `aws:` tag-key refusal behind the resource lookup, so the same bad request UntagResource answers 400 is answered 404

_Filed: 2026-10-29T01:11:11Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every door in this crate that takes tag keys from a request judges them through the one shared helper `crate::state::reserved_tag_key_rejection`, whose own doc states that it is the REQUEST that is refused and that it is "Called before anything is written". Two of the three doors apply it that way: `UntagResource` runs the check before it touches the store at all, and `CreateHarvestJob` runs it before `state.lock()` under the comment "Refusals the request carries on its own face, before the store is read."

`TagResource` orders the same two steps the other way round: it resolves the ARN's resource first (`store.tags_of`, answering 404 `NotFoundException` when the resource is absent) and only then asks whether the request's keys are in the reserved namespace. The consequence is that one and the same request class — a tagging call naming an `aws:`-prefixed key against an ARN whose resource does not exist — is classified two different ways by two sibling handlers that share the rule: `UntagResource` answers 400 `ValidationException` / `MEMBER_INVALID` (and `CreateHarvestJob` 400 as well), while `TagResource` answers 404 `NotFoundException`, telling the caller to create a resource instead of telling it the tag key is one it may never write. Nothing about the reserved-key rule needs the store — unlike the tag-bounds check on the merged map just below it, which genuinely does — so the lookup is the only reason the refusal classes differ.

## Evidence

The shared rule says the REQUEST is what is refused, before anything is written — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/state.rs:454-468:
```
/// Refuse a request whose tag keys reach into the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace, reporting the modelled rejection.
///
/// `keys` are the keys the REQUEST named — the keys it would create, replace or delete —
/// rather than the keys the resource ends up holding: a stored map can never hold a
/// reserved key (this gate is what keeps it out), and it is the request that is refused.
...
/// Called before anything is written, so a refused request leaves the resource exactly
/// as it was.
```

`TagResource` puts the store lookup (and its 404) FIRST — src/ops/tag_resource.rs:206-221:
```
        let Some(current) = store.tags_of(&region, &target).cloned() else {
            return Err(crate::scaffolded::Scaffolded::unmodeled(
                crate::state::RESOURCE_NOT_FOUND_CODE,
                404,
                crate::state::TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
            ));
        };
        // AWS owns the `aws:` tag-key namespace: a caller may neither create nor replace
        // a tag in it, so the keys the REQUEST names are judged against it before
        // anything is written. ...
        if let Some(rejection) =
            crate::state::reserved_tag_key_rejection("tags", input.tags.keys().map(String::as_str))
```

Its sibling puts the same check BEFORE any lookup — src/ops/untag_resource.rs:184-207:
```
        // AWS owns the `aws:` tag-key namespace: a caller can neither edit nor DELETE a
        // tag in it, so naming one here is refused rather than silently ignored. ...
        if let Some(rejection) = crate::state::reserved_tag_key_rejection(
            "tagKeys",
            input.tag_keys.iter().map(String::as_str),
        ) {
            return Err(UntagResourceErrorModeled::ValidationException(rejection).into());
        }
        let untagged = store.with_tags_mut(&region, &target, at, |tags| {
...
        if !untagged {
            return Err(crate::scaffolded::Scaffolded::unmodeled(
                crate::state::RESOURCE_NOT_FOUND_CODE,
                404,
```

And the crate states the principle explicitly at the third door — src/ops/create_harvest_job.rs:253-268 (the tag checks run before `let mut store = state.lock();` at line 301):
```
        // Refusals the request carries on its own face, before the store is read.
        let tags = input.tags.clone().unwrap_or_default();
...
        if let Some(rejection) =
            crate::state::reserved_tag_key_rejection("Tags", tags.keys().map(String::as_str))
        {
            return Err(CreateHarvestJobErrorModeled::ValidationException(rejection).into());
        }
```

Test coverage leaves the case open: tests/all/reserved_tag_keys.rs:20-26 says the SITE table varies only "where the keys arrive: `TagResource`, `UntagResource` and tag-on-create for a channel group, a channel, an origin endpoint and a harvest job", and every row first creates the resource it then tags (`KEPT` is "the key a resource under test is created with", lines 38-40), so no row sends a reserved key against an absent resource.

## Steps to Reproduce

1. Boot the emulator and create NOTHING (so the channel group named below does not exist).
2. Send DELETE /tags/arn:aws:mediapackagev2:us-east-1:<emulator-account>:channelGroup/absent?tagKeys=aws%3Afoo — observe 400 ValidationException whose message states the reserved-prefix rule (the request is refused on its own face, as UntagResource runs reserved_tag_key_rejection before the lookup).
3. Send POST /tags/arn:aws:mediapackagev2:us-east-1:<emulator-account>:channelGroup/absent with body {"tags":{"aws:foo":"v"}} — observe 404 NotFoundException ("The resource arn you provided does not name a resource that exists") instead of the same 400: TagResource resolved the resource before it looked at the keys.
4. For the third reading of the same rule, send POST /channelGroup/absent/channel/c/originEndpoint/e/harvestJob with a valid body plus "Tags": {"aws:foo": "v"} — observe 400 ValidationException even though the channel group does not exist, because CreateHarvestJob judges the request-only refusals before reading the store.
