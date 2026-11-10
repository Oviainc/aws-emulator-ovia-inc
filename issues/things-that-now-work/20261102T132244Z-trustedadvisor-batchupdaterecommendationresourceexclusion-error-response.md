Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ac390f4705cff2073509ff718f5e29aedd87870c

# Repeated-ARN batch is answered 403 instead of 400 for an unentitled account, breaking the crate's "a malformed request is a ValidationException whether the caller is entitled or not" rule

_Filed: 2026-11-02T13:22:44Z_
_Operation: BatchUpdateRecommendationResourceExclusion | Class: error-response_

> _Note: this issue was identified against the code base at commit `ac390f4705cff2073509ff718f5e29aedd87870c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule about the support-level gate and malformed requests: a malformed request answers the modelled `ValidationException` (HTTP 400) regardless of whether the calling account's support plan includes the Trusted Advisor API. The crate's own suite pins that rule for every refusal the model's constraint traits can express (`shape_validation_precedes_the_gate`, including the empty-exclusion-batch row).

`BatchUpdateRecommendationResourceExclusion` owns the one other refusal the crate itself classifies as a malformed request: a list naming the same recommendation-resource ARN more than once, which its own module doc calls "not a well-formed request" and refuses with `ValidationException` (HTTP 400). But the handler orders that refusal AFTER the support-level gate, so the very same malformed request answers two different errors depending only on the caller's support plan:

* account on a plan that includes the API -> 400 `ValidationException` (the refusal the handler owns);
* account on Basic/Developer -> 403 `AccessDeniedException` ("Access denied due to support level"), with the malformed list never looked at.

So a request the crate itself calls malformed is NOT a `ValidationException` when the caller is unentitled, while an empty list (same operation, same "malformed request" class, but expressible as a modelled `@length` minimum) is 400 for exactly that caller. The two refusals of one operation disagree on the gate ordering purely because one condition is expressible as a constraint trait and the other is not — and the crate explicitly says the handler-raised one is "still one of this service's input-validation refusals".

Fix shape: evaluate the repeated-ARN well-formedness check ahead of `crate::state::support_level_refusal`, where the modelled-constraint refusals already sit, so this operation's two malformed-request refusals answer the same status for the same caller.

## Evidence

The rule, in the doc of the gate itself — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:1894-1899:
```
/// Every operation applies this BEFORE it resolves an identifier and before it
/// writes anything, so an unentitled caller learns it lacks access rather than
/// whether a recommendation exists, and a mutation it may not make changes
/// nothing. The modelled constraints are refused earlier still, by the
/// centralized request walk, so a malformed request is a `ValidationException`
/// whether the caller is entitled or not.
```

The crate classifying a handler-raised condition as one of the SAME refusals — src/service/validation.rs:47-50:
```
//! A condition the model's constraint traits CANNOT express is refused by the handler that
//! owns it, so the framework's walk never collects it and it reaches no [`Violation`]. Such
//! a refusal is still one of this service's input-validation refusals and is worded here
//! too: ...
```

The handler's own characterisation of the repeated-ARN request — src/ops/batch_update_recommendation_resource_exclusion.rs:16-20:
```
//! that the modelled `uniqueItems` compares whole `RecommendationResourceExclusion`
//! ... A list
//! that names one recommendation-resource ARN more than once asks for two
//! exclusion states for one resource, which is not a well-formed request: the
//! WHOLE request is refused with the modelled `ValidationException` (HTTP 400)
//! and no entry is applied, rather than the last occurrence silently winning.
```

The ordering that breaks it — the gate first, src/ops/batch_update_recommendation_resource_exclusion.rs:272-283:
```
        // The support-level gate, ahead of everything else this handler does —
        // ahead of applying any exclusion, so a batch this account may not make
        // changes nothing and reports nothing about which resources exist. See
        // [`crate::state::support_level_refusal`].
        if let Some(denied) = crate::state::support_level_refusal(&*self.engine) {
            return Err(
                BatchUpdateRecommendationResourceExclusionErrorModeled::AccessDeniedException(
                    denied,
                )
                .into(),
            );
        }
```
and only then the malformed-request refusal, :284-296:
```
        // Request well-formedness, ahead of every lookup and every write: a list
        // that names one recommendation-resource ARN twice asks for two exclusion
        // states for one resource, so the WHOLE request is refused and nothing is
        // applied — including the entries that name no repeated ARN.
        let repeated = repeated_arns(&input.recommendation_resource_exclusions);
        if let Some(message) = repeated_arn_refusal(&repeated) {
            return Err(
                BatchUpdateRecommendationResourceExclusionErrorModeled::ValidationException(
                    ValidationException { message },
                )
                .into(),
            );
        }
```

Test coverage that pins the rule for the modelled half of the same operation, and shows the handler-raised half is in no row — tests/all/support_level_gate.rs:309-314 and :351-356, :367-370:
```
/// The modelled constraints are refused AHEAD of the gate: a malformed request is a 400
/// `ValidationException` even from an account the gate would refuse — the rows differ
/// along the constraint broken (pattern, length, range, enum, list size, required
/// member).
#[tokio::test]
async fn shape_validation_precedes_the_gate() {
    let (router, _engine) = emulator_on_plan(SupportPlan::Basic);
```
```
        (
            "an empty exclusion batch, below the modelled list minimum",
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            Some(json!({ "recommendationResourceExclusions": [] })),
        ),
```
```
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 ahead of the gate, got {status} with {response}"
        );
```
The repeated-ARN suite drives the condition only on an entitled account, so nothing pins the unentitled answer either way — tests/all/batch_exclusion_unique_arns.rs:29-30 and :88-89:
```
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; that gate is pinned in `tests/all/support_level_gate.rs`.
```
```
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
```

## Steps to Reproduce

1. Start the emulator on an account whose plan excludes the Trusted Advisor API (leave EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN unset, or set it to `basic`).
2. PUT /v1/batch-update-recommendation-resource-exclusion with `{"recommendationResourceExclusions": []}` — an empty list, below the modelled @length(1,25) minimum. Observe HTTP 400 `ValidationException`: the modelled malformed-request refusal precedes the gate, as tests/all/support_level_gate.rs:314 asserts.
3. PUT the same route with the SAME recommendation-resource ARN twice, e.g. `{"recommendationResourceExclusions": [{"arn": "<seeded standard resource arn>", "isExcluded": true}, {"arn": "<the same arn>", "isExcluded": false}]}`.
4. Observe HTTP 403 `AccessDeniedException` with message "Access denied due to support level" — not the 400 `ValidationException` the handler's own doc (src/ops/batch_update_recommendation_resource_exclusion.rs:16-20) says a list naming one ARN twice earns, and not what src/state.rs:1897-1899 states for a malformed request from an unentitled caller.
5. Restart with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business and repeat step 3: the same bytes now answer 400 `ValidationException` (`1 validation error detected: Value '<arn>' at 'recommendationResourceExclusions' failed to satisfy constraint: Member must have unique arn values`), so one malformed request has two different refusal statuses that differ only by support plan.
