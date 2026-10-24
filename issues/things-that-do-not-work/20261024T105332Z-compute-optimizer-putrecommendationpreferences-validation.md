Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: a32383ef1b0ca99666925af11391d8042f80bab1

# Refuse an enhancedInfrastructureMetrics preference for a resource type other than Ec2Instance or AutoScalingGroup

_Filed: 2026-10-24T10:53:32Z_
_Operation: PutRecommendationPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `a32383ef1b0ca99666925af11391d8042f80bab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: real Compute Optimizer refuses `PutRecommendationPreferences` when `enhancedInfrastructureMetrics` is set for a `resourceType` that does not support enhanced infrastructure metrics. Recorded AWS behaviour for `resourceType=EcsService, enhancedInfrastructureMetrics=Active` is HTTP 400 with `__type com.amazonaws.computeoptimizer#InvalidParameterValueException` and the message 'The specified resource type is not valid for "EnhancedInfrastructureMetrics" preference. Specify "Ec2Instance" or "AutoScalingGroup" in your request and try again.' (message text quoted only as evidence of which types are legal — the requirement is the 400 + InvalidParameterValueException), and nothing is persisted, so the subsequent `GetRecommendationPreferences` read-back reports no record.

Actual: the handler accepts it with HTTP 200 and an empty body for every one of the ten `ResourceType` enum values (AuroraDBClusterStorage, AutoScalingGroup, EbsVolume, Ec2Instance, EcsService, Idle, LambdaFunction, License, NotApplicable, RdsDBInstance), persists the preference under the (resourceType, scope) key, and `GetRecommendationPreferences` then reports a record a real account could never hold. `GetEffectiveRecommendationPreferences` will likewise resolve a preference AWS never created.

This is a regression of a previously-closed divergence: the restriction is absent from the handler's resource-type restriction table, whose comment states that `enhancedInfrastructureMetrics` 'documents no restriction' and so is 'accepted for every resource type'. The same reasoning is applied to `inferredWorkloadTypes`, which in the real service is likewise an EC2-instance / Auto Scaling group feature, so that member is the sibling symptom of the same gap.

## Evidence

Spec/behaviour grounding: (a) AWS recording quoted verbatim in the already-closed issue 20261022T170557Z-compute-optimizer-omits-response-fields-and-skips-preference.md, Cause 3 of 4: `PutRecommendationPreferences(resourceType=EcsService, enhancedInfrastructureMetrics=Active)` → HTTP 400 InvalidParameterValueException naming `Ec2Instance` / `AutoScalingGroup`, with Cause 4 of the same issue recording that nothing is stored for the refused request; (b) well-established service semantics — enhanced infrastructure metrics (3-month look-back) is documented for Amazon EC2 instances and Auto Scaling groups only; (c) the model declares InvalidParameterValueException (HTTP 400) for this operation.

Handler: `src/ops/put_recommendation_preferences.rs` — `request_rejection()` restriction table (lines ~328-365) carries only `externalMetricsPreference`, `lookBackPeriod`, `preferredResources` and `utilizationPreferences`; the comment above it (lines ~320-327) explicitly exempts `enhancedInfrastructureMetrics`, `inferredWorkloadTypes` and `savingsEstimationMode` from any resource-type check.

Test coverage status: the current (over-permissive) behaviour is actively asserted by `tests/all/unrestricted_preference_resource_types.rs` (`enhanced_metrics_is_settable_for_every_resource_type`, header lines 3-13) and by the acceptance rows in `tests/all/reported_defaults_and_preference_validation.rs` lines 286-335 ('enhanced metrics on EcsService' / 'EbsVolume' / 'LambdaFunction' / 'Idle' expected accepted). Those assertions contradict the AWS recording and must be inverted as part of the fix.

## Steps to Reproduce

1. Boot the emulator and opt the account in: `UpdateEnrollmentStatus(status="Active", includeMemberAccounts=false)`.
2. Call `PutRecommendationPreferences` with `{"resourceType": "EcsService", "scope": {"name": "AccountId", "value": "<caller account id>"}, "enhancedInfrastructureMetrics": "Active"}`.
3. Observe HTTP 200 with an empty body; real AWS answers HTTP 400 with error code `InvalidParameterValueException` naming Ec2Instance / AutoScalingGroup as the only valid resource types.
4. Call `GetRecommendationPreferences(resourceType="EcsService", scope=same)` and observe a persisted `recommendationPreferencesDetails` entry carrying `enhancedInfrastructureMetrics`, where real AWS reports none because the write was refused.
5. Repeat for `resourceType` values EbsVolume, LambdaFunction, Idle, License, NotApplicable, RdsDBInstance and AuroraDBClusterStorage — all are accepted today.
