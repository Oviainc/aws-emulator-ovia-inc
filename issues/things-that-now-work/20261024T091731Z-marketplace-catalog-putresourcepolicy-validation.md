Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 671706971a680016eadecf6b3438021fa34d8a56

# Any parseable JSON is accepted as a resource policy

_Filed: 2026-10-24T09:17:31Z_
_Operation: PutResourcePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `671706971a680016eadecf6b3438021fa34d8a56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutResourcePolicyRequest$Policy` is "the policy document to set; formatted in JSON" — a resource-based policy document (an object with `Version`/`Statement`, each statement carrying `Effect`, `Principal`, `Action`, `Resource`). `src/ops/put_resource_policy.rs` only checks that the body parses as JSON (`serde_json::from_str::<serde_json::Value>`), so `"5"`, `"\"hello\""`, `"[]"` or `"{}"` are all stored and later returned verbatim by GetResourcePolicy.

Expected: a body that is not a policy document — not a JSON object, or an object with no `Statement` array, or a statement missing `Effect`/`Principal`/`Action` — is refused with the modeled `ValidationException` (HTTP 422). Actual: HTTP 200 and the non-policy value becomes the resource's policy.

## Evidence

Spec: `PutResourcePolicy` models `ValidationException` (HTTP 422) and `Policy` targets `ResourcePolicyJson`, whose doc describes a resource-based policy document; the real service rejects a document that is not a well-formed policy. Handler: `src/ops/put_resource_policy.rs` lines ~164-176 — the only check is `serde_json::from_str::<serde_json::Value>(&input.policy).is_err()`, which accepts any JSON scalar/array. Tests: `tests/all/resource_policy_and_tagging.rs::a_resource_policy_is_reported_from_what_was_attached` and `resource_policy_reads.rs` only exercise well-formed objects and a non-JSON string; no test asserts a refusal for parseable-but-not-a-policy input.

## Steps to Reproduce

1. Create an entity via StartChangeSet (Intent=APPLY) and wait for SUCCEEDED to obtain its EntityArn
2. POST /PutResourcePolicy with {"ResourceArn":"<entity arn>","Policy":"5"} — observe HTTP 200; expected ValidationException (422)
3. GET /GetResourcePolicy?resourceArn=<entity arn> — observe Policy:"5" reported back as the resource's policy
4. Repeat with Policy="{}" (an object with no Statement) — observe HTTP 200 instead of 422
