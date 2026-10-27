Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# UntagResource accepts TagKeys the modelled TagKey pattern rejects, while sibling TagResource refuses them

_Filed: 2026-10-25T05:00:46Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource$TagKeys` targets `TagKeyList`, whose member is `TagKey` with `@pattern "^(?!aws:)[a-zA-Z+-=._:/]+$"`. The sibling `TagResource` handler in this same crate applies that pattern itself, in the handler, with an explicit comment explaining that the boundary validator CANNOT apply it (the pattern opens with a negative lookahead, the `regex` crate has no look-around, and the vendored validator treats an uncompilable pattern as matching). `UntagResource` applies no such check: its handler goes straight from the ARN lookup and the authorization decision to `record.tags.remove(key)` for every key the request carried, so a `DELETE /tags/{ResourceArn}?tagKeys=aws%3Aowner` answers 204 (no content, success) while the identical key sent to `TagResource` is refused 400/`BadRequestException`.

Worse, `UntagResource`'s own doc comment asserts the opposite of what `TagResource`'s comment establishes: it claims the `@pattern` constraints on the keys are validated by the centralized walk. One of the two comments in this crate is wrong about the same constraint on the same shape, and the code under the `UntagResource` one is the one that lets the value through.

The crate already has the frontend wiring for a `tagKeys` refusal — `crate::service`'s `TAGGING_MEMBER_PATHS` includes `"tagKeys"` and words such a refusal with the tagging frontend's `BadRequestException` code — so nothing new has to be invented; only the check is absent on this path.

## Evidence

Model (smithy_query shape=TagKeyList): `"member": {"target": "TagKey", "length": {"min": 1, "max": 128}, "pattern": "^(?!aws:)[a-zA-Z+-=._:/]+$"}`.

Sibling applies the check — services/service-com-amazonaws-rum-rum/src/ops/tag_resource.rs:198-203: "This check belongs in the handler rather than at the boundary: `TagKey`'s pattern opens with the negative lookahead `(?!aws:)`, the vendored boundary validator compiles a `@pattern` with the `regex` crate, which has no look-around, and it treats a pattern it cannot compile as matching (`pattern_matches` in `vendor/smithy-schema-server/src/constraints.rs`). So the constraint the model declares would otherwise never be applied to a tag key at all."
and tag_resource.rs:391-395:
```
        for (key, _) in &requested {
            if let Some(message) = tag_key_violation(&format!("tags.{key}.member"), key) {
                return Err(TagResourceError::BadRequest(ValidationException { message }));
            }
        }
```

The boundary indeed cannot apply it — vendor/smithy-schema-server/src/constraints.rs:421-424: "True if `value` satisfies the Smithy `@pattern` `regex` (a find, not an anchored match). A pattern that fails to compile is treated as matching (we never reject a value because of an unparseable model pattern)."

This handler claims the opposite — services/service-com-amazonaws-rum-rum/src/ops/untag_resource.rs:135-138: "including `ResourceArn`, which is still refused when absent (the framework's own missing-member refusal), and the `@length`/`@pattern` constraints on the keys, which the centralized walk validates after this step against the SAME input schema."

And it removes without any key check — untag_resource.rs:383-385:
```
        for key in &input.tag_keys {
            record.tags.remove(key);
        }
```

Test coverage proves only `TagResource` is exercised: services/service-com-amazonaws-rum-rum/tests/all/resource_tagging.rs:481 `a_tag_key_outside_the_modelled_pattern_is_refused_and_writes_nothing`, whose loop body at :491 calls `tag_resource(&router, &arn, &requested).await` only, over `REJECTED_TAG_KEYS` (resource_tagging.rs:86: `"aws:owner", "aws:", ...`). No case sends a rejected key to `UntagResource`.

## Steps to Reproduce

1. Boot the emulator with the rum service mounted and create an app monitor: POST /appmonitor {"Name":"tag-pattern","Domain":"example.com","Tags":{"Env":"dev"}} -> note the ARN arn:aws:rum:<region>:<account>:appmonitor/tag-pattern
2. POST /tags/<ARN> with body {"Tags":{"aws:owner":"x"}} -> 400 with error code BadRequestException (TagResource applies the modelled TagKey pattern)
3. DELETE /tags/<ARN>?tagKeys=aws%3Aowner -> 204 No Content: the same key, refused by TagResource, is accepted by UntagResource because the modelled TagKey @pattern is applied on neither the boundary nor this handler
