Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-enrollment-status-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-enrollment-status-lifecycle compute-optimizer-recommendation-preferences-ec2-crud compute-optimizer-recommendation-preferences-error-paths
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|GetEnrollmentStatus|2xx->2xx|$.numberOfMemberAccountsOptedIn
Issue-Key: compute-optimizer|GetEffectiveRecommendationPreferences|2xx->2xx|$.preferredResources
Issue-Key: compute-optimizer|GetEffectiveRecommendationPreferences|2xx->2xx|$.utilizationPreferences
Issue-Key: compute-optimizer|PutRecommendationPreferences|4xx->2xx|$
Issue-Key: compute-optimizer|PutRecommendationPreferences|4xx->2xx|status
Issue-Key: compute-optimizer|GetRecommendationPreferences|2xx->2xx|$.recommendationPreferencesDetails[*]
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Compute Optimizer omits response fields and skips preference validation

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEnrollmentStatus

## Description
Wire replay of the compute-optimizer fixtures shows four distinct defects in the Compute Optimizer emulator: two response serializers drop modelled output members, and PutRecommendationPreferences performs no resource-type validation of the preference being set, which then corrupts the stored preference set read back by GetRecommendationPreferences. This issue bundles FOUR causes; all four must be fixed and re-replayed before it is closed.

Cause 1 of 4 - GetEnrollmentStatus omits numberOfMemberAccountsOptedIn (fixture compute-optimizer-enrollment-status-lifecycle, step task#3).
AWS: Smithy: GetEnrollmentStatusResponse.numberOfMemberAccountsOptedIn targets NumberOfMemberAccountsOptedIn (integer), documented as "The count of organization member accounts that are opted in to the service, if your account is an organization management account." The recording shows the member present on a 200 response with the value 0, i.e. AWS serializes the count even when it is zero / no members are enrolled.
Emulator: returns 200 but the response body has no numberOfMemberAccountsOptedIn key at all ($.numberOfMemberAccountsOptedIn: missing, expected 0, got None). The same omission recurs on the later poll in the same fixture (step task#5).
Why: a client that reads the opted-in member count unconditionally gets null/absent instead of a number, so SDK accessors return no value and callers cannot distinguish "zero member accounts opted in" from "field not reported"; organization-level enrollment dashboards and any arithmetic on the count break.
Fix: the GetEnrollmentStatus handler/serializer must always populate numberOfMemberAccountsOptedIn (defaulting to 0 when no member accounts are opted in) rather than skipping the member when it is zero or unset.
Traces: compute-optimizer-enrollment-status-lifecycle#task#3 (also task#5).

Cause 2 of 4 - GetEffectiveRecommendationPreferences omits preferredResources and utilizationPreferences (fixture compute-optimizer-recommendation-preferences-ec2-crud, step task#4).
AWS: Smithy: GetEffectiveRecommendationPreferencesResponse.preferredResources targets EffectivePreferredResources (list) and .utilizationPreferences targets UtilizationPreferences (list). The recording shows that after PutRecommendationPreferences stores preferredResources/utilizationPreferences for a resource, the effective-preferences read returns both lists, with preferredResources expanded into its effectiveIncludeList of concrete instance types.
Emulator: returns 200 with both members absent ($.preferredResources: missing; $.utilizationPreferences: missing), i.e. the preferences written by the preceding Put are not reflected in the effective-preferences projection.
Why: the effective-preferences call is how a client confirms which preferences actually apply to a resource; dropping these members makes a successfully stored preference look as though it was never applied, and consumers that iterate effectiveIncludeList see an empty/absent list.
Fix: the GetEffectiveRecommendationPreferences handler must project the stored recommendation preferences for the requested resourceArn into preferredResources (including the effectiveIncludeList expansion) and utilizationPreferences, and the response serializer must emit both list members.
Traces: compute-optimizer-recommendation-preferences-ec2-crud#task#4.

Cause 3 of 4 - PutRecommendationPreferences accepts an EnhancedInfrastructureMetrics preference for an unsupported resourceType instead of rejecting it (fixture compute-optimizer-recommendation-preferences-error-paths, step task#3).
AWS: Smithy: PutRecommendationPreferences takes required resourceType (ResourceType enum) plus enhancedInfrastructureMetrics, and models InvalidParameterValueException (client error, httpError 400). The recording shows AWS rejecting a request that sets enhancedInfrastructureMetrics for a resourceType that does not support it with HTTP 400 and __type com.amazonaws.computeoptimizer#InvalidParameterValueException; example message: "The specified resource type is not valid for \"EnhancedInfrastructureMetrics\" preference. Specify \"Ec2Instance\" or \"AutoScalingGroup\" in your request and try again." (message quoted as an example only - the requirement is the 400 status and the InvalidParameterValueException code).
Emulator: answers HTTP 200 with an empty body ({}), i.e. no validation of resourceType against the preference being set; the request is accepted as if valid.
Why: a client relying on AWS's contract gets a silent success for a request AWS refuses. Error-handling paths are never exercised, the client believes a preference is in force that AWS would never have created, and the invalid preference is persisted (see Cause 4).
Fix: PutRecommendationPreferences input validation must reject enhancedInfrastructureMetrics for resource types that do not support it, raising InvalidParameterValueException with HTTP 400 before any state mutation.
Traces: compute-optimizer-recommendation-preferences-error-paths#task#3.

Cause 4 of 4 - GetRecommendationPreferences returns a preference record that should never have been stored (fixture compute-optimizer-recommendation-preferences-error-paths, step task#4).
AWS: Smithy: GetRecommendationPreferencesResponse.recommendationPreferencesDetails targets RecommendationPreferencesDetails (list). The recording shows the list empty/absent after the rejected Put of Cause 3, because a request rejected with InvalidParameterValueException leaves no stored preference.
Emulator: returns 200 with an extra element: $.recommendationPreferencesDetails[0] = {"scope": {"name": "AccountId", "value": "..."}, "resourceType": "EcsService", "enhancedInfrastructureMetrics": "Active"} (expected None, got that object). The account id in the recorded value is environment-specific and is not itself a requirement; the defect is the presence of the record.
Why: the emulator's preference state diverges from AWS after a request AWS rejects - a client listing preferences sees a preference for a resource type that cannot carry it, so read-after-write assertions, preference reconciliation and cleanup logic all operate on state a real account would never hold.
Fix: once Cause 3's validation rejects the request, the PutRecommendationPreferences handler must not write to the recommendation-preference state; the state layer should only record preferences for validated (resourceType, preference) combinations so GetRecommendationPreferences reports nothing for the rejected request.
Traces: compute-optimizer-recommendation-preferences-error-paths#task#4.

Note: a fifth divergence in the same corpus (PutRecommendationPreferences, compute-optimizer-recommendation-preferences-error-paths#task#2) differs ONLY in the error message text - status 400 and InvalidParameterValueException match and the Auto Scaling group scope rule documented on PutRecommendationPreferencesRequest.scope is enforced correctly - so it is not filed as a defect here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 12 fixtures, 69 steps, 29 passed, score 42.0%, state_reset_failures 0, bootstrap_signal false).

Cause 1 - GetEnrollmentStatus, compute-optimizer-enrollment-status-lifecycle#task#3 (cluster also covers #task#5):
  expected status 200 / actual status 200
  detail: $.numberOfMemberAccountsOptedIn: missing (expected 0, got None)

Cause 2 - GetEffectiveRecommendationPreferences, compute-optimizer-recommendation-preferences-ec2-crud#task#4:
  expected status 200 / actual status 200
  detail: $.preferredResources: missing (expected a list of effectiveIncludeList entries of concrete instance types, got None); $.utilizationPreferences: missing

Cause 3 - PutRecommendationPreferences, compute-optimizer-recommendation-preferences-error-paths#task#3:
  expected status 400 / actual status 200
  detail: status: expected 400, got 200; $: empty_body (expected {'__type': 'com.amazonaws.computeoptimizer#InvalidParameterValueException', 'message': 'The specified resource type is not valid for "EnhancedInfrastructureMetrics" preference. Specify "Ec2Instance" or "AutoScalingGroup" in your request and try again.'}, got {})

Cause 4 - GetRecommendationPreferences, compute-optimizer-recommendation-preferences-error-paths#task#4:
  expected status 200 / actual status 200
  detail: $.recommendationPreferencesDetails[0]: extra (expected None, got {'scope': {'name': 'AccountId', 'value': '000000000000'}, 'resourceType': 'EcsService', 'enhancedInfrastructureMetrics': 'Active'})

Model evidence: Smithy GetEnrollmentStatusResponse.numberOfMemberAccountsOptedIn (integer); GetEffectiveRecommendationPreferencesResponse.preferredResources (EffectivePreferredResources list) and .utilizationPreferences (UtilizationPreferences list); PutRecommendationPreferences errors include InvalidParameterValueException (client, httpError 400); GetRecommendationPreferencesResponse.recommendationPreferencesDetails (list).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture compute-optimizer-enrollment-status-lifecycle and inspect step task#3 - call GetEnrollmentStatus and check the 200 body for numberOfMemberAccountsOptedIn; it is absent where AWS reports a count (0). Step task#5 of the same fixture shows it again.
3. Cause 2: replay fixture compute-optimizer-recommendation-preferences-ec2-crud through step task#4 - after the earlier PutRecommendationPreferences in that fixture stores preferredResources/utilizationPreferences, call GetEffectiveRecommendationPreferences for the same resourceArn; both members are missing from the 200 body.
4. Cause 3: replay fixture compute-optimizer-recommendation-preferences-error-paths step task#3 - call PutRecommendationPreferences with enhancedInfrastructureMetrics=Active for a resourceType that does not support it; the emulator answers 200 {} instead of 400 InvalidParameterValueException.
5. Cause 4: immediately after step 4, replay step task#4 of the same fixture - GetRecommendationPreferences returns a recommendationPreferencesDetails entry for the rejected request instead of reporting none.
