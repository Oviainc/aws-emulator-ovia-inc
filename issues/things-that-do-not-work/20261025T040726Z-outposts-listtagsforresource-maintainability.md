Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# ListTagsForResource re-spells the ResourceArn refusal inline instead of using the shared report helper, and the copy renders the offending value

_Filed: 2026-10-25T04:07:26Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two tagging handlers implement the same rule — read `ResourceArn` with `crate::state::resource_arn_parts` and, when it is not an Outposts resource ARN, answer the operation's modelled `ValidationException` — but they word it in two different ways. `TagResource` delegates to the single helper the crate provides for this refusal (`crate::state::resource_arn_pattern_report()`); `ListTagsForResource` re-spells the message inline with a hand-written `format!` that interpolates the caller's ARN. The two copies have already drifted, and the inline one breaks the crate's own documented ValidationException wording rule (the report names the member and the constraint and NEVER renders the offending value). Collapsing the inline copy onto the existing helper leaves every reachable response unchanged — the modelled `Arn` pattern pins the service segment to `outposts`, so the boundary refuses every value `resource_arn_parts` would reject before the handler runs — and no test asserts the inline text (grep for "must be an Outposts resource ARN" in tests/ finds nothing).

## Evidence

The rule, stated once in this crate — src/state.rs:2630-2631:
```
// * `ValidationException` — the body is the standard AWS validation report, and
//   it never renders the offending value. See [`validation_report`].
```
and the same rule pinned by the suite — tests/all/error_message_rules.rs:14-18: "Rule B — `ValidationException` (HTTP 400): the body is the standard AWS validation report ... and it NEVER renders the offending value, whichever operation or member it came from."

Copy 1 (uses the helper) — src/ops/tag_resource.rs:156-161:
```
        let (resource_type, resource_id) = crate::state::resource_arn_parts(&input.resource_arn)
            .ok_or_else(|| {
                TagResourceErrorModeled::ValidationException(ValidationException {
                    message: Some(crate::state::resource_arn_pattern_report()),
                })
            })?;
```

Copy 2 (hand-rolled, renders the value) — src/ops/list_tags_for_resource.rs:155-163:
```
        let (resource_type, resource_id) = crate::state::resource_arn_parts(&input.resource_arn)
            .ok_or_else(|| {
                ListTagsForResourceErrorModeled::ValidationException(ValidationException {
                    message: Some(format!(
                        "ResourceArn must be an Outposts resource ARN, got {}",
                        input.resource_arn
                    )),
                })
            })?;
```

The helper that already exists for exactly this refusal — src/state.rs:2937-2949: "The report a `ResourceArn` that is not an Outposts resource ARN states for a handler that reads that ARN itself." / `pub fn resource_arn_pattern_report() -> String { constraint_report(RESOURCE_ARN_MEMBER, ViolationKind::Pattern(RESOURCE_ARN_PATTERN.to_string())) }`.

Tests that cover the two paths and must stay unchanged: tests/all/error_message_rules.rs:430 (`validation_reports_name_the_member_and_constraint_and_omit_the_value`) and tests/all/resource_tagging.rs (tagging round trips, incl. the tag-limit report at :368).

## Steps to Reproduce

1. Open src/ops/tag_resource.rs:156-161 and src/ops/list_tags_for_resource.rs:155-163 side by side: the same `resource_arn_parts` refusal is worded twice, once through `crate::state::resource_arn_pattern_report()` and once through an inline `format!` that interpolates `input.resource_arn`
2. Compare the inline wording with the rule stated at src/state.rs:2630-2631 and tests/all/error_message_rules.rs:14-18: a ValidationException body must never render the offending value
3. grep for "must be an Outposts resource ARN" under services/service-com-amazonaws-outposts-outpostsolafservice/tests — no test pins the inline text, so replacing it with the helper changes no asserted response
