Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cf1145e7beae3fcf58bc3c47bccc54d7fcf374b0

# TagResource judges only reserved tag keys before resolving the ARN, so an invalid tag against an unknown resource answers 404 instead of 400

_Filed: 2026-10-26T07:27:15Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `cf1145e7beae3fcf58bc3c47bccc54d7fcf374b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` runs only the reserved-namespace half of the tag judgement on the input (`validate_tag_keys_unreserved`) before the store is consulted. The rest of the tag judgement — the key/value length bounds and (through `merge_tags`) the per-resource cap — is only reached AFTER the ARN has been resolved, so a request carrying a tag key of 0 or >128 characters, or a value longer than 256 characters, is answered with the store's not-found failure (HTTP 404 `NotFoundException`) whenever the ARN names no live record, and with the modelled `ValidationException` (HTTP 400) only when it does. The same logical bad request therefore gets two different answers depending on store contents.

This contradicts (a) the documented contract of the helper that performs the judgement, which states that the judgement is on the INPUT alone and is run by an operation "ahead of resolving anything", and (b) the sibling handler `PutDeploymentParameter`, which does run the full `validate_tags_member` on its `tags` before any state is touched precisely so that "the refusal does not depend on whether this call would create the parameter or update one that exists" — a rule its own test suite pins for the unresolved-target case.

Fix direction: run the same input-only judgement `PutDeploymentParameter` runs (with `TagConstraints::Unconstrained`) before the lock/lookup in `TagResource`, leaving `merge_tags` to add only the result-level cap.

## Evidence

Helper doc, `src/state.rs:774-778`:
"Because it judges only the input, a request carrying a tag the member does not admit is refused the SAME way whichever path the operation would then take, and whether or not the tags would have been applied at all: an operation runs this over its `tags` ahead of resolving anything, and nothing is stored either way. [`merge_tags`] runs it too, so no merge can store an entry this would refuse."

Sibling handler, `src/ops/put_deployment_parameter.rs:240-255`:
"// The TAGS next, still before any state is touched: the modelled `tags` member is judged on the INPUT, so the refusal does not depend on whether this call would create the parameter (and apply `tags`) or update one that exists ... A key in the namespace AWS reserves ..., a key or value outside its length bound, and a key outside the character class this member's own `TagsMap` states are all bad requests, answered here rather than at the merge below" — followed by `if let Some(tags) = input.tags.as_ref() { if let Err(why) = crate::state::validate_tags_member(tags, crate::state::TagConstraints::TagsMap) { return Err(...ValidationException(why).into()); } }`.

This handler, `src/ops/tag_resource.rs:175-181` runs ONLY the reserved-key half before the store:
`if let Some(tags) = input.tags.as_ref() { if let Err(why) = crate::state::validate_tag_keys_unreserved(tags.keys().map(String::as_str), "tags") { ... } }`
then `src/ops/tag_resource.rs:199-204`:
`let Some(record) = store.parameters.get_mut(&input.resource_arn) else { ... return Err(crate::state::parameter_not_found()); };`
and only afterwards, `src/ops/tag_resource.rs:214-223`, `crate::state::merge_tags(&record.tags, &input.tags.unwrap_or_default(), crate::state::TagConstraints::Unconstrained)` — which is where `validate_tags_member` (the length bounds) finally runs (`src/state.rs:837`).

Test coverage today pins only the two halves separately, never their combination: `tests/all/not_found_error_code.rs:261-280` sends the invalid tag (`{"tags": {"": "v"}}`) against a STORED ARN ("A stored ARN with a tag key outside the tagging limits ... is still the modelled `ValidationException`"), while `tests/all/not_found_error_code.rs:91-108` sends only VALID tags (`json!({ "tags": { "owner": "integration-test" } })`) against the unknown ARNs. The sibling rule IS pinned for PutDeploymentParameter: `tests/all/put_deployment_parameter_target.rs:492-514` ("a tag key in the namespace AWS reserves, target not held" must be 400 and "must not read as an unresolved target").

## Steps to Reproduce

1. Start the emulator, provision a target and PutDeploymentParameter once so a live ARN exists (as `tests/all/not_found_error_code.rs::emulator_with_one_parameter` does).
2. Call TagResource POST /tags/<live-arn> with body {"tags": {"": "v"}} (a tag key outside the 1..128 bound) — observe HTTP 400 ValidationException.
3. Call TagResource POST /tags/<same ARN with the region or account changed, i.e. a well-formed aws-marketplace ARN that names no record> with the identical body {"tags": {"": "v"}} — observe HTTP 404 with code NotFoundException instead of the 400 the input alone earns.
4. Compare with PutDeploymentParameter: send a reserved/invalid tag together with an unresolvable target and observe 400 (pinned by tests/all/put_deployment_parameter_target.rs:503-519), showing the two handlers order the same input judgement differently.
