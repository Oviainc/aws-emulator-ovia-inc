Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# TagResource accepts tag keys with the reserved aws: prefix

_Filed: 2026-10-24T15:33:28Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): the `aws:` tag-key prefix is reserved for AWS internal use across every tagging API — a request that tries to CREATE a tag whose key begins with `aws:` is refused with HTTP 400 `ValidationException` and no tag is written. MWAA's `TagKey` pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) permits the colon, so the boundary validator cannot refuse it and the rule has to be enforced in the handler.

Actual (emulator): `TagResource` resolves the ARN, checks only that the merge stays within the 50-tag ceiling, and then inserts every key/value pair verbatim (src/ops/tag_resource.rs:160-181). `TagResource(ResourceArn=<env arn>, Tags={'aws:cloudformation:stack-name':'mine'})` answers HTTP 200, and `ListTagsForResource` / `GetEnvironment` then report a reserved tag AWS would never have stored. The same unchecked path applies to the `Tags` map on `CreateEnvironment` (src/ops/create_environment.rs:441) — one shared check closes both.

## Evidence

Grounding: AWS's resource-tagging rule is documented service-wide ("Do not use the aws: prefix in your tag names or values because it is reserved for AWS use. You can't edit or delete tag names or values with this prefix"), and tagging APIs reject an attempt to create such a key with a client error; `TagResource` declares `ValidationException` (@httpError 400) for exactly this class. The model's `TagMap` key constraints (length 1..128, pattern allowing `:`) are the only thing the boundary validator can enforce, so the reserved-prefix rule is the handler's.
Handler: src/ops/tag_resource.rs:156-181 — ARN lookup, then the MAX_TAGS merge-count check, then `stored.tags.insert(key, value)` for every pair, with no key inspection. src/ops/create_environment.rs:441 stores `input.tags` unchecked.
Tests: tests/all/environment_lifecycle.rs:1160-1240 round-trips ordinary keys through TagResource/ListTagsForResource/UntagResource and asserts the map is stored as sent; tests/all/rejection_contract.rs covers only the ARN pattern and framework constraint refusals. No test asserts a refusal for a reserved key, so nothing currently pins the permissive behavior.

## Steps to Reproduce

1. Boot the emulator and create environment 'TagEnv'; note its Arn.
2. Call TagResource (POST /tags/<url-encoded arn>) with body {"Tags": {"aws:internal": "v"}}.
3. Expect HTTP 400 ValidationException (tag keys beginning with 'aws:' are reserved) and ListTagsForResource reporting no such key.
4. Observe HTTP 200; ListTagsForResource(ResourceArn=<arn>) reports {"aws:internal": "v"} and GetEnvironment('TagEnv') reports it under Tags.
5. Repeat the reserved key through CreateEnvironment's Tags map — it is stored the same way.
