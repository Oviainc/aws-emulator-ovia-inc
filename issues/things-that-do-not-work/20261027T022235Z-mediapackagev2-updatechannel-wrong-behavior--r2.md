Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# UpdateChannel (and UpdateChannelGroup) commit on a write that changed nothing, rotating the ETag the caller is entitled to re-use

_Filed: 2026-10-27T02:22:35Z_
_Operation: UpdateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements the "a write that CHANGED the resource commits" rule in one update handler and skips it in the other two.

`UpdateOriginEndpoint` captures `OriginEndpoint::configuration_fingerprint()` before it writes and commits only if the fingerprint moved, so an update that re-submits exactly the stored configuration leaves `ModifiedAt` and the ETag alone — the state module documents that this is deliberate, so "the tag the caller holds stays usable as the precondition of the update it is entitled to make".

`UpdateChannel` and `UpdateChannelGroup` call `commit(now())` unconditionally on the same code path, with no before/after comparison. So a client that reads a channel, re-sends the same `Description` (or the same body twice) with `x-amzn-update-if-match` set to the tag it just received gets its ETag rotated and `ModifiedAt` advanced by a request that stored nothing new — and its next conditional update with the tag it read one response earlier is refused `409 CONFLICTING_OPERATION`, the exact lost-update false positive the endpoint path was written to avoid. Two identical UpdateChannel requests in a row produce two different ETags for one unchanged channel.

## Evidence

The rule, applied — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/update_origin_endpoint.rs:532-542:
```
        // A write that CHANGED the endpoint commits a new version of it, and the ETag is
        // a function of that version as well as of the endpoint's content: ... An update that stored what was
        // already there changed nothing, so it commits nothing: neither the tag nor
        // `ModifiedAt` moves, and the tag the caller holds stays usable as the
        // precondition of the update it is entitled to make.
        if endpoint.configuration_fingerprint() != before {
            endpoint.commit(crate::state::now());
        }
```
backed by src/state.rs:1002-1005:
```
    /// `UpdateOriginEndpoint` captures it before it writes and compares afterwards, so a
    /// write that changed ANY member commits (and rotates the tag) while one that stored
    /// exactly what was there does not.
```

The same rule skipped by the sibling update handlers — src/ops/update_channel.rs:363-377:
```
        channel.description = input.description;
        channel.input_switch_configuration = ...
        ...
        channel.commit(crate::state::now());
```
and src/ops/update_channel_group.rs:221-222:
```
        group.description = input.description;
        group.commit(crate::state::now());
```
(no fingerprint is taken before the write in either file; `Channel`/`ChannelGroup` have no `configuration_fingerprint` equivalent.)

Test coverage: tests/all/update_origin_endpoint.rs:428-436 pins the endpoint behaviour and states the reason ("a tag that rotated on every write would refuse a conditional update the caller is entitled to make"). The channel-side tables (tests/all/update_channel.rs:109-150 `replace_cases`, and the unconditional-update row at tests/all/update_channel.rs:409-422) only ever submit bodies that DIFFER from what was stored, so no existing test pins rotation on a no-op UpdateChannel/UpdateChannelGroup.

## Steps to Reproduce

1. CreateChannelGroup `g`, then CreateChannel `g`/`c` with `{"Description": "same"}`; note the ETag `E0`.
2. PUT UpdateChannel `g`/`c` with body `{"Description": "same"}` and header `x-amzn-update-if-match: E0`. It answers 200 with a NEW ETag `E1` and a later `ModifiedAt`, although nothing about the channel changed.
3. PUT the identical UpdateChannel body again with `x-amzn-update-if-match: E0` (the tag the caller last read before step 2's response). It is refused 409 `ConflictException` / `CONFLICTING_OPERATION`.
4. Do the same against an origin endpoint (UpdateOriginEndpoint re-submitting the stored configuration): the ETag is unchanged and the previously read tag stays accepted — the two update paths disagree. UpdateChannelGroup behaves like UpdateChannel.
