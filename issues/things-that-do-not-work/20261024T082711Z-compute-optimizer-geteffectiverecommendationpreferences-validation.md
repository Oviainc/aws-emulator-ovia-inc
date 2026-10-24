Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Refuse a resourceArn naming another account's resource instead of resolving preferences for it

_Filed: 2026-10-24T08:27:11Z_
_Operation: GetEffectiveRecommendationPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a `resourceArn` whose account field is not the caller's account is refused with `InvalidParameterValueException` (HTTP 400) — the cross-account refusal this crate already records from AWS for its other readers ("Invalid permission to access another account. If this is an organization account, request the management account to enable service access.") — because this emulator presents one standalone account with no organization, so no other account's resource is one it may answer about.

Actual: the handler only checks that the ARN parses as an EC2 instance or Auto Scaling group ARN (`crate::state::parse_resource_arn`, which returns the ARN's account id but never compares it with `engine::emulator_account_id()`), then resolves and returns effective preferences with HTTP 200. Worse, the resolution keys account-level precedence off the ARN's OWN account id (`effective_preferences(&input.resource_arn, resource)` → `account_scope_precedence`), so an `AccountId`-scoped preference written for a foreign account is reported back to the caller as the effective preference of a resource in that foreign account. The region field is likewise unchecked, so an ARN of another Region is answered by this regional endpoint.

## Evidence

Spec: `GetEffectiveRecommendationPreferences$resourceArn` — "The Amazon Resource Name (ARN) of the resource for which to return recommendation preferences... Only EC2 instance and Auto Scaling group ARNs are currently supported"; the operation models `InvalidParameterValueException` (HTTP 400). In-crate recorded AWS behaviour for the same condition: `src/ops/get_ebs_volume_recommendations.rs` `CROSS_ACCOUNT_MESSAGE`/`foreign_account()` and the identical helper in `src/ops/get_lambda_function_recommendations.rs`. Handler: `src/ops/get_effective_recommendation_preferences.rs` lines ~215-257. State: `src/state.rs` `parse_resource_arn` (~line 472) performs no account check; `account_scope_precedence` (~line 546) matches an `AccountId` scope against the ARN's account id. Tests: `tests/all/enrollment_and_preferences.rs` and `tests/all/reported_defaults_and_preference_validation.rs` only ever pass ARNs built by the harness for the caller's own account, so no case pins a foreign-account ARN.

## Steps to Reproduce

1. Boot the emulator; call UpdateEnrollmentStatus(status=Active).
2. Call GetEffectiveRecommendationPreferences with resourceArn=arn:aws:ec2:us-east-1:210987654321:instance/i-0123456789abcdef0 (a well-formed instance ARN of another account).
3. Observe HTTP 200 carrying effective preferences, where AWS refuses the cross-account request with InvalidParameterValueException (400).
4. Optionally first call PutRecommendationPreferences with scope={name:AccountId,value:210987654321} and lookBackPeriod=DAYS_93, then repeat step 2: the foreign account's stored preference is reported as effective for the foreign resource.
5. For contrast, send accountIds=['210987654321'] to GetEBSVolumeRecommendations and observe the 400 cross-account refusal.
