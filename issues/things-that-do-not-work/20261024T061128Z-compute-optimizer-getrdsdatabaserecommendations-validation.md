Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Refuse an accountIds value naming another account instead of using it as a catalog filter

_Filed: 2026-10-24T06:11:28Z_
_Operation: GetRDSDatabaseRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a well-formed 12-digit `accountIds` value that is not the caller's own account is refused with `InvalidParameterValueException` (HTTP 400) — this emulator presents one standalone account and records no organization, so there is no member account it can be asked about. This is the behaviour the EBS and Lambda readers of this crate already implement from the service's recorded cross-account refusal.

Actual: the handler validates the count and the 12-digit shape, then uses the id purely as a selection filter over the findings catalog: `.filter(|resource| account_ids.is_empty() || account_ids.contains(&resource.account_id))`. A foreign id is answered HTTP 200 (empty `rdsDBRecommendations`, empty `errors`) — and where the catalog does hold a row owned by another account, the request RETURNS that account's database recommendation to the caller rather than refusing the request, which the crate's own test fixture demonstrates is a reachable state (`tests/all/rds_database_recommendations.rs` seeds a `MEMBER_ACCOUNT` database into the same catalog).

## Evidence

Spec: `GetRDSDatabaseRecommendations$accountIds` — "you can only specify one account ID per request", the management-account mechanism for naming a member account; the operation models `InvalidParameterValueException` (HTTP 400). In-crate recorded behaviour: `src/ops/get_ebs_volume_recommendations.rs` `foreign_account()` / `CROSS_ACCOUNT_MESSAGE`, documented there as the real service's constant refusal, and the same helper in `src/ops/get_lambda_function_recommendations.rs`. Handler: `src/ops/get_rds_database_recommendations.rs` lines ~207-224 (count + `crate::state::is_account_id`) and the `account_ids.contains(&resource.account_id)` filter at lines ~323. Tests: `tests/all/rds_database_recommendations.rs` lines ~704-713 pass `accountIds: [ACCOUNT]` (the caller's own) with another account's ARN; no case supplies a foreign accountIds value, so nothing asserts the current acceptance.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active).
2. Call GetRDSDatabaseRecommendations with accountIds=['210987654321'] (well-formed, not the caller's account).
3. Observe HTTP 200 with empty rdsDBRecommendations, where AWS refuses with InvalidParameterValueException (400).
4. Record an analysed RDS row owned by that other account into the findings catalog, repeat the call, and observe the caller is handed another account's recommendation instead of a refusal.
