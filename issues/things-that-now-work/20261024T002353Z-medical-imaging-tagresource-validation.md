Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# Enforce the TagKey pattern (including the reserved aws: prefix) in TagResource

_Filed: 2026-10-24T00:23:53Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` accepts and persists any tag key whose LENGTH is 1..128, regardless of the characters it contains. The model constrains a tag key with `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$` (shape `TagKey`, reached through `TagMap`), which forbids (a) keys beginning with the reserved `aws:` prefix and (b) keys containing any character outside `[a-zA-Z+-=._:/]` (a space, `*`, a digit-only key, non-ASCII text, …).

Expected: a `TagResource` call carrying `{"aws:owner": "x"}` or `{"my key *": "x"}` is refused with HTTP 400 `ValidationException` and the resource's tag map is unchanged.

Actual: HTTP 200, the key is stored, and a following `ListTagsForResource` reports it — so a client can create tags AWS would never accept, and the reserved `aws:` namespace is writable.

Root cause: the framework's `@pattern` checker compiles the model pattern with the `regex` crate, which does not support the `(?!…)` negative look-ahead; `pattern_matches` treats a pattern that fails to compile as MATCHING, so this one constraint is silently never applied. Other services in this tree handle look-ahead patterns in their own code for exactly this reason (see `services/service-com-amazonaws-datazone-datazone/src/state.rs`, which re-implements its look-ahead pattern by hand), so the enforcement belongs in this crate's handler/state code.

## Evidence

Spec: `smithy_query(query="shape", shape="TagMap")` → key target `TagKey`, length 1..128, pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`. Well-established AWS semantics: tag keys prefixed `aws:` are service-reserved and rejected.
Handler: `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/tag_resource.rs` lines ~176-202 — the only checks are ARN parse, resource existence and the emulator's own `MAX_TAGS_PER_RESOURCE` bound; keys are inserted verbatim with no character or prefix check.
Framework: `vendor/smithy-schema-server/src/constraints.rs` lines 454-462 — `let Ok(re) = regex::Regex::new(regex) else { return true; }`, i.e. an uncompilable model pattern is treated as satisfied, and `(?!` does not compile with the `regex` crate.
Tests: `tests/all/resource_tagging.rs` `round_trip_cases()` (lines 377-442) exercises only legal keys (length bounds and the allowed punctuation `ns:sub/part.v-1`); no case asserts a refusal for an `aws:`-prefixed key or a key with a disallowed character, so this is uncovered.

## Steps to Reproduce

1. CreateDatastore, then GetDatastore to read `datastoreProperties.datastoreArn`
2. POST /tags/{datastoreArn} with body {"tags": {"aws:owner": "reserved"}}
3. Observe HTTP 200 instead of HTTP 400 ValidationException
4. GET /tags/{datastoreArn} and observe the reserved key reported back
5. Repeat with a key containing a disallowed character, e.g. {"tags": {"my key *": "v"}} — also accepted
