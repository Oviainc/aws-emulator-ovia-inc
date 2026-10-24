Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Tagging operations reject harvest-job ARNs as malformed, so a harvest job's tags can never be read or changed

_Filed: 2026-10-24T05:11:59Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Harvest jobs are a taggable MediaPackage v2 resource: `CreateHarvestJobRequest` accepts `Tags`, `CreateHarvestJobResponse`/`GetHarvestJobResponse` report `Tags`, and the service mints a harvest-job ARN (`…:channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob/{j}`) which it returns in `Arn`. `TagResource` / `UntagResource` / `ListTagsForResource` take that ARN as `ResourceArn` and must operate on the job.

The emulator's shared ARN parser recognises only three resource paths — `channelGroup/{g}`, `channelGroup/{g}/channel/{c}` and `channelGroup/{g}/channel/{c}/originEndpoint/{e}` (`crate::state::TaggedResource` has exactly those three variants). A harvest-job ARN falls into the `_ => None` arm, so all three tagging operations answer HTTP 400 `BadRequestException` / "Invalid input resource arn: bad syntax" for an ARN THIS SERVICE just produced. Consequently tags supplied to `CreateHarvestJob` are stored on the record and reported by `GetHarvestJob`, but are unreachable through the tagging API: they can never be listed, added to, or removed.

Expected: `POST /tags/{harvest-job-arn}` answers 204 and the tags appear on `GetHarvestJob` and on `GET /tags/{harvest-job-arn}`; a harvest-job ARN naming a job that does not exist is the *not-found* class, not the malformed-ARN class.

## Evidence

Spec: Smithy `CreateHarvestJobRequest$Tags` / `CreateHarvestJobResponse$Tags` / `GetHarvestJobResponse$Tags` (TagMap) — harvest jobs carry tags; `HarvestJob$Arn` is `@required`, and `ListTagsForResourceRequest$ResourceArn` targets the unconstrained `TagArn` string with no restriction to a resource subtype.

Handler/state: `src/state.rs:1023-1087` — `enum TaggedResource { ChannelGroup, Channel, OriginEndpoint }` and `parse_tag_arn`, whose match has no `harvestJob` arm and falls through to `None`; `src/state.rs:378-450` (`tags_of` / `with_tags_mut`) likewise have no harvest-job case. `src/ops/tag_resource.rs:186-192`, `src/ops/untag_resource.rs:168-174` and `src/ops/list_tags_for_resource.rs:152-158` all turn that `None` into HTTP 400 `BadRequestException`. `src/state.rs:1007-1021` (`harvest_job_arn`) is the function that mints the very ARN being rejected.

Tests: `tests/all/rejection_rule_and_negative_paths.rs:607-651` enumerates the malformed-ARN rows (not-an-ARN, too few fields, another service, unknown resource type, empty segment, half-way path, empty partition) — a harvest-job ARN is NOT among them, and no test asserts that harvest-job tagging is refused or supported.

## Steps to Reproduce

1. Create a channel group, channel, origin endpoint and a harvest job with `Tags: {"k": "v"}`; note the job's `Arn` from the response.
2. GET `/tags/{url-encoded harvest-job ARN}`. Expected: HTTP 200 with `{"Tags": {"k": "v"}}`. Actual: HTTP 400 `BadRequestException` "Invalid input resource arn: bad syntax".
3. POST `/tags/{url-encoded harvest-job ARN}` with `{"tags": {"k2": "v2"}}`. Expected: HTTP 204 and `GetHarvestJob` then reports both tags. Actual: HTTP 400 BadRequestException.
4. DELETE `/tags/{url-encoded harvest-job ARN}?tagKeys=k`. Expected: HTTP 204 and the tag is gone. Actual: HTTP 400 BadRequestException.
