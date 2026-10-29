Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 08a55bcc278a7e8a745e318c09a635676933476e

# The "ResourceArn is not a networkmanager resource ARN" refusal is hand-rolled in all three tagging handlers, although state.rs already states it once in `malformed_resource_arn`

_Filed: 2026-10-29T03:58:26Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `08a55bcc278a7e8a745e318c09a635676933476e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::malformed_resource_arn` builds exactly one `ValidationException`: top-level message `"{member} is not a networkmanager resource ARN"`, `reason = REASON_FIELD_VALIDATION_FAILED`, and one `Fields` entry whose `name` is the member and whose message is `"must read arn:<partition>:networkmanager::<account>:<type>/<id>"`. Its own doc comment says the wording came FROM the three tagging handlers and names their files. Yet all three of those handlers still spell the same exception out by hand — the same four field values, the reason string `"FieldValidationFailed"` written as a literal instead of the `REASON_FIELD_VALIDATION_FAILED` constant, and the member name `"ResourceArn"` written as a literal instead of `crate::state::FIELD_RESOURCE_ARN`. `UpdateNetworkResourceMetadata`, which answers the same input class on the same member, goes through the helper.

So one rule — "a string that is not an ARN of this service is a 400 naming that member" — exists in four copies in this crate, three of them open-coded. They can drift: changing the helper (or the `REASON_FIELD_VALIDATION_FAILED` / `FIELD_RESOURCE_ARN` constants it reads) silently moves `UpdateNetworkResourceMetadata`'s refusal while leaving the three tagging refusals where they were, so the same malformed ARN would start reading differently depending on which operation it arrived on — precisely what the helper's doc says must not happen.

Replacing each hand-rolled block with `crate::state::malformed_resource_arn(crate::state::FIELD_RESOURCE_ARN)` is byte-for-byte identical on the wire (`FIELD_RESOURCE_ARN == "ResourceArn"`, `REASON_FIELD_VALIDATION_FAILED == "FieldValidationFailed"`), so no response and no existing test changes.

## Evidence

The single statement of the rule, services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5503-5516, whose doc names the three files that still duplicate it:
```
/// The wording is the one the tagging operations of this service already answer for the
/// same input class (`ops/list_tags_for_resource.rs`, `ops/tag_resource.rs`,
/// `ops/untag_resource.rs`), so one malformed ARN reads the same whichever operation it
/// arrived on.
pub fn malformed_resource_arn(member: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} is not a networkmanager resource ARN"),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: "must read arn:<partition>:networkmanager::<account>:<type>/<id>".to_string(),
        }]),
    }
}
```

Copy 1 — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/list_tags_for_resource.rs:152-162:
```
        let parts = crate::state::parse_resource_arn(&input.resource_arn).ok_or_else(|| {
            ListTagsForResourceErrorModeled::ValidationException(ValidationException {
                message: "ResourceArn is not a networkmanager resource ARN".to_string(),
                reason: Some("FieldValidationFailed".to_string()),
                fields: Some(vec![ValidationExceptionField {
                    name: "ResourceArn".to_string(),
                    message: "must read arn:<partition>:networkmanager::<account>:<type>/<id>"
                        .to_string(),
                }]),
            })
        })?;
```

Copy 2 — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/tag_resource.rs:162-173:
```
        let parts =
            crate::state::parse_resource_arn(&input.resource_arn).ok_or_else(|| {
                TagResourceErrorModeled::ValidationException(ValidationException {
                    message: "ResourceArn is not a networkmanager resource ARN".to_string(),
                    reason: Some("FieldValidationFailed".to_string()),
                    fields: Some(vec![ValidationExceptionField {
                        name: "ResourceArn".to_string(),
                        message: "must read arn:<partition>:networkmanager::<account>:<type>/<id>"
                            .to_string(),
                    }]),
                })
            })?;
```

Copy 3 — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/untag_resource.rs:159-169:
```
        let parts = crate::state::parse_resource_arn(&input.resource_arn).ok_or_else(|| {
            UntagResourceErrorModeled::ValidationException(ValidationException {
                message: "ResourceArn is not a networkmanager resource ARN".to_string(),
                reason: Some("FieldValidationFailed".to_string()),
                fields: Some(vec![ValidationExceptionField {
                    name: "ResourceArn".to_string(),
                    message: "must read arn:<partition>:networkmanager::<account>:<type>/<id>"
                        .to_string(),
                }]),
            })
        })?;
```

The fourth caller of the same rule already uses the helper — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_network_resource_metadata.rs:176-180:
```
        let parts = crate::state::parse_resource_arn(&input.resource_arn).ok_or_else(|| {
            UpdateNetworkResourceMetadataErrorModeled::ValidationException(
                crate::state::malformed_resource_arn(crate::state::FIELD_RESOURCE_ARN),
            )
        })?;
```

The constants that make the collapse response-identical — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5402 `pub const FIELD_RESOURCE_ARN: &str = "ResourceArn";` and the `REASON_FIELD_VALIDATION_FAILED` constant (= `"FieldValidationFailed"`) the helper already reads.

Test coverage that must stay green and unchanged:
- tests/all/arn_scoping.rs:375-441 — the case "a site id under the site's own kind twice over" expects 400 / `ValidationException` from `ListTagsForResource`, `TagResource` AND `UntagResource` on the same malformed ARN (`assert_eq!(error_code(&body), "ValidationException", ...)` at arn_scoping.rs:435-439).
- tests/all/list_tags_for_resource.rs — "End-to-end coverage of `ListTagsForResource` as a READER of stored tag state."
- tests/all/resource_tagging.rs — the tagging slice over every taggable kind.
- tests/all/network_resource_metadata.rs — the `UpdateNetworkResourceMetadata` path that already answers through the helper.

## Steps to Reproduce

1. Open services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:5495-5516 and read `malformed_resource_arn` together with its doc comment, which names `ops/list_tags_for_resource.rs`, `ops/tag_resource.rs` and `ops/untag_resource.rs` as the operations whose wording it states.
2. Open src/ops/list_tags_for_resource.rs:152-162, src/ops/tag_resource.rs:162-173 and src/ops/untag_resource.rs:159-169 and compare field by field with the helper: same top-level message, same reason, same `Fields` name and same `Fields` message — only written out by hand, with `"FieldValidationFailed"` and `"ResourceArn"` as literals.
3. Compare with src/ops/update_network_resource_metadata.rs:176-180, which answers the same input class on the same member by calling `crate::state::malformed_resource_arn(crate::state::FIELD_RESOURCE_ARN)`.
4. Simulate the drift: change the `Fields` message in `malformed_resource_arn` only. `UpdateNetworkResourceMetadata` reports the new wording while the three tagging operations keep the old one, with no compile error and no failing test — the exact outcome the helper's doc comment says it exists to prevent.
5. Confirm the collapse is response-neutral: `FIELD_RESOURCE_ARN` is `"ResourceArn"` (src/state.rs:5402) and `REASON_FIELD_VALIDATION_FAILED` is `"FieldValidationFailed"`, so the helper reproduces each hand-rolled exception exactly; tests/all/arn_scoping.rs:375-441 and tests/all/list_tags_for_resource.rs must stay green unchanged.
