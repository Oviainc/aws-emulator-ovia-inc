Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Refuse reserved aws:-prefixed tag keys with ValidationException

_Filed: 2026-10-24T14:35:27Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` merges whatever tag map the request carries into the resource's tag set, checking only the total tag count against the model's `TagMap` ceiling. Tag keys in the reserved `aws:` namespace are accepted and then reported back by `ListTagsForResource`.

Expected (real AWS): the `aws:` prefix is reserved for AWS-generated tags; a `TagResource` call carrying a key such as `aws:cloudformation:stack-name` or `aws:foo` is refused with `ValidationException` (400) and NOTHING is written. (The same holds for the `tags` members of `CreateLandingZone`/`EnableControl`/`EnableBaseline`, which write straight into the same tag store.)

Actual (emulator): the key is stored verbatim and a subsequent `ListTagsForResource` reports it, so the emulator produces a tag set AWS could not hold — and a client that relies on the rejection (for example, one that round-trips tags copied from another resource's AWS-generated tags) sees a 204 where AWS answers 400.

## Evidence

Smithy model: TagResource (POST /tags/{resourceArn}) declares ValidationException (400) as one of only three errors, and `tags` targets `TagMap` whose key is `TagKey` (len 1..128) — the length bound is all the wire type can express, so the reserved-prefix rule is a handler-side semantic check. That `aws:` is a reserved tag-key prefix an AWS service rejects on a tagging API is well-established, cross-service AWS behavior.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/tag_resource.rs lines 163-195 — after the existence check the handler calls `store.merge_tags(&input.resource_arn, input.tags)` and the only refusal is the per-resource tag-count cap; no key is inspected. src/state.rs lines 1190-1210 (`merge_tags`) extends the stored BTreeMap with the supplied pairs unchanged. The create/enable paths (src/ops/create_landing_zone.rs line 269-271, enable_control.rs 335-337, enable_baseline.rs 245-247) insert their `tags` member into the same store without inspection either.

Tests: tests/all/resource_tags.rs and tests/all/list_tags_for_resource.rs cover the merge semantics, the 404 for an unknown ARN, the percent-encoded `{resourceArn}` label and the tag-count cap; no test asserts that a reserved `aws:`-prefixed key is refused. No TagResource issue exists in the backlog.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{...}} and note the landing zone arn.
2. POST /tags/<percent-encoded arn> with {"tags":{"aws:cloudformation:stack-name":"example"}} — the emulator answers 204; AWS refuses with ValidationException because the `aws:` prefix is reserved.
3. GET /tags/<percent-encoded arn> — the emulator reports the reserved key as a tag of the resource, a state AWS would never produce.
4. Same with the create/enable paths: POST /enable-control ... with {"tags":{"aws:anything":"v"}} is accepted and the tag is reported by ListTagsForResource.
