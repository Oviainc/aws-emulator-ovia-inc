Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0559e2588b1548e68d881f6a6f917eedea630bd1

# LimitExceededException and InternalServerException are answered with the modelled Code member unset, breaking the crate's own "every member the shape declares is reported" rule

_Filed: 2026-10-28T18:55:40Z_
_Operation: CreateLifecyclePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `0559e2588b1548e68d881f6a6f917eedea630bd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in its own state module, that the `Code` member of a DLM error shape is "the service's own error code for the exception class — the member a client branches on beside the shape name", and it applies that rule through single constructors for the two error classes it has constants for: every `InvalidRequestException` goes through `crate::state::invalid_request` (which sets `code: Some(INVALID_PARAMETER_CODE)`) and every `ResourceNotFoundException` through `policies_not_found` / `policy_not_found_by_arn` (which set `code: Some(RESOURCE_NOT_FOUND_CODE)`).

The two remaining error shapes this service answers are built inline in the handlers with `code: None`:

* `LimitExceededException` — `CreateLifecyclePolicy`'s store-bound refusal fills `Message` and `ResourceType` but leaves `Code` unset, even though the shape declares `Message`, `Code` and `ResourceType` and the sibling `ResourceNotFoundException` constructor's doc states the rule as "Every member the shape declares is reported: the `Code` for the exception class, the `ResourceType` token of the resource that was not found, ...". The same construction therefore honours the rule for `ResourceType` and breaks it for `Code`.
* `InternalServerException` — the "store is unavailable" branch of all eight handlers is written the same way (`code: None`), so the only 500 this service can answer carries no `Code` either.

Consequence: a client that branches on `$.Code` (which is exactly how this crate justifies populating the member for the other two classes) gets `null` for the 429 and the 500, while it gets a code for the 400 and the 404 — one rule, applied to two of the four error classes the service answers. No test pins either payload.

## Evidence

The rule, stated in this crate — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:40-50`:
```
/// The `Code` every `InvalidRequestException` this service answers carries.
///
/// `InvalidRequestException$Code` is the service's own error code for the
/// exception class — the member a client branches on beside the shape name —
/// and Amazon Data Lifecycle Manager answers `InvalidParameter` for a request
/// whose parameters it refuses.
pub const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The `Code` every `ResourceNotFoundException` this service answers carries,
/// by the same rule as [`INVALID_PARAMETER_CODE`].
pub const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";
```
and the "report every declared member" wording on the sibling constructor — `src/state.rs:912-927`:
```
/// The `ResourceNotFoundException` payload for policy ids that name no policy.
///
/// Every member the shape declares is reported: the `Code` for the exception
/// class, the `ResourceType` token of the resource that was not found, the ids
/// that named none, and a message naming the resource asked for.
pub fn policies_not_found(ids: &[String]) -> ResourceNotFoundException {
    ...
        code: Some(RESOURCE_NOT_FOUND_CODE.to_string()),
        resource_type: Some(POLICY_RESOURCE_TYPE.to_string()),
```
The side that breaks it — `src/ops/create_lifecycle_policy.rs:358-370`:
```
        if store.policy_count() >= crate::state::POLICY_QUOTA {
            return Err(CreateLifecyclePolicyErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "this account already holds the maximum of {} lifecycle policies",
                        crate::state::POLICY_QUOTA
                    )),
                    code: None,
                    resource_type: Some(crate::state::POLICY_RESOURCE_TYPE.to_string()),
                },
            )
            .into());
        }
```
and `src/ops/create_lifecycle_policy.rs:187-195` (identical in the other seven handlers):
```
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateLifecyclePolicyErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the dlm lifecycle-policy store is unavailable".to_string()),
                    code: None,
                },
            )
            .into());
        };
```
The model declares the member on both shapes — `smithy_query(query="shape", shape="LimitExceededException")`: members `Message`, `Code` (`ErrorCode`), `ResourceType`; `smithy_query(query="shape", shape="InternalServerException")`: members `Message`, `Code`.

Test coverage: nothing pins either payload — a grep for `LimitExceeded|InternalServer` under `services/service-com-amazonaws-dlm-dlm-20180112/tests/` matches only two prose comments listing the modelled error set (`tests/all/unresolvable_policy_id.rs:20`, `tests/all/error_paths.rs:779`), never an assertion on a 429 or 500 body. The `Code` assertions that do exist are for the 400 and 404 only (e.g. `tests/all/default_policy_resource_type.rs:43`: `const INVALID_PARAMETER_CODE: &str = "InvalidParameter";`).

## Steps to Reproduce

1. Read src/state.rs:40-50 and src/state.rs:912-927: the crate states that the `Code` member names the exception class and is "the member a client branches on", and reports "every member the shape declares" for ResourceNotFoundException.
2. Read src/ops/create_lifecycle_policy.rs:358-370: the LimitExceededException the quota branch answers sets `resource_type` but `code: None`.
3. Read src/ops/create_lifecycle_policy.rs:187-195 (and the same block in the other seven handlers): the InternalServerException also sets `code: None`.
4. Drive the quota branch (create POLICY_QUOTA = 1000 policies via POST /policies, then one more) and observe the 429 body carries `ResourceType` and `Message` but no `Code`, while a 400 from POST /policies with an invalid Description carries `Code: "InvalidParameter"` and a 404 from GET /policies/policy-0000000000000999 carries `Code: "ResourceNotFound"`.
