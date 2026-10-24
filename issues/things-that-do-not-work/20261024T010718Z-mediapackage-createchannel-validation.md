Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateChannel accepts a blank or structurally invalid Id, creating a channel that can never be described, updated or deleted

_Filed: 2026-10-24T01:07:18Z_
_Operation: CreateChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: MediaPackage refuses a `Id` that is not a usable resource identifier — an empty string, whitespace, or one carrying characters outside the identifier alphabet (letters, digits, `_`, `-`) — with the modelled `UnprocessableEntityException` (HTTP 422), and creates nothing. The id is the only handle the resource has: it is the last segment of the resource ARN (`...:channels/<id>`) and the `{Id}` http label of `GET/PUT/DELETE /channels/{Id}`, so the service cannot store one it could not address afterwards.

Actual: `src/ops/create_channel.rs` bounds only the LENGTH of `input.id` (`crate::state::bound_chars("id", &input.id, MAX_STRING_CHARS)`) and then stores the record under that key. `CreateChannel` with `Id: ""` answers 200 and persists a channel whose ARN is `arn:aws:mediapackage:<region>:<account>:channels/` and whose `Id` is the empty string; `Id: "bad id/with slash"` persists a channel whose id cannot be placed in the `{Id}` label. Either record is then unreachable: `DescribeChannel`, `UpdateChannel`, `ConfigureLogs`, `Rotate*Credentials` and `DeleteChannel` all address the channel by that label, so the record is permanently undeletable while `ListChannels` keeps reporting it, and `ListTagsForResource`/`TagResource` on its ARN answer 422 because `crate::state::parse_tag_arn` rejects an ARN with an empty id segment.

The same unguarded `Id` is accepted by `CreateOriginEndpoint` (`/origin_endpoints/{Id}`) and `CreateHarvestJob` (`/harvest_jobs/{Id}`) — the fix should apply the one id rule to all three creates, since all three ids are http labels of their own reads.

## Evidence

Spec grounding: the model documents `CreateChannelRequest$Id` as "The ID of the Channel. The ID must be unique within the region and it cannot be changed after a Channel is created" and binds it as the `{Id}` httpLabel of `DescribeChannel`/`UpdateChannel`/`DeleteChannel` (`smithy_query(query="operation",operation="CreateChannel,DescribeChannel,DeleteChannel")`); the operation declares `UnprocessableEntityException` (httpError 422) as its client-input error. The v1 model publishes no `@pattern` (which is why the framework's constraint walk cannot catch this), but the sibling MediaPackage v2 model pins the same identifier as `^[a-zA-Z0-9_-]+$`, and an empty http label cannot be routed at all — so refusing a blank id is well-established service behaviour rather than a style choice.

Handler: `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_channel.rs` lines 159-205 — the only checks before `state.channels.insert(id, record)` are the character-count bound, the duplicate-id rule and the store cap; nothing rejects an empty or non-identifier id. `src/state.rs` `parse_tag_arn` (~line 1569) already refuses an ARN whose id segment is empty, which shows the record this create stores cannot round-trip through the service's own ARN parser.

Test coverage: `tests/all/channel_lifecycle.rs` exercises only well-formed ids (`"Z"`, `"one_tag_1"`, `"x".repeat(64)`, …) and `tests/all/error_envelope.rs` covers only the over-long description/id length refusals; no test asserts that a blank or slash-bearing id is accepted or refused.

## Steps to Reproduce

1. Boot the emulator and point boto3 at it.
2. Call mp.create_channel(Id="") -> observe HTTP 200 and a body with "Id": "" and "Arn": "arn:aws:mediapackage:<region>:<account>:channels/"; AWS refuses with UnprocessableEntityException / 422.
3. Call mp.list_channels() -> the blank-id channel is listed.
4. Call mp.describe_channel(Id="") and mp.delete_channel(Id="") -> neither reaches the record (the empty {Id} label does not route), so the channel is permanently stuck in the listing.
5. Repeat with Id="bad id/with slash" -> 200 again, and GET /channels/bad%20id%2Fwith%20slash cannot address the stored record.
6. Same with mp.create_origin_endpoint(Id="", ChannelId=<ch>, HlsPackage={}) and mp.create_harvest_job(Id="", ...).
