Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Tag keys in the reserved aws: namespace are accepted and stored instead of refused

_Filed: 2026-10-24T05:09:57Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every Tags member in this model (Stage$tags, IngestConfiguration$tags, TagResourceRequest$tags) documents: "See Best practices and strategies in Tagging AWS Resources and Tag Editor for details, including restrictions that apply to tags and 'Tag naming limits and requirements'; Amazon IVS has no constraints on tags beyond what is documented there." One of those restrictions is AWS-wide and well established: the `aws:` prefix is reserved for AWS use — a user-supplied tag key beginning with `aws:` cannot be created or edited, and the tagging call is refused with a ValidationException (HTTP 400).

Expected: TagResource with a tag key such as `aws:cloudformation:stack-name` is refused with the modeled ValidationException (400) and no tag is written.

Actual: the handler validates only the resourceArn (length/pattern) and the resulting tag COUNT; the keys themselves are never inspected, so the reserved key is stored and ListTagsForResource reports it back. The same unchecked path is taken by the `tags` maps of CreateStage and CreateIngestConfiguration, which persist reserved keys at creation time.

## Evidence

Spec: smithy_query(query="shape", shape="Tags,TagKey") — Tags documents the Tagging-AWS-Resources restrictions as applying in full ("Amazon IVS has no constraints on tags beyond what is documented there"); TagKey carries only @length(1..128), so the reserved-prefix rule is documented-but-unmodeled and must be enforced by the handler, exactly as this service already does for the documented-but-unmodeled 1 KB `attributes` limit.
Handler: services/.../src/ops/tag_resource.rs (handle, lines ~154-185) calls crate::state::check_resource_arn and crate::state::check_tag_count, then `tags.extend(input.tags)` — no inspection of key contents. src/state.rs check_tag_count only compares a count.
Tests: tests/all/tagging.rs rows cover ARN admission/refusal, the 50-tag maximum and the 204 framing; grep for "aws:" in that file matches only ARN literals — no reserved-prefix row exists.

## Steps to Reproduce

1. POST /CreateStage {} → note stage.arn
2. POST /tags/{percent-encoded stage arn} with body {"tags":{"aws:owner":"platform"}}
3. Expected: HTTP 400 ValidationException naming TagResource.tags and the reserved aws: prefix; the resource's tags unchanged
4. Actual: HTTP 204, and GET /tags/{percent-encoded stage arn} reports {"aws:owner":"platform"}
