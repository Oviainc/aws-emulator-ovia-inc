Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Tag-on-create accepts reserved `aws:` tag keys and unbounded tag maps instead of refusing them

_Filed: 2026-10-24T01:04:03Z_
_Operation: CreatePackagingGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `Tags` map supplied to a create is stored verbatim with no validation at all. AWS applies its standard resource-tagging rules to tag-on-create: a tag key using the reserved `aws:` prefix is refused (the caller may not create or modify `aws:`-prefixed tags), at most 50 tags may be attached to a resource, a key is at most 128 characters and a value at most 256 characters. A request violating any of those is rejected — for this service, with the modeled `UnprocessableEntityException` (HTTP 422, "The parameters sent in the request are not valid.") — and NO resource is created.

Expected: `CreatePackagingGroup(Id=g, Tags={"aws:owner": "x"})` is refused with UnprocessableEntityException/422 and a following DescribePackagingGroup on that id answers NotFoundException; likewise for 51+ tags or an over-long key/value.
Actual: the handler checks only that `Id` is non-empty, then calls `state.set_tags(&arn, input.tags.as_ref())`, which collects whatever arrived into the ARN-keyed tag store. The group is created with HTTP 200 and the reserved key is echoed back in `Tags` by the create response and by every later DescribePackagingGroup / ListPackagingGroups / ConfigureLogs / UpdatePackagingGroup projection.

The same unvalidated `set_tags` path is used by `CreateAsset` and `CreatePackagingConfiguration`, so those two creates accept the same invalid tag maps (consolidated here as one tag-validation gap on the create path).

## Evidence

Spec: the model defines `Tags` (`map<__string,__string>`) on CreatePackagingGroupRequest/CreateAssetRequest/CreatePackagingConfigurationRequest, and each of those operations models `UnprocessableEntityException` (errorKind client, httpError 422, "The parameters sent in the request are not valid.") — the error AWS uses for invalid request parameters on this surface. The reserved-`aws:`-prefix rule and the 50-tags / 128-char key / 256-char value limits are AWS-wide resource-tagging semantics (and are the basis of comparable already-filed issues in this backlog, e.g. the iotsecuretunneling and aiops TagResource reserved-prefix issues).
Handler: `src/ops/create_packaging_group.rs` `handle()` (lines ~177-219) validates only `input.id.is_empty()`, duplicate id and the capacity bound before `state.set_tags(&arn, input.tags.as_ref())`; `src/state.rs` `set_tags()` (lines ~225-232) copies every entry with no key/value inspection or count check. Note the sibling mediapackage crate does bound caller-supplied strings (`crate::state::bound_chars`), this crate bounds none.
Tests: `tests/all/success_path_projections.rs` (`create_packaging_group_projects_the_whole_group_representation`, cases "tags only" / "empty tag map") and `tests/all/packaging_lifecycle.rs` assert only that valid tags round-trip; `tests/all/error_responses.rs` covers empty id, unresolvable body ids, duplicate ids, package-block count and forged cursors — no test asserts any tag rejection.

## Steps to Reproduce

1. POST /packaging_groups with body {"id": "vod-group", "tags": {"aws:cloudformation:stack-name": "x"}}: observe HTTP 200 and the reserved key echoed in the response's `tags`, where AWS refuses the request with UnprocessableEntityException (422) and creates nothing.
2. GET /packaging_groups/vod-group: observe the group exists and still reports the reserved `aws:`-prefixed tag.
3. POST /packaging_groups with 51 distinct tag keys, and again with a 200-character tag key and a 300-character tag value: observe HTTP 200 in each case instead of a 422 refusal.
4. Repeat step 1 against POST /assets and POST /packaging_configurations (same `set_tags` path): observe the same unvalidated acceptance.
