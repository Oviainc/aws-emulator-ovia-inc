Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Refuse an accountIds value naming another account instead of using it as a selection filter

_Filed: 2026-10-24T06:11:28Z_
_Operation: GetIdleRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a well-formed 12-digit `accountIds` value naming an account other than the caller's is refused with `InvalidParameterValueException` (HTTP 400), since `accountIds` is the organization-management-account mechanism for naming a member account and this emulator presents one standalone account with no organization. The EBS and Lambda readers of this crate already answer exactly that refusal.

Actual: the handler checks the count (>1) and the 12-digit shape, then filters: `account_ids.is_empty() || recommendation.account_id ... account_ids.contains(..)`. A foreign id is answered HTTP 200 with an empty `idleRecommendations` list. As with the sibling readers, the filter is a selection, not an authorisation check: any idle recommendation recorded for another account would be reported to the caller rather than refused.

## Evidence

Spec: `GetIdleRecommendations$accountIds` — "You can only specify one account ID per request" — is the management-account mechanism; the operation models `InvalidParameterValueException` (HTTP 400). In-crate recorded behaviour: `src/ops/get_ebs_volume_recommendations.rs` `foreign_account()` / `CROSS_ACCOUNT_MESSAGE` and the identical helper in `src/ops/get_lambda_function_recommendations.rs`. Handler: `src/ops/get_idle_recommendations.rs` lines ~195-212 (count + `crate::state::is_account_id`) and the account filter at lines ~301-307. Tests: `tests/all/idle_recommendations.rs` lines ~379-388 pass `accountIds: [ACCOUNT]` (the caller's own) together with another account's resource ARN; no case supplies a foreign accountIds value. Distinct from the open issue 20261024T022952Z-compute-optimizer-getidlerecommendations-wrong-behavior.md, which is about the empty idle store rather than the rejection of a cross-account request.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active).
2. Call GetIdleRecommendations with accountIds=['210987654321'] (well-formed, not the caller's account).
3. Observe HTTP 200 with an empty idleRecommendations list, where AWS refuses with InvalidParameterValueException (400).
4. For contrast, send the same accountIds value to GetEBSVolumeRecommendations and observe the 400 refusal.
