Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# TagResource stores tag keys that violate the modelled TagKey length and pattern (only the aws: prefix is checked)

_Filed: 2026-10-24T18:42:09Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagMap`'s key is `TagKey`, modelled `@length(min: 1, max: 128)` and `@pattern("^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$")`. Those are constraints on a MAP KEY, and the framework's constraint walk explicitly does not apply them: `vendor/smithy-schema-server/src/constraints.rs:569-570` — "Map *keys* are not covered (codegen does not capture key constraints yet)" — and line 1091, "the key itself is not a constrained leaf" (map VALUES are validated, keys are not). The handler adds only the documented `aws:`-prefix rule.

Expected: a `tags` map carrying a key of 129+ characters, an empty key, or a key with an out-of-class character (e.g. `bad!key`, `team#1`) is refused 400 `ValidationException` and nothing is stored.

Actual: `src/ops/tag_resource.rs:163-214` checks only `crate::state::reserved_tag_key(...)` and the accumulated 50-tag cap, then inserts every key verbatim into the workspace's tag map, so the emulator answers 200 and `ListTagsForResource`/`DescribeWorkspace` report a tag key the real service would never have accepted. The same omission applies to the `tags` members of `CreateWorkspace`, `CreateRuleGroupsNamespace` and `CreateAnomalyDetector`, which also only check the `aws:` prefix.

## Evidence

Spec: `com.amazonaws.amp#TagKey` @length(1,128) + @pattern; `TagResource` models `ValidationException` (HTTP 400). Framework: `vendor/smithy-schema-server/src/constraints.rs:569-570` and :1089-1100 state map keys are captured but not validated — the same reason this crate hand-checks `LabelName` map keys in `src/ops/update_workspace_configuration.rs:366-372`. Handler: `src/ops/tag_resource.rs:163-214`. Tests: `tests/all/workspace_lifecycle.rs:1005-1038` covers only reserved `aws:` keys; no test sends an over-long or out-of-pattern tag key.

## Steps to Reproduce

1. Create a workspace, wait for ACTIVE and read its arn.
2. POST /tags/{arn} with {"tags":{"bad!key":"v"}} — expected 400 ValidationException naming `tags`, actual 200.
3. POST /tags/{arn} with {"tags":{"<129 'a' characters>":"v"}} — expected 400 ValidationException, actual 200.
4. GET /tags/{arn} — the invalid keys are reported as stored tags.
