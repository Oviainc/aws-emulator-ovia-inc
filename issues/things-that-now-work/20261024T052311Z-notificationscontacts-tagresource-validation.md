Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Enforce the TagKey constraints (no reserved aws: prefix, 1..128 characters) on TagResource

_Filed: 2026-10-24T05:23:11Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagMap`'s key target is `TagKey`, constrained by `@pattern "^(?!aws:).{1,128}$"` — a key may not begin with the reserved `aws:` prefix, may not be empty, and may not exceed 128 characters. Real AWS rejects such a key with a 400 ValidationException.

The emulator accepts and persists them. The handler (`src/ops/tag_resource.rs`) validates only the MERGED map cardinality against 200, and the centralized constraint walk cannot help: `vendor/smithy-schema-server/src/constraints.rs::pattern_matches` compiles each `@pattern` with the `regex` crate and, on compile failure, RETURNS TRUE ("a pattern that fails to compile is treated as matching"). The `regex` crate has no look-around support, so `^(?!aws:).{1,128}$` never compiles and the key constraint is silently a no-op — and because the 1..128 length bound lives only inside that same pattern (TagKey carries no separate `@length`), key length is unenforced too.

Expected: TagResource with tags={"aws:owner":"x"} or a 129-character key or an empty key -> 400 ValidationException, nothing persisted. Actual: HTTP 200 and ListTagsForResource reports the illegal key.

## Evidence

Spec: smithy_query(query="shape", shape="TagMap") -> key target TagKey with pattern ^(?!aws:).{1,128}$, value TagValue length 0..256. Framework: vendor/smithy-schema-server/src/constraints.rs lines 420-462 (`let Ok(re) = regex::Regex::new(regex) else { return true; }`) means the look-ahead pattern is skipped, so nothing rejects the key. Handler: src/ops/tag_resource.rs lines 146-193 — only the merged tag COUNT is checked. Test coverage: tests/all/resource_tags.rs defines MAX_TAG_KEY_LEN = 128 and only exercises keys AT the bound ("k".repeat(128)); grep for `aws:`/129/reserved across tests/ finds no case asserting rejection of a reserved-prefix or over-length key.

## Steps to Reproduce

1. CreateEmailContact(name="contact-a", emailAddress="contact-a@example.com") -> arn
2. TagResource(arn=<arn>, tags={"aws:reserved": "v"}) -> emulator answers 200; AWS answers 400 ValidationException
3. ListTagsForResource(arn=<arn>) -> the emulator reports the reserved key as persisted
4. Repeat with tags={"k"*129: "v"} and with tags={"": "v"} -> both accepted by the emulator, both rejected by AWS
