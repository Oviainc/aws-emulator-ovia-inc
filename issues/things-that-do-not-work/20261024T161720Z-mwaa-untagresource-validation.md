Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# UntagResource deletes tag keys with the reserved aws: prefix instead of refusing the request

_Filed: 2026-10-24T16:17:20Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved for AWS internal use, and AWS's tagging rule is explicit that such a tag can neither be created NOR deleted by a caller: "Do not use the aws: prefix in your tag names or values because it is reserved for AWS use. You can't edit or delete tag names or values with this prefix." A tagging API therefore refuses an `UntagResource` whose `tagKeys` names an `aws:`-prefixed key with HTTP 400 `ValidationException`, and removes nothing.

Expected: `UntagResource(ResourceArn=<env arn>, tagKeys=['aws:cloudformation:stack-name'])` is refused with HTTP 400 `ValidationException` (the key is reserved) and the environment's tag map is left untouched.

Actual: the handler resolves the ARN and then calls `stored.tags.remove(key)` for every key it was given, with no inspection of the key at all, and answers HTTP 200. Any reserved tag an MWAA-internal process had placed on the environment would be silently deleted, and a client that relies on `ValidationException` to detect that it tried to modify a reserved tag sees a success instead. `TagKey`'s modeled pattern permits the colon, so the boundary validator cannot refuse this and the rule has to live in the handler. The already-open TagResource validation issue (20261024T153328Z) covers only the CREATE direction (TagResource plus CreateEnvironment's `Tags` map); nothing covers the delete direction, which is a different handler and a different operation.

## Evidence

Spec grounding: `UntagResource` declares `ValidationException` (@httpError 400) for bad input (smithy_query query="operation" operation="UntagResource"), and `tagKeys` is `TagKeyList` whose keys permit `:` so the constraint validator passes `aws:`-prefixed keys. AWS's service-wide resource-tagging rule reserves the `aws:` prefix and states a caller cannot edit or delete tags carrying it — the same grounding the accepted TagResource reserved-prefix issue uses for the create direction.
Handler: services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/untag_resource.rs:154-163 — ARN lookup, then `for key in &input.tag_keys { stored.tags.remove(key); }` and `Ok(UntagResourceOutput {})`. No key inspection of any kind; the comment above the loop treats every key as freely removable because the operation is `@idempotent`.
Tests: tests/all/environment_lifecycle.rs (the tagging round-trip, which builds `tagKeys=` query strings around line 170) removes only ordinary keys and asserts the removal succeeds; tests/all/rejection_contract.rs:326 exercises `/tags/{arn}?tagKeys=k` only for the ARN-pattern and not-found refusals. No test pins a refusal for a reserved key, so nothing currently asserts the permissive behavior.

## Steps to Reproduce

1. Boot the emulator and create environment 'TagEnv'; note its Arn.
2. Seed a reserved key the way an AWS-internal process would — call TagResource (POST /tags/<url-encoded arn>) with {"Tags": {"aws:internal": "v"}} (that create is itself the subject of the open TagResource issue; alternatively assert step 4 on a reserved key that is simply absent).
3. Call UntagResource: DELETE /tags/<url-encoded arn>?tagKeys=aws:internal
4. Expected: HTTP 400 ValidationException stating that tag keys beginning with 'aws:' are reserved and cannot be deleted, with the stored tag map unchanged.
5. Observed: HTTP 200 with an empty body, and ListTagsForResource(ResourceArn=<arn>) no longer reports the reserved key — the emulator deleted a tag AWS refuses to let a caller delete.
