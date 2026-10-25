Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# Factor the triplicated resourceArn "not a savingsplan resource" refusal into one helper

_Filed: 2026-10-25T15:51:10Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a resourceArn that passes the model @pattern but does not name a savingsplans savingsplan resource is a 400 ValidationException with this message" is implemented three separate times, verbatim, in the three handlers that take a resourceArn: ListTagsForResource, TagResource and UntagResource. All three copies are byte-identical apart from the per-operation error enum name, so the three can drift independently.

This is a one-rule-three-copies defect in a crate that already demonstrates the single-place pattern for the ADJACENT rule: the sibling not-found message was factored into `crate::state::resource_not_found_message`, whose doc comment states the template "lives in ONE place, shared by every operation that takes a `resourceArn`" — and all three handlers call it. The refusal branch right above that call was left duplicated.

The risk is concrete rather than hypothetical for this crate: a previously filed-and-fixed issue for this very service is titled "Savings Plan resourceArn validation and resource-not-found errors diverge across tagging operations", i.e. these three paths have already drifted once. Coverage is also uneven today: the only test that exercises this branch (an ARN of another service that still satisfies the model pattern) runs it for TagResource and ListTagsForResource and NOT for UntagResource, so a change to UntagResource's copy alone would not be caught.

The fix is a pure extraction: move the `savings_plan_id_from_arn(..).is_none()` check and its message into one helper in `src/state.rs` beside `resource_not_found_message` (handing back the message string, each handler wrapping it in its own ValidationException variant). Every response stays byte-identical — the three format strings are already the same — and every existing test keeps passing.

## Evidence

Copy 1 — services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/list_tags_for_resource.rs:136-150:
```
        // The model's `@pattern` already refused anything that is not a
        // Savings Plan ARN; what is left to refuse is an ARN of some other
        // service or resource type.
        if crate::state::savings_plan_id_from_arn(&input.resource_arn).is_none() {
            return Err(ListTagsForResourceErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "resourceArn '{}' is not the ARN of a {} savingsplan resource",
                        input.resource_arn,
                        crate::state::SERVICE_NAMESPACE
                    ),
                },
            )
            .into());
        }
```

Copy 2 — src/ops/tag_resource.rs:144-156:
```
        // The model's `@pattern` already refused anything that is not a
        // Savings Plan ARN; what is left to refuse is an ARN of some other
        // service or resource type.
        if crate::state::savings_plan_id_from_arn(&input.resource_arn).is_none() {
            return Err(TagResourceErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "resourceArn '{}' is not the ARN of a {} savingsplan resource",
                    input.resource_arn,
                    crate::state::SERVICE_NAMESPACE
                ),
            })
            .into());
        }
```

Copy 3 — src/ops/untag_resource.rs:134-146:
```
        // The model's `@pattern` already refused anything that is not a
        // Savings Plan ARN; what is left to refuse is an ARN of some other
        // service or resource type.
        if crate::state::savings_plan_id_from_arn(&input.resource_arn).is_none() {
            return Err(UntagResourceErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "resourceArn '{}' is not the ARN of a {} savingsplan resource",
                    input.resource_arn,
                    crate::state::SERVICE_NAMESPACE
                ),
            })
            .into());
        }
```

The single-place precedent this one rule skips — src/state.rs:611-620:
```
/// How this service states that the resource an operation was pointed at does
/// not exist.
///
/// AWS builds this message FROM THE REQUEST, naming the `resourceArn` the
/// caller sent — `Resource with resourceArn: <arn> doesn't exist` — so the
/// template is part of the contract and lives in ONE place, shared by every
/// operation that takes a `resourceArn`.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource with resourceArn: {resource_arn} doesn't exist")
}
```
and all three handlers do share that one (list_tags_for_resource.rs:160, tag_resource.rs:166, untag_resource.rs:156).

Test coverage that must stay green and unchanged:
* tests/all/savings_plans_lifecycle.rs:1154 `tagging_an_arn_that_names_no_plan_is_refused` — row `("arn:aws:ec2::000000000000:savingsplan/00000000-0000-4000-8000-000000000000", 400)` at line 1163, commented "Well formed under the model's pattern, but another service's ARN"; it loops only `for operation in ["TagResource", "ListTagsForResource"]` (line 1172), so UntagResource's copy of the branch is not covered.
* tests/all/tagging_error_paths.rs:61 `a_refused_resource_arn_is_named_from_the_request_object_inwards` and :265 `an_unknown_plan_is_reported_with_the_arn_the_caller_sent` — the boundary-pattern and 404 paths around it, run across all three operations.
* tests/all/tagging_error_paths.rs:328 `an_existing_plans_arn_still_round_trips_through_the_tagging_operations`, :370 `untagging_removes_exactly_the_keys_it_names` — the success paths past the branch.

## Steps to Reproduce

1. Open services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/list_tags_for_resource.rs:136-150, src/ops/tag_resource.rs:144-156 and src/ops/untag_resource.rs:134-146 side by side: the same three-line comment, the same `savings_plan_id_from_arn(..).is_none()` guard and the same `"resourceArn '{}' is not the ARN of a {} savingsplan resource"` message appear three times, differing only in the error-enum name.
2. Compare with src/state.rs:611-620, where the sibling not-found template was factored into `resource_not_found_message` with the doc comment "the template is part of the contract and lives in ONE place, shared by every operation that takes a `resourceArn`" — and which all three handlers call.
3. Confirm the drift risk is uneven coverage: tests/all/savings_plans_lifecycle.rs:1163 is the only row that reaches this branch (an `arn:aws:ec2:...:savingsplan/<uuid>` that satisfies the model pattern) and line 1172 runs it only for TagResource and ListTagsForResource, so editing UntagResource's copy alone breaks nothing in the suite.
4. Fix: add one helper beside `resource_not_found_message` in src/state.rs returning the refusal message for an ARN that is not a savingsplans savingsplan resource, and have the three handlers call it and wrap the string in their own ValidationException variant. Run `cargo test -p service-com-amazonaws-savingsplans-awssavingsplan`; every status, error code and message must be unchanged.
