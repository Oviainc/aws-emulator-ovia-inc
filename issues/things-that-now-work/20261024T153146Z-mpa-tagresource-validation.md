Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# TagResource stores tag keys in the reserved aws: namespace instead of rejecting them

_Filed: 2026-10-24T15:31:46Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS reserves the `aws:` tag-key prefix for AWS-managed tags; a TagResource request whose Tags map carries a key beginning with `aws:` (any case) is refused with ValidationException (HTTP 400) and no tag is written. This is uniform AWS tagging behavior and TagResource declares ValidationException for exactly this class of refusal.

Actual: the handler validates only the ResourceArn form, the account boundary and the per-resource tag count (MAX_TAGS_PER_RESOURCE = 50, reported as TooManyTagsException). It then upserts every supplied key verbatim into the resource's tag map, so `aws:cloudformation:stack-name` or `AWS:foo` is accepted with HTTP 200 and is afterwards reported by ListTagsForResource as if the caller owned it. The same unvalidated write path is also reachable through CreateApprovalTeam$Tags and CreateIdentitySource$Tags, and the reserved key can then only be removed with UntagResource — i.e. the emulator lets a client build state real MPA refuses to create.

## Evidence

Spec/well-established behavior: the `aws:` tag-key prefix is reserved across AWS tagging APIs and a request using it is rejected with ValidationException; the model declares ValidationException (400) for TagResource and bounds TagKey at length 1..128 with no other grammar, so the reserved-prefix rule is a service rule the handler must apply (the crate already applies other unmodelled service rules itself — see the is_well_formed_arn ARN check in the same handler). Handler: services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/tag_resource.rs — after the count check it runs `for (key, value) in &input.tags { tags.insert(key.clone(), value.clone()); }` with no key inspection; state.rs offers only merged_tag_count and no tag-key validator. Tests: tests/all/resource_tags.rs covers round-trip, the 50-tag ceiling and the not-found wording; grep for `aws:` tag keys in tests/ returns no hits, so nothing asserts this.

## Steps to Reproduce

1. Seed a tagged resource: record the caller's account as the organization management account, then POST /approval-teams to create a team and note its Arn.
2. PUT /tags/{Arn} with body {"Tags": {"aws:owner": "someone"}}.
3. Observe HTTP 200 and an empty body; real AWS answers 400 ValidationException for a key in the reserved aws: namespace.
4. GET /tags/{Arn} and observe the reserved key reported back as one of the resource's tags.
