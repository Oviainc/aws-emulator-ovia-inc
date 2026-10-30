Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3721cdcd5d81ad7b59ab57035aad7fc30db6f74c

# The input-validation refusal sentence is built a second time inside the batch handler, in the one place the crate says it must not be

_Filed: 2026-10-30T01:56:47Z_
_Operation: BatchUpdateRecommendationResourceExclusion | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3721cdcd5d81ad7b59ab57035aad7fc30db6f74c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, as a rule over the whole service, that the WORDS of an input-validation refusal live in exactly one module: `src/service/validation.rs` "owns the sentence, so the clause rules live in one place instead of being spread through the handlers that never see them", and its rule 1 fixes the envelope (`<N> validation error(s) detected: ` over one clause per failing value, joined with `; `) "kept so the two refusal paths cannot disagree".

`BatchUpdateRecommendationResourceExclusion` is the one handler that raises a refusal of its own (the repeated-ARN check, which the modelled `uniqueItems` cannot express because it compares whole structures). To word it, `repeated_arn_refusal` re-implements BOTH halves of that rule inline: the clause skeleton `<subject> at '<member path>' failed to satisfy constraint: <requirement>` and the singular/plural count envelope — byte-for-byte the same two formats `validation::refusal_message` already holds, and its own doc comment names that module as the source ("in the sentence this service builds every input-validation refusal from (see `crate::service::validation`)").

So the envelope the crate says exists once now exists in two implementations inside `src/`, and they can drift: a change to the envelope in `validation.rs` (the module the crate designates as its owner) leaves the batch handler answering the old sentence, which is exactly the disagreement rule 1 exists to prevent. Nothing about the refused CONDITION is duplicated — only the sentence-building — so the fix is to have both paths call one clause/envelope builder.

This is a pure refactor: the requirement text the batch clause carries ("Member must have unique arn values") and the member path it names stay exactly as they are, so every response and every pinned message is unchanged.

## Evidence

The rule, stated for the whole service — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/service/validation.rs:7-12:
```
//! [`crate::service`] owns the SEAM — which requests are read and validated, and how
//! the modelled shape is serialized — and this module owns the sentence, so the clause
//! rules live in one place instead of being spread through the handlers that never see
//! them. What is ENFORCED is always the model's: every [`Violation`] handed to
//! [`refusal_message`] comes from `aws_smithy_schema_server::constraints::validate`, the
//! same call the centralized request walk makes.
```
and src/service/validation.rs:17-19:
```
//! 1. the envelope is `<N> validation error(s) detected: ` over one clause per failing
//!    member, joined with `; ` ([`refusal_message`]) — the framework's own envelope,
//!    kept so the two refusal paths cannot disagree.
```

Copy 1, the designated owner — src/service/validation.rs:197-218:
```
pub(super) fn refusal_message(
    violations: &[Violation],
    collections: &RenderedCollections,
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::with_capacity(violations.len());
    for violation in violations {
        let requirement = requirement_of(&violation.kind)?;
        let subject = subject(violation, collections);
        clauses.push(format!(
            "{subject} at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ));
    }
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}
```

Copy 2, inside the handler that the rule above says must not hold one — src/ops/batch_update_recommendation_resource_exclusion.rs:126-153 (its own doc naming the other copy, then the two formats again):
```
/// The `message` of the refusal a repeated ARN earns, in the sentence this service
/// builds every input-validation refusal from (see `crate::service::validation`):
/// the clause count, then one `<subject> at '<member path>' failed to satisfy
/// constraint: <requirement>` clause per offending value, joined with `; `.
...
fn repeated_arn_refusal(repeated: &[&str]) -> String {
    let clauses: Vec<String> = repeated
        .iter()
        .map(|arn| {
            format!(
                "Value '{arn}' at 'recommendationResourceExclusions' failed to satisfy \
                 constraint: Member must have unique arn values"
            )
        })
        .collect();
    match clauses.as_slice() {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}
```
and the handler calls it as its own refusal — src/ops/batch_update_recommendation_resource_exclusion.rs:278-288:
```
        let repeated = repeated_arns(&input.recommendation_resource_exclusions);
        if !repeated.is_empty() {
            return Err(
                BatchUpdateRecommendationResourceExclusionErrorModeled::ValidationException(
                    ValidationException {
                        message: repeated_arn_refusal(&repeated),
```

Test coverage that pins both sentences and so must stay byte-identical after the fix: tests/all/batch_exclusion_unique_arns.rs:142-164 mirrors the same envelope a third time for the assertion (`fn expected_message`, with the comment "in the sentence this service builds every input-validation refusal from (`src/service/validation.rs`)") and is asserted by `a_batch_naming_one_resource_arn_twice_is_refused_whole` (tests/all/batch_exclusion_unique_arns.rs:185) and `a_batch_of_distinct_resource_arns_still_applies` (:326); tests/all/validation_message_rendering.rs pins `refusal_message`'s output for the framework-collected violations of every operation.

## Steps to Reproduce

1. Read services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/service/validation.rs:7-12 and :17-19 — the crate states that the clause rules and the `<N> validation error(s) detected: ` envelope live in this one module, 'instead of being spread through the handlers'.
2. Read src/service/validation.rs:197-218 — `refusal_message` implements that clause skeleton and that envelope.
3. Read src/ops/batch_update_recommendation_resource_exclusion.rs:135-153 — `repeated_arn_refusal` implements the same clause skeleton and the same singular/plural envelope a second time, inside a handler, with a doc comment that points at `crate::service::validation` as the source of the sentence it is copying.
4. Change the envelope wording in `refusal_message` (e.g. the plural clause join) and observe that the batch handler's repeated-ARN refusal keeps the old sentence: the two refusal paths now disagree, which is the outcome src/service/validation.rs:17-19 says the single implementation exists to prevent.
5. Confirm the fix is response-preserving: make both paths share one clause/envelope builder, keeping the batch clause's own subject, member path ('recommendationResourceExclusions') and requirement ('Member must have unique arn values') exactly as they are, and re-run tests/all/batch_exclusion_unique_arns.rs and tests/all/validation_message_rendering.rs — both assert the same strings as before.
