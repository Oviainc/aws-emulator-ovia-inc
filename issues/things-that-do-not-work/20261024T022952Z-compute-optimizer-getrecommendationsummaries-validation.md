Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# Refuse a malformed accountIds value with InvalidParameterValueException

_Filed: 2026-10-24T02:29:52Z_
_Operation: GetRecommendationSummaries | Class: validation_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRecommendationSummaries` accepts any string in `accountIds` and answers HTTP 200 with an empty `recommendationSummaries` list. Only the list LENGTH is checked ("only one account ID can be specified per request"); the value itself is never validated, so `accountIds: ["not-an-account"]`, `["123"]` or `[""]` are treated as "an account whose findings this emulator does not hold" instead of a bad request.

Expected: a value that is not a 12-digit Amazon Web Services account ID is refused with `InvalidParameterValueException` (HTTP 400, "The value supplied for the input parameter is out of range or not valid") — which is exactly what every other findings reader in this same crate does for the same member.
Actual: HTTP 200 with an empty list, so a caller that typoed an account id reads "this account has no recommendations" rather than being told the parameter is invalid.

The model gives `AccountId` no `@pattern`/`@length`, so the request boundary cannot refuse it — the check has to live in the handler, as it does in the siblings.

## Evidence

Spec: `GetRecommendationSummaries$accountIds` is "The ID of the Amazon Web Services account for which to return recommendation summaries" and the operation models `InvalidParameterValueException` (HTTP 400); `AccountId` is a plain string in the model (`smithy_query shape=AccountId` — no constraints), so no boundary validation applies. Handler: `src/ops/get_recommendation_summaries.rs` lines 202-222 check only `requested_accounts.len() > 1` and then use `requested_accounts.first()` as-is. Siblings that DO validate the same member with `crate::state::is_account_id`: `src/ops/get_idle_recommendations.rs` lines 204-212, `src/ops/get_rds_database_recommendations.rs` lines 216-224, `src/ops/get_auto_scaling_group_recommendations.rs` lines 209-217, `src/ops/get_ebs_volume_recommendations.rs` `requested_account_id()` and `src/ops/get_lambda_function_recommendations.rs` `requested_account_id()`. Tests: `tests/all/recommendation_summaries.rs::account_ids_selects_one_account_and_refuses_more_than_one` covers absent/empty/own/another WELL-FORMED id and the >1 refusal — no case supplies a malformed id, so nothing asserts the current acceptance.

## Steps to Reproduce

1. Boot the compute-optimizer emulator and opt the account in with UpdateEnrollmentStatus(status=Active)
2. Call GetIdleRecommendations with accountIds=["not-an-account"] — observe HTTP 400 InvalidParameterValueException
3. Call GetRecommendationSummaries with accountIds=["not-an-account"] (also try ["123"] and [""]) — observe HTTP 200 with recommendationSummaries: [] instead of the 400
