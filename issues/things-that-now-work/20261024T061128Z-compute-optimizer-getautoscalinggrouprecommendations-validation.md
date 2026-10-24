Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Refuse an accountIds value naming another account instead of answering 200 with an empty list

_Filed: 2026-10-24T06:11:28Z_
_Operation: GetAutoScalingGroupRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `accountIds` is how an organization MANAGEMENT account names a member account ("If your account is the management account of an organization, use this parameter to specify the member account for which you want to return Auto Scaling group recommendations"). This emulator presents one standalone account and records no organization, so a well-formed 12-digit id that is not the caller's own account is a request the service cannot answer and is refused with `InvalidParameterValueException` (HTTP 400) — the same constant refusal the sibling readers in this very crate already emit ("Invalid permission to access another account. If this is an organization account, request the management account to enable service access.").

Actual: the handler validates only the COUNT (>1) and the 12-digit SHAPE of the id, then uses it as a filter key: `analyses.iter().filter(|group| group.account_id == account_id)`. A foreign account id therefore yields HTTP 200 with `autoScalingGroupRecommendations: []` and `errors: []`, so a client that mistakenly asks about another account is told "that account has no recommendations" rather than "you may not ask". Worse, if the analysis store ever holds a row for another account, the filter would REPORT another account's recommendations to the caller.

## Evidence

Spec: `GetAutoScalingGroupRecommendations$accountIds` documents the member as the management-account mechanism for naming a member account; the operation models `InvalidParameterValueException` (HTTP 400). In-crate recorded behaviour: `src/ops/get_ebs_volume_recommendations.rs` `foreign_account()` / `CROSS_ACCOUNT_MESSAGE` and `src/ops/get_lambda_function_recommendations.rs` `foreign_account()` both refuse exactly this case and document the message as "the real service's constant refusal". Handler: `src/ops/get_auto_scaling_group_recommendations.rs` lines ~200-217 (count + `crate::state::is_account_id` shape check only) and lines ~301-321, where the id becomes a filter (`unwrap_or_else(|| engine::emulator_account_id())`). Tests: `tests/all/auto_scaling_group_recommendations.rs` has no case supplying a foreign account id, so nothing asserts the current acceptance.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active) and wait for the analysis to become due.
2. Call GetAutoScalingGroupRecommendations with accountIds=['210987654321'] (a well-formed id that is not the caller's).
3. Observe HTTP 200 with an empty autoScalingGroupRecommendations list and empty errors, where AWS refuses with InvalidParameterValueException (400).
4. For contrast, issue the same accountIds value to GetEBSVolumeRecommendations or GetLambdaFunctionRecommendations on the same emulator and observe the 400 refusal those handlers already implement.
