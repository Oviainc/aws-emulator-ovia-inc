Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Tag keys in the reserved aws: namespace are stored instead of refused

_Filed: 2026-10-24T01:22:38Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS use across every taggable AWS service: a TagResource that carries a key beginning with `aws:` (any case) is refused — for Amazon EVS with the tag-policy refusal the model declares (`TagPolicyException`, "The request doesn't comply with IAM tag policy", deprecated in favour of `ValidationException`) — and no tag is written.

Expected: TagResource(resourceArn=<environment>, tags={"aws:cost": "x"}) is refused with HTTP 400 (ValidationException, or the deprecated TagPolicyException) and the environment's tags are unchanged.

Actual: the handler merges whatever the request carries into `environment.tags` after checking only the 200-tag count bound, so the reserved key is accepted with HTTP 200 and ListTagsForResource reports it afterwards — the modelled tag-policy refusal is unreachable. The modelled `@length`/`@pattern` on TagKey (1..128, `^[\w.:/=+-@]+$`) admit `aws:`-prefixed keys, so the boundary validator does not catch it either. CreateEnvironment's own `tags` member takes the same unchecked path.

## Evidence

Spec: TagResource models `TagPolicyException` (HTTP 400, "The request doesn't comply with IAM tag policy. Correct your request and then retry it." — "See ValidationException instead") and `TagKey` is `^[\w.:/=+-@]+$`, which allows a colon, so the reserved-prefix rule is a service rule rather than a shape rule; the `aws:` prefix reservation is AWS-wide, established tagging behaviour. Handler: src/ops/tag_resource.rs lines 193-215 — `merged.extend(input.tags)` with only the `MAX_TAGS_PER_RESOURCE` count check before `environment.tags = merged`; no key inspection. src/ops/create_environment.rs line 408 stores `input.tags` the same way. Tests: tests/all/tagging_not_found.rs only covers the not-found ARN paths and a successful tag/read-back; grep for "aws:" in tests/ finds only ARN literals, so no test asserts the reserved-prefix refusal.

## Steps to Reproduce

1. CreateEnvironment and build its ARN (arn:aws:evs:<region>:<account>:environment/<environmentId>).
2. Call TagResource with tags={"aws:owner": "team"}.
3. Observe HTTP 200 instead of a 400 tag-policy/validation refusal.
4. Call ListTagsForResource — the reserved `aws:owner` key is reported as a customer tag.
