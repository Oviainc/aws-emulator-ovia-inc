Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# Fold the hand-rolled "expected vendor" refusal sentence in validate_resource_arn onto service_common::validation_message::expected_vendor

_Filed: 2026-10-27T02:22:06Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_resource_arn` — the one ARN check every tagging operation of this crate runs (`ListTagsForResource`, `TagResource`, `UntagResource`) — builds the wrong-vendor refusal sentence with a local `format!`. The shared `service-common` crate, which this crate already depends on and already calls for the ARN parse on the very next line (`::service_common::arn::is_resource_arn_of`), owns that exact sentence as `validation_message::expected_vendor`, and that function's own doc names THIS file as still carrying an inlined copy that is to be migrated rather than copied again.

The rendered bytes are identical (`expected_vendor("aws-marketplace")` == `"Invalid input resource arn: expected vendor aws-marketplace"`), so folding the local `format!` onto the shared helper changes no response, no status, no error code and no test. Keeping the copy is what lets the two drift: a change to the shared sentence would silently leave this service wording the same refusal differently from the other services that use it.

## Evidence

The shared crate owns the sentence AND names this file as owing the migration — `services/common/src/validation_message.rs:162-176`:
```
/// The sentence the resource-tagging layer in front of a service refuses a caller-supplied
/// `resourceArn` of ANOTHER vendor with: it names the vendor it expected ...
/// `services/service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs` and
/// `services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs`
/// still carry their own inlined copies of this sentence; they are to be migrated onto this
/// one rather than copied again.
#[must_use]
pub fn expected_vendor(vendor: &str) -> String {
    format!("Invalid input resource arn: expected vendor {vendor}")
}
```

The inlined copy in this crate — `services/.../src/state.rs:918-922`:
```
    if !::service_common::arn::is_resource_arn_of(candidate, ARN_NAMESPACE) {
        return Err(crate::scaffolded::BadRequest::new(format!(
            "Invalid input resource arn: expected vendor {ARN_NAMESPACE}"
        )));
    }
```

The dependency is already present — `services/.../Cargo.toml:12`: `service-common = { path = "../common" }`, and a grep over `src/` shows `validation_message` is never referenced by this crate.

Tests that cover the span and stay unchanged: `tests/all/resource_arn_validation.rs:306-311` asserts only that the message names the vendor —
```
            if names_vendor {
                assert!(
                    reported.contains(ARN_NAMESPACE),
                    "case {case:?}/{operation}: the message names the expected vendor \
                     {ARN_NAMESPACE:?}, got {reported:?}"
                );
```
and `tests/all/not_found_error_code.rs` / `tests/all/resource_arn_classification.rs` pin the code/status of the same refusal. Since `expected_vendor(ARN_NAMESPACE)` renders the identical string, every one of them passes untouched.

## Steps to Reproduce

1. Open `services/common/src/validation_message.rs` and read the doc of `expected_vendor` (lines 162-176): it states that `services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs` "still carr[ies] their own inlined copies of this sentence; they are to be migrated onto this one rather than copied again."
2. Open `services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs:918-922` and observe the local `format!("Invalid input resource arn: expected vendor {ARN_NAMESPACE}")` inside `validate_resource_arn`, one line after the crate already calls `::service_common::arn::is_resource_arn_of`.
3. Grep `src/` for `validation_message`: no hit, so the shared helper is unused here even though `service-common` is a declared dependency (`Cargo.toml:12`).
4. Compare the two renderings for this service's namespace: both produce `Invalid input resource arn: expected vendor aws-marketplace`, so `tests/all/resource_arn_validation.rs:306-311` and the sibling ARN suites assert the same bytes before and after the fold.
