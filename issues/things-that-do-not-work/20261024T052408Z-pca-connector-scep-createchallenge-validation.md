Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Tag keys and values are stored without any of AWS's tag validation rules (only the entry count is checked)

_Filed: 2026-10-24T05:24:08Z_
_Operation: CreateChallenge | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a create that carries `Tags` is rejected with ValidationException (400) when a tag does not satisfy AWS's tagging rules — an empty or >128-character key, a >256-character value, characters outside the allowed tag alphabet, or a key in the reserved `aws:` namespace (system tags cannot be set by a caller). ValidationException's own documentation names exactly this case: "An input validation error occurred. For example, invalid characters in a name tag".

Actual: the handler checks only how MANY entries the map has (against the emulator's 50-tag ceiling) and then stores the map verbatim. `Tags` is modeled as `map<String,String>` with no @length/@pattern on key or value, so the framework's boundary validator enforces nothing either: `{"": "x"}`, a 500-character value and `{"aws:cloudformation:stack-name": "mine"}` are all accepted with HTTP 202 and become part of the challenge's stored tag set (which would then be reported by ListTagsForResource once that operation is implemented). The same count-only check sits on CreateConnector's tag path, but CreateConnector already carries an open validation issue, so this one is filed against CreateChallenge.

## Evidence

Spec: smithy_query shape=Tags → `{"type": "map", "key": "String", "value": "String"}` — no constraints, so nothing is enforced at the boundary and the handler is the only place a tag rule could live; smithy_query shape=ValidationException → doc "An input validation error occurred. For example, invalid characters in a name tag, or an invalid pagination token", and CreateChallenge declares ValidationException (400) (OPERATIONS.md, CreateChallenge errors). AWS-wide tagging rules (key 1-128 chars, value 0-256 chars, `aws:` prefix reserved for system tags) are well-established.

Handler: src/ops/create_challenge.rs lines 264-279 — `tag_count > MAX_TAGS_PER_RESOURCE` is the ONLY tag check, then `input.tags.unwrap_or_default().into_iter().collect()` is stored on StoredChallenge.tags (src/state.rs lines 164-166). Same shape at src/ops/create_connector.rs lines 255-270.

Test coverage: tests/all/connector_lifecycle.rs `tag_count_limit_is_enforced_on_both_creates` (line 1387) asserts the count ceiling only; no case passes an empty key, an over-long key/value, a disallowed character or an `aws:`-prefixed key.

## Steps to Reproduce

1. CreateConnector with a valid CA ARN and let it settle to ACTIVE.
2. POST /challenges with {"ConnectorArn": "<arn>", "Tags": {"": "empty-key"}}. Actual: HTTP 202 and the empty-keyed tag is stored. Expected: HTTP 400 ValidationException.
3. Repeat with Tags = {"aws:cloudformation:stack-name": "mine"} (reserved namespace), then with a 200-character key, then with a 400-character value. Actual: HTTP 202 in each case. Expected: HTTP 400 ValidationException for each.
