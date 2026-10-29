Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6249b35117da18b492ce466a823561b75d5aa9f1

# Hand-applied TagKey refusal omits the list element index the framework's input path carries

_Filed: 2026-10-28T21:46:26Z_
_Operation: InvokeDataAutomationAsync | Class: error-response_

> _Note: this issue was identified against the code base at commit `6249b35117da18b492ce466a823561b75d5aa9f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three operations that accept a tag key (`InvokeDataAutomationAsync` and `TagResource` through `tags`, `UntagResource` through `tagKeys`) apply the modelled `TagKey` pattern by hand, because the framework's boundary walk cannot compile its `(?!aws:)` lookahead. The refusal they build names a FIXED input path with no element index — `tags.member.key` / `tagKeys.member` — while the crate documents that path as "the framework's own input-path form" and states that the single refusal envelope exists "so one logical failure cannot read two different ways depending on which side caught it".

The framework's input-path form for a list element carries the 1-based index of the offending element (`<list>.<n>.member`). So inside this one service the two halves of the same member's constraints read differently: a tag key past the modelled `@length(max: 128)` is caught by the walk and reported at `tags.2.member.key`, while a tag key outside the modelled pattern is caught by the handler and reported at `tags.member.key`. The caller is told WHICH tag was too long but not WHICH tag was malformed — in a request carrying up to 200 tags the handler-side refusal does not identify the offending element at all.

The fix is confined to `malformed_tag_key_refusal_at`, which already iterates the keys and therefore has the index available; `violation_clause` / `validation_envelope` / `malformed_tag_key_message` stay as they are.

## Evidence

The crate claims the framework's path form and single-reading goal:
- `services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/src/state.rs:149`: "/// `path` is the framework's own input-path form (segments joined with `.`, with `member` for the step from a list into its element)."
- `.../src/state.rs:162`: "/// The ONE place this service's refusal envelope is written: the boundary walk's refusals reach it through `service::override_refusal`, and a rule the walk cannot express (the `TagKey` lookahead) reaches it through [`malformed_tag_key_message`], so one logical failure cannot read two different ways depending on which side caught it."
- `.../src/state.rs:182`: "/// The input path a `tags` list's key occupies, in the framework's own input-path form: the `tags` member, the step into one element (`member`), then that element's `key`." / `const TAGS_MEMBER_KEY_PATH: &str = "tags.member.key";` (`:184`) and `const TAG_KEYS_MEMBER_PATH: &str = "tagKeys.member";` (`:188`) — neither carries an index, and `malformed_tag_key_refusal_at` (`.../src/state.rs:197`) passes that constant through unchanged: `keys.into_iter().any(|key| !tag_key_is_well_formed(key)).then(|| malformed_tag_key_message(path))`.

The framework's actual path form, which the crate cites as its reference:
- `vendor/smithy-schema-server/src/constraints.rs:34`: "Member names are lowerCamel; list elements contribute a `<1-based-index>.member` segment (`keySchema.1.member.keyType`); map entries contribute the string key as a segment."
- `vendor/smithy-schema-server/src/constraints.rs:1876` (list of structs, exactly the `tags: list<Tag>` shape): `assert_eq!(violations[0].path, "keySchema.2.member.keyType");` with message "Value 'BOGUS' at 'keySchema.2.member.keyType' failed to satisfy constraint: ...".

Both refusals are reachable on the same member, by this crate's own test:
- `.../tests/all/tag_key_refusal.rs:190`: "// The modelled `TagKey` upper length bound (`@length(min: 1, max: 128)`, `smithy_query` shape `TagKey`): a key AT the bound is still admitted, and the walk refuses one past it with its own length refusal, which is not this rule."

Test coverage: `.../tests/all/tag_key_refusal.rs:260-270` pins the un-indexed paths (`"tags.member.key"`, `"tagKeys.member"`) for all three operations, and `.../tests/all/data_automation_invocations.rs:1432` (`every_operation_that_accepts_a_tag_key_applies_the_modelled_tag_key_pattern`) asserts the same message; no test in the crate exercises a walk-caught violation on a list element, so nothing pins the indexed form against the un-indexed one.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and a well-formed submission (inputConfiguration.s3Uri, outputConfiguration.s3Uri, dataAutomationProfileArn, a dataAutomationConfiguration naming a project that exists) to mint an invocation ARN.
2. POST / with `X-Amz-Target: ...TagResource` and body {"resourceARN": <that ARN>, "tags": [{"key":"ok","value":"v"},{"key":"<129 characters>","value":"v"}]} — the boundary walk refuses it and the message names `tags.2.member.key` (vendor constraints.rs path rule), identifying the offending element.
3. POST / with `X-Amz-Target: ...TagResource` and body {"resourceARN": <that ARN>, "tags": [{"key":"ok","value":"v"},{"key":"aws:reserved","value":"v"}]} — the handler refuses it and the message names `tags.member.key`, with no element index, so the same member's two constraints report in two different path forms and the caller cannot tell which tag was rejected.
4. Repeat step 3 against `UntagResource` with {"resourceARN": <that ARN>, "tagKeys": ["ok", "aws:reserved"]} — the message names `tagKeys.member` rather than `tagKeys.2.member`.
