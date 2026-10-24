Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Enforce the TagKey constraints on the tags supplied at CreateEmailContact

_Filed: 2026-10-24T05:23:11Z_
_Operation: CreateEmailContact | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEmailContactRequest$tags` is a `TagMap`, whose keys target `TagKey` (`@pattern "^(?!aws:).{1,128}$"`). A create naming a key with the reserved `aws:` prefix, an empty key, or a key longer than 128 characters must be refused with a 400 ValidationException and must persist nothing.

The emulator creates the contact anyway: `src/ops/create_email_contact.rs` stores `input.tags.unwrap_or_default()` verbatim, and the centralized constraint walk cannot refuse the key because `vendor/smithy-schema-server/src/constraints.rs::pattern_matches` treats a pattern that fails to compile as matching — and `^(?!aws:).{1,128}$` never compiles under the `regex` crate (no look-around). Since the 1..128 bound lives only inside that pattern, key length goes unchecked too.

Expected: 400 ValidationException, no contact created. Actual: HTTP 201 with the illegal tag key persisted and reported by ListTagsForResource/GetEmailContact flows.

## Evidence

Spec: smithy_query(query="shape", shape="TagMap") -> key TagKey pattern ^(?!aws:).{1,128}$. Framework no-op: vendor/smithy-schema-server/src/constraints.rs lines 454-462. Handler: src/ops/create_email_contact.rs line 201 (`tags: input.tags.unwrap_or_default().into_iter().collect()`) with no key screening. Test coverage: tests/all/resource_tags.rs tag_set_table() only builds legal keys (longest is exactly 128 chars); no test asserts a create with an `aws:`-prefixed or over-length key is rejected.

## Steps to Reproduce

1. CreateEmailContact(name="contact-b", emailAddress="contact-b@example.com", tags={"aws:owner": "team"})
2. Observe HTTP 201 and an arn; AWS answers 400 ValidationException and creates nothing
3. ListTagsForResource(arn=<arn>) -> the emulator reports the reserved key
4. Repeat with tags={"k"*129: "v"} -> also accepted by the emulator
