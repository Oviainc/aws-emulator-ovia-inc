Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Refuse an accountIds value naming another account instead of silently narrowing the result set

_Filed: 2026-10-24T06:11:28Z_
_Operation: GetECSServiceRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a well-formed 12-digit `accountIds` value that is not the caller's own account names a member account of an organization this emulator does not have, so the request is refused with `InvalidParameterValueException` (HTTP 400), as the EBS and Lambda readers of this same crate already do with the service's recorded cross-account message.

Actual: the handler checks only that at most one id was supplied and that it has the 12-digit shape, then uses it to NARROW the candidate list (`candidates.retain(|c| accounts.is_empty() || accounts.contains(&c.reference.account_id))`). A foreign id is answered HTTP 200 with `ecsServiceRecommendations: []` and `errors: []`. Because the candidate account id comes from the ECS service ARN, the same code path would also hand a caller another account's ECS recommendations if the ECS inventory held a service owned by that account — the filter is not an authorisation check.

## Evidence

Spec: `GetECSServiceRecommendations$accountIds` — "Return the Amazon ECS service recommendations to the specified Amazon Web Services account IDs... You can only specify one account ID per request" — is the management-account mechanism; the operation models `InvalidParameterValueException` (HTTP 400). In-crate recorded behaviour: `src/ops/get_ebs_volume_recommendations.rs` `CROSS_ACCOUNT_MESSAGE` / `foreign_account()` and the identical helper in `src/ops/get_lambda_function_recommendations.rs`. Handler: `src/ops/get_ecs_service_recommendations.rs` lines ~358-375 (count + `crate::state::is_account_id` only) and the `candidates.retain(..)` at lines ~481-484. Tests: `tests/all/ecs_service_recommendations.rs` exercises a foreign-account service ARN but never a foreign `accountIds` value, so nothing asserts the current acceptance.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active).
2. Call GetECSServiceRecommendations with accountIds=['210987654321'] (well-formed, not the caller's account).
3. Observe HTTP 200 with empty ecsServiceRecommendations and empty errors, where AWS refuses with InvalidParameterValueException (400).
4. For contrast, send the same accountIds value to GetEBSVolumeRecommendations and observe the 400 refusal.
