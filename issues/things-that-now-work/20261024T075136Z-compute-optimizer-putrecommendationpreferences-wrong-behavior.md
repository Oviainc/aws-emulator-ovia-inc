Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# A PutRecommendationPreferences request that sets no preference creates a phantom preference record

_Filed: 2026-10-24T07:51:36Z_
_Operation: PutRecommendationPreferences | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The upsert inserts a record for the (resourceType, scope) key BEFORE checking whether the request carries any preference at all: `preferences.entry(key).or_insert_with(PreferenceRecord::for_key).apply(&input)`. `apply` only copies members the request supplied, so a request carrying just `resourceType` (and optionally `scope`) leaves an all-empty record behind.

Expected: a call that sets no preference leaves nothing for a later read to report — `GetRecommendationPreferences` for that resource type returns no detail, and `GetEffectiveRecommendationPreferences` resolves to the next scope / documented defaults.

Actual: the empty record persists and `GetRecommendationPreferences` reports a phantom `RecommendationPreferencesDetail` carrying only `resourceType` (and `scope`) with every preference member absent — a preference that was never set. The phantom record also consumes one of the `MAX_PREFERENCE_RECORDS` (1000) slots the store refuses new keys past with 503, so repeated no-op calls at distinct `ResourceArn` scopes can exhaust the store without ever storing a preference. Note the inverse rule is already implemented on the delete side: `DeleteRecommendationPreferences` removes a record once `holds_any_preference()` is false, precisely so an empty record is not left addressable.

## Evidence

Spec: `PutRecommendationPreferences` is documented as "Creates a new recommendation preference or updates an existing recommendation preference" — a request that names no preference creates none, and `GetRecommendationPreferences` returns "the recommendation preferences that are in effect", which an all-empty record is not. Handler: `src/ops/put_recommendation_preferences.rs` lines 261-270 (`can_hold` then `entry(..).or_insert_with(..).apply(&input)`); `src/state.rs` `PreferenceRecord::apply` touches only `Some`/non-empty members, `PreferenceRecord::to_detail` emits a detail for any stored record, and `holds_any_preference()` exists but is consulted only by `delete_recommendation_preferences.rs` lines 233-235. Test coverage: no test in `tests/all/enrollment_and_preferences.rs` or `tests/all/reported_defaults_and_preference_validation.rs` issues a Put with no preference member — every fixture sets at least one.

## Steps to Reproduce

1. POST UpdateEnrollmentStatus {"status":"Active","includeMemberAccounts":false} -> 200
2. POST PutRecommendationPreferences {"resourceType":"EbsVolume"} -> 200 (empty body)
3. POST GetRecommendationPreferences {"resourceType":"EbsVolume"} -> actual 200 with one recommendationPreferencesDetails entry {"resourceType":"EbsVolume"} and no preference members; expected an empty list because nothing was set
4. Repeat the no-preference Put at 1000 distinct {"name":"ResourceArn","value":<distinct EC2 instance ARN>} scopes for resourceType Ec2Instance, then Put a real preference at a new scope -> 503 ServiceUnavailableException, the store filled with records holding no preference
