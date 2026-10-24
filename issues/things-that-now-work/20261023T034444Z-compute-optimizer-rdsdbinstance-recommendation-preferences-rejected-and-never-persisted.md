Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-recommendation-preferences-rds-crud
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-recommendation-preferences-rds-crud
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|PutRecommendationPreferences|2xx->4xx|$
Issue-Key: compute-optimizer|PutRecommendationPreferences|2xx->4xx|status
Issue-Key: compute-optimizer|GetRecommendationPreferences|2xx->2xx|$.recommendationPreferencesDetails[*]
Issue-Commit: d66da8d621774a402898e98aa83287c30e78f3fd

# RdsDBInstance recommendation preferences rejected and never persisted

> _Note: this issue was identified against the code base at commit `d66da8d621774a402898e98aa83287c30e78f3fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecommendationPreferences

## Description
The emulator's recommendation-preference support treats the `enhancedInfrastructureMetrics` preference as valid only for a hard-coded subset of resource types (Ec2Instance, AutoScalingGroup). As a result an account-scoped preference for `RdsDBInstance` is refused at write time and, because nothing is written, is also absent from the subsequent read. Two causes, both on compute-optimizer.

Cause 1 of 2: PutRecommendationPreferences rejects resourceType=RdsDBInstance with an InvalidParameterValueException instead of accepting it.
  AWS: PutRecommendationPreferences takes a required `resourceType` whose enum (Smithy: ResourceType) admits Ec2Instance, AutoScalingGroup, EbsVolume, LambdaFunction, NotApplicable, EcsService, License, RdsDBInstance, AuroraDBClusterStorage, Idle. Neither the operation's input shape nor the `enhancedInfrastructureMetrics` member (Smithy: PutRecommendationPreferencesRequest.enhancedInfrastructureMetrics -> EnhancedInfrastructureMetrics, values Active/Inactive, not required, no per-resource-type constraint) restricts which resource types may carry that preference, and the only client error modeled for a bad member value is InvalidParameterValueException (HTTP 400). The recording shows AWS accepting the request: HTTP 200 with an empty body (Smithy: PutRecommendationPreferencesResponse has no members).
  Emulator: returns HTTP 400 with `__type` InvalidParameterValueException, message e.g. "The value supplied for parameter resourceType is not valid: the RdsDBInstance resource type does not support the enhancedInfrastructureMetrics preference. Specify one of Ec2Instance, AutoScalingGroup in your request and try again." (message quoted as an example; the divergence is the status and the error itself, which AWS does not raise at all here).
  Why: a client configuring preferences for RDS DB instances — a resource type Compute Optimizer supports and the model enumerates — gets a hard 400 for a well-formed request. SDK calls raise InvalidParameterValueException, so setup/onboarding code that enables enhanced infrastructure metrics across resource types aborts, and any code path that only ever exercises RDS preferences can never succeed.
  Fix: the resource-type/preference compatibility validation in the PutRecommendationPreferences handler of the compute-optimizer service crate. The allowlist used to gate `enhancedInfrastructureMetrics` must not exclude resource types the ResourceType enum admits; on acceptance the handler must persist the preference and return an empty 200 body.
  Traces: compute-optimizer-recommendation-preferences-rds-crud#task#2 (also task#4).

Cause 2 of 2: GetRecommendationPreferences returns an empty `recommendationPreferencesDetails` list after a preference was put for RdsDBInstance.
  AWS: GetRecommendationPreferences returns `recommendationPreferencesDetails`, a list of RecommendationPreferencesDetail (Smithy: GetRecommendationPreferencesResponse.recommendationPreferencesDetails -> RecommendationPreferencesDetails; detail members scope, resourceType, enhancedInfrastructureMetrics, lookBackPeriod, ...), filtered by the required `resourceType` and the optional `scope`. The recording shows the read-back of the preference written in cause 1: one detail element carrying the stored resourceType, enhancedInfrastructureMetrics, lookBackPeriod and the account-level scope ({name: AccountId, value: <12-digit account id>}); status 200 in both cases.
  Emulator: answers 200 but with no element at `$.recommendationPreferencesDetails[0]` — the list is empty, so a preference written for this resource type and scope is not readable.
  Why: the put/get round-trip for recommendation preferences does not hold. A client that writes a preference and then reads it back to confirm, or that lists current preferences to decide whether to enable one, sees it as never configured and either loops re-applying it or reports the feature as off.
  Fix: the preference store backing the compute-optimizer service crate and the GetRecommendationPreferences handler that reads it — the entry written by PutRecommendationPreferences must be indexed by resourceType and scope and returned as a RecommendationPreferencesDetail with the members that were set. Note this cause is downstream of cause 1 for RdsDBInstance specifically, so verify persistence and read-back independently once cause 1 accepts the write; do not assume fixing the allowlist alone closes it.
  Traces: compute-optimizer-recommendation-preferences-rds-crud#task#3 (also task#5).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; state_reset_failures=0, so neither cluster is state-bleed suspect).

Cause 1 — fixture compute-optimizer-recommendation-preferences-rds-crud, step task#2 (cluster also covers task#4), operation PutRecommendationPreferences:
  expected: status 200, body {}
  actual:   status 400, body {"__type": "InvalidParameterValueException", "message": "The value supplied for parameter resourceType is not valid: the RdsDBInstance resource type does not support the enhancedInfrastructureMetrics preference. Specify one of Ec2Instance, AutoScalingGroup in your request and try again."}
  divergence_key: service=compute-optimizer|PutRecommendationPreferences|status=200->400|keys=$,status

Cause 2 — fixture compute-optimizer-recommendation-preferences-rds-crud, step task#3 (cluster also covers task#5), operation GetRecommendationPreferences:
  expected status 200 with $.recommendationPreferencesDetails[0] = {"enhancedInfrastructureMetrics": "Inactive", "lookBackPeriod": "DAYS_14", "resourceType": "RdsDBInstance", "scope": {"name": "AccountId", "value": "<account id>"}}
  actual:   status 200, $.recommendationPreferencesDetails[0] missing (empty list)
  divergence_key: service=compute-optimizer|GetRecommendationPreferences|status=200->200|keys=$.recommendationPreferencesDetails[0]

Model evidence: Smithy ResourceType enum includes RdsDBInstance; PutRecommendationPreferencesRequest.enhancedInfrastructureMetrics carries no resource-type restriction; PutRecommendationPreferencesResponse is empty; GetRecommendationPreferences is paginated over recommendationPreferencesDetails.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture compute-optimizer-recommendation-preferences-rds-crud step task#2 (repeats at task#4) — call PutRecommendationPreferences with resourceType=RdsDBInstance, an account-level scope ({name: AccountId, value: <12-digit account id>}), enhancedInfrastructureMetrics=Inactive and lookBackPeriod=DAYS_14. Expect HTTP 200 with an empty JSON body; observe HTTP 400 InvalidParameterValueException.
3. Cause 2: replay the same fixture's step task#3 (repeats at task#5) — call GetRecommendationPreferences with resourceType=RdsDBInstance and the same account-level scope. Expect HTTP 200 whose recommendationPreferencesDetails contains one detail echoing the preference written in step 2; observe HTTP 200 with an empty recommendationPreferencesDetails list.
4. Both causes must pass before closing: re-replay fixture#step compute-optimizer-recommendation-preferences-rds-crud#task#2 and compute-optimizer-recommendation-preferences-rds-crud#task#3.
