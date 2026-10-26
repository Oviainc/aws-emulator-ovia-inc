Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# Enforce the TagKey constraints on UntagResource tagKeys elements

_Filed: 2026-10-24T05:23:11Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResourceRequest$tagKeys` is a `TagKeys` list whose member targets `TagKey` (`@pattern "^(?!aws:).{1,128}$"`). A request naming an element with the reserved `aws:` prefix, an empty element, or an element longer than 128 characters must be refused with a 400 ValidationException.

The emulator answers 200: `src/ops/untag_resource.rs` only resolves the contact and then calls `contact.tags.remove(key)` for each element (a key that is not present is a no-op), and the centralized constraint walk cannot reject the element because `pattern_matches` treats the uncompilable look-ahead pattern as matching. A caller passing a malformed key therefore gets a success where AWS returns a validation failure.

## Evidence

Spec: smithy_query(query="shape", shape="TagKeys") -> member TagKey, pattern ^(?!aws:).{1,128}$; UntagResource declares ValidationException (400). Framework no-op: vendor/smithy-schema-server/src/constraints.rs lines 454-462 (uncompilable pattern => matches). Handler: src/ops/untag_resource.rs lines 145-173. Test coverage: tests/all/resource_tag_writes.rs exercises only legal keys (longest exactly 128 chars); no case asserts a 400 for a reserved-prefix or over-length tagKeys element.

## Steps to Reproduce

1. CreateEmailContact -> arn
2. DELETE /tags/{arn}?tagKeys=aws%3Areserved (UntagResource with an `aws:`-prefixed key)
3. Observe HTTP 200 from the emulator; AWS answers 400 ValidationException
4. Repeat with a 129-character tagKeys element -> also 200 from the emulator
