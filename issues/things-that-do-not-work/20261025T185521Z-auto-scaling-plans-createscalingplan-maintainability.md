Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# Fold the triplicated "names a non-empty CloudFormationStackARN" predicate into one helper

_Filed: 2026-10-25T18:55:21Z_
_Operation: CreateScalingPlan | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "an `ApplicationSource` names a CloudFormation stack when `CloudFormationStackARN` is present and non-empty" is written three times in `src/state.rs`, as the same `cloud_formation_stack_arn` + `.filter(|arn| !arn.is_empty())` chain: once in `application_source_key` (which needs the ARN value), once in `names_stack` (which needs only the boolean), and once inline in `validate_application_source` (which needs the value, then asks `names_stack` for the boolean two lines later). Three copies of one predicate can drift: changing what counts as "names a stack" (e.g. trimming whitespace) in one copy and not the others would make the exclusivity check, the identity key and the ARN-format check disagree about the same source, which is exactly the inconsistency the single-plan-per-application-source rule depends on not happening.

One helper returning `Option<&str>` (the named stack ARN, if any) serves all three call sites: `application_source_key` maps it to `stack:{arn}`, `validate_application_source` ARN-checks it, and `names_stack` becomes `.is_some()`. Every refusal message, identity key and response stays byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:263-270:
```
pub fn application_source_key(source: &ApplicationSource) -> Option<String> {
    if let Some(arn) = source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        return Some(format!("stack:{arn}"));
    }
```
Copy 2 — state.rs:297-303:
```
/// Whether a `CloudFormationStackARN` is present and non-empty.
fn names_stack(source: &ApplicationSource) -> bool {
    source
        .cloud_formation_stack_arn
        .as_deref()
        .is_some_and(|arn| !arn.is_empty())
}
```
Copy 3 — state.rs:345-355, which then calls copy 2 on the next line:
```
pub fn validate_application_source(source: &ApplicationSource) -> Result<String, String> {
    if let Some(arn) = source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        if !is_cloud_formation_stack_arn(arn) {
            return Err(format!("Invalid CloudFormation stack ARN: {arn}"));
        }
    }
    match (names_stack(source), names_tag_filters(source)) {
```

Tests that already cover all three call sites and must stay unchanged: tests/all/application_source_exclusivity.rs (module doc, lines 3-23: "One application source is claimed by at most one scaling plan, on EVERY path that sets a plan's `ApplicationSource`" — table rows for a stack ARN and for tag-filter respellings, driving both CreateScalingPlan and UpdateScalingPlan), and the "documented combination rules" table in tests/all/scaling_plan_lifecycle.rs (malformed/empty/both-member sources refused 400), plus tests/all/error_message_wording.rs for the refusal texts.

## Steps to Reproduce

1. Read src/state.rs:263-270, 297-303 and 345-355: the same "cloud_formation_stack_arn present and non-empty" predicate appears three times, twice as the value-returning `.filter(|arn| !arn.is_empty())` chain and once as the boolean `names_stack`.
2. Note that `validate_application_source` holds its own copy and then calls `names_stack(source)` at state.rs:355 for the same question it just answered inline.
3. Confirm the behaviour is one rule: `CreateScalingPlan` (create_scaling_plan.rs:164) and `DescribeScalingPlans` (describe_scaling_plans.rs:206) call `validate_application_source`, while `plan_covering_source` (state.rs:238-242) decides exclusivity through `application_source_key` — all three must agree on what names a stack.
4. Run the crate's tests (application_source_exclusivity, scaling_plan_lifecycle, error_message_wording) before and after folding the three copies into one `Option<&str>` helper: responses and assertions are unchanged.
