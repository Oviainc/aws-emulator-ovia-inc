Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 0831f46a299ca00fdb6555dd0d1d0166298aa2f3

# UntagResource accepts tag keys in the reserved aws: namespace

_Filed: 2026-10-24T16:32:16Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `0831f46a299ca00fdb6555dd0d1d0166298aa2f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS use on every taggable AWS resource: a user request may neither create nor REMOVE such a key, and the attempt is refused — for Amazon EVS with the tag-policy refusal the operation models (`TagPolicyException`, "The request doesn't comply with IAM tag policy", HTTP 400, deprecated in favour of ValidationException).

Expected: UntagResource(resourceArn=<environment>, tagKeys=["aws:cloudformation:stack-name"]) is refused with HTTP 400 and NO tag key of the resource is removed — including the non-reserved keys the same request named, since the request is refused as a whole.

Actual: the handler loops over `input.tag_keys` and removes whatever it finds, with no inspection of the keys, so a request carrying a reserved key is accepted with HTTP 200; the modelled `TagPolicyException` is unreachable on this operation for every input. The modelled `TagKey` pattern (`^[\w.:/=+-@]+$`) admits a colon, so the boundary validator does not catch it either.

## Evidence

Spec: UntagResource declares `TagPolicyException` (HTTP 400, "The request doesn't comply with IAM tag policy. Correct your request and then retry it.") and `ResourceNotFoundException` only; the reserved `aws:` prefix is AWS-wide, established tagging behaviour (keys beginning with `aws:` are service-managed and cannot be created or deleted by a caller). Handler: src/ops/untag_resource.rs lines 174-180 — `for key in &input.tag_keys { removed |= environment.tags.remove(key).is_some(); }`, no key inspection anywhere before it, and no code path in the crate constructs `TagPolicyException`. Tests: tests/all/tagging_not_found.rs covers only the not-found ARN path; tests/all/environment_lifecycle.rs lines 1190-1200 assert UntagResource removes the named keys and is idempotent, with ordinary keys — grep for "aws:" in tests/ finds only ARN literals, so no test asserts the reserved-prefix refusal. The counterpart gap on the write side is filed separately as "Tag keys in the reserved aws: namespace are stored instead of refused" (TagResource/validation); that issue does not cover the removal path, which answers 200 even though the reserved key can never legitimately be removed by a caller.

## Steps to Reproduce

1. Create an environment and build its ARN (arn:aws:evs:<region>:<account>:environment/<environmentId>).
2. Call TagResource with tags={"team":"platform"} so the resource carries a normal tag.
3. Call UntagResource with tagKeys=["aws:cloudformation:stack-name", "team"].
4. Observe HTTP 200 instead of a 400 TagPolicyException/ValidationException, and ListTagsForResource showing that the non-reserved `team` key was removed by a request real EVS would have refused in full.
