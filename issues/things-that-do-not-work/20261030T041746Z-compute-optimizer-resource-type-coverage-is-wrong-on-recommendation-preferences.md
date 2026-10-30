Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-recommendation-preferences-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-recommendation-preferences-error-paths compute-optimizer-recommendation-summaries-and-export-job-listing
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|PutRecommendationPreferences|4xx->2xx|$
Issue-Key: compute-optimizer|PutRecommendationPreferences|4xx->2xx|status
Issue-Key: compute-optimizer|GetRecommendationPreferences|2xx->2xx|$.recommendationPreferencesDetails[*]
Issue-Key: compute-optimizer|GetRecommendationSummaries|2xx->2xx|$.recommendationSummaries[*]
Issue-Commit: 67cb0015417b8708a2fc0fa3f9bd7e09f0d5f565

# Resource-type coverage is wrong on recommendation preferences and recommendation summaries

> _Note: this issue was identified against the code base at commit `67cb0015417b8708a2fc0fa3f9bd7e09f0d5f565`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecommendationPreferences

## Description
THREE causes, one shared fault: the compute-optimizer crate does not model WHICH resource types a given recommendation preference / recommendation summary row applies to. On the write path it accepts a preference for a resource type that preference does not support (and then serves it back on the read path); on the summaries path it emits summary rows for only a subset of the resource types AWS reports and never emits the storage resource types at all. All three causes are in the same crate and all three are net-new: no open issue carries a replay key for any of them.

=== Cause 1 of 3: PutRecommendationPreferences accepts EnhancedInfrastructureMetrics for an unsupported resource type ===
AWS: Smithy (services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): PutRecommendationPreferences input = {resourceType (ResourceType enum, REQUIRED), scope (Scope), enhancedInfrastructureMetrics (EnhancedInfrastructureMetrics = Active|Inactive), inferredWorkloadTypes, externalMetricsPreference, lookBackPeriod, utilizationPreferences, preferredResources, savingsEstimationMode}; output is an EMPTY structure; modeled error InvalidParameterValueException (client, httpError 400). ResourceType enumerates Ec2Instance, AutoScalingGroup, EbsVolume, LambdaFunction, NotApplicable, EcsService, License, RdsDBInstance, AuroraDBClusterStorage, Idle — i.e. the member accepts resource types that the EnhancedInfrastructureMetrics preference itself does not support, so the restriction is enforced by the service, not by the model. Recorded behavior: a request carrying enhancedInfrastructureMetrics together with a resourceType outside {Ec2Instance, AutoScalingGroup} is REFUSED HTTP 400 with __type com.amazonaws.computeoptimizer#InvalidParameterValueException and message 'The specified resource type is not valid for "EnhancedInfrastructureMetrics" preference. Specify "Ec2Instance" or "AutoScalingGroup" in your request and try again.' In that sentence pair every word is FIXED text for this preference, including the double-quoted tokens; the only part that varies across the preference family is the quoted preference name (here "EnhancedInfrastructureMetrics") — the offending resourceType value from the request is NOT echoed and the member name resourceType is not mentioned.
Emulator: HTTP 200 with an empty JSON body ({}) — the refusal is never raised, the request is accepted and the preference is written to state. Nothing in the response tells the caller the preference it asked for is not supported for that resource type.
Why: a client that relies on AWS refusing the combination believes it has activated enhanced infrastructure metrics for a resource type where that preference cannot exist, and gets no error to handle; worse, the accepted write corrupts the account's preference store (see Cause 2), so a later GetRecommendationPreferences reports a preference AWS would never have. A test suite asserting the documented 400/InvalidParameterValueException sees a success.
Fix: the PutRecommendationPreferences handler's input validation in the compute-optimizer crate — the preference/resource-type compatibility rule (which resource types each preference member is accepted for) must be checked before the preference is written, and must raise InvalidParameterValueException with AWS's fixed sentence for the preference named in the request. This sits next to the existing scope/resourceType check that already refuses AutoScalingGroup at Organization/AccountId scope.
Traces: compute-optimizer-recommendation-preferences-error-paths#task#3

=== Cause 2 of 3: GetRecommendationPreferences serves back the preference that should never have been stored ===
AWS: Smithy: GetRecommendationPreferences input = {resourceType (REQUIRED), scope, nextToken, maxResults (range 0..1000)}, output = {nextToken, recommendationPreferencesDetails (list)}; paginated on recommendationPreferencesDetails. Recorded behavior: because the Put of Cause 1 is refused, no preference exists for that resource type and the read returns HTTP 200 with recommendationPreferencesDetails EMPTY. The read path only ever reports preferences that were accepted by a write.
Emulator: HTTP 200 but recommendationPreferencesDetails has one extra element describing the preference the emulator wrongly accepted — a detail whose scope is the account-level scope from the request, whose resourceType is the unsupported resource type from the request, and whose enhancedInfrastructureMetrics is the value from the request.
Why: the account's preference state diverges from AWS for the rest of the session. Any client that enumerates preferences (console preference pages, drift/compliance tooling, an SDK paginator) sees a preference that cannot exist on AWS, and code that branches on the presence of a preference for a resource type takes the wrong branch. It also makes Cause 1 non-idempotent to clean up: there is no AWS-side delete that would ever have been needed.
Fix: no separate fix in the read handler is expected — it faithfully returns state — so this is the state-level consequence of Cause 1: once the write validation refuses the unsupported combination, the store holds no such preference. A fixer MUST re-replay this step too, because a Put that is refused but still mutates state would leave this step failing.
Traces: compute-optimizer-recommendation-preferences-error-paths#task#4

=== Cause 3 of 3: GetRecommendationSummaries omits the storage resource-type summary rows, so every row past the sixth is misaligned ===
AWS: Smithy: GetRecommendationSummaries input = {accountIds, nextToken, maxResults 0..1000}, output = {nextToken, recommendationSummaries (list of RecommendationSummary)}. RecommendationSummary.recommendationResourceType targets RecommendationSourceType, which enumerates Ec2Instance, AutoScalingGroup, EbsVolume, LambdaFunction, EcsService, License, RdsDBInstance, RdsDBInstanceStorage, AuroraDBClusterStorage, NatGateway, DynamoDBTable, ElastiCacheCluster, MemoryDBCluster, DocumentDBCluster, WorkSpaces and SageMakerEndpoint — a strictly larger set than ResourceType, and notably including the two storage types. Recorded behavior: an enrolled account with no analysed resources gets one summary row PER reported resource type, each with accountId and recommendationResourceType set and its findings counts at 0. The rows are not uniform: a storage row (recommendationResourceType RdsDBInstanceStorage, AuroraDBClusterStorage) carries ONLY accountId, recommendationResourceType and summaries, whose finding names are that type's own finding vocabulary (Optimized/Overprovisioned/Underprovisioned for RdsDBInstanceStorage; Optimized/NotOptimized for AuroraDBClusterStorage), with NO reasonCodeSummaries, no idleSummaries and none of the savingsOpportunity / idleSavingsOpportunity / aggregatedSavingsOpportunity / currentPerformanceRiskRatings blocks that a compute row carries.
Emulator: HTTP 200, but the list it serves covers only a subset of RecommendationSourceType and contains NO storage rows at all. From the seventh element onward the emulator's list no longer lines up with AWS's: where AWS has the RdsDBInstanceStorage row the emulator has a compute-shaped RdsDBInstance row (full Underprovisioned/Overprovisioned/Optimized summaries with reasonCodeSummaries, idleSummaries, all three savingsOpportunity blocks and currentPerformanceRiskRatings), and the rows AWS reports at the following indices are missing entirely.
Why: a client that reads the summary list by resource type — a cost dashboard, an exporter, or anything that asserts coverage of the resource types it manages — silently gets no row for the storage types and therefore no totals for them, and a client that indexes the list positionally reads one resource type's counts as another's. Because the differing values are all zero in an empty account, the fault is purely which rows exist and what shape each row has, not the numbers.
Fix: the GetRecommendationSummaries handler/serializer in the compute-optimizer crate — build one row per reported RecommendationSourceType value (including RdsDBInstanceStorage and AuroraDBClusterStorage), and emit the storage rows in their reduced shape with their own finding vocabulary rather than reusing the compute-row template.
Traces: compute-optimizer-recommendation-summaries-and-export-job-listing#task#3

Relation to open issues: the open wire issue 20261029T183426Z-compute-optimizer-refusal-messages-are-emulator-authored-instead-of-aws.md concerns PutRecommendationPreferences MESSAGE WORDING on the AutoScalingGroup-scope refusal and names compute-optimizer-recommendation-preferences-error-paths#task#2; it does not cover Cause 1 here, which is a refusal that never happens at all (200 instead of 400) on a different rule, and the replay reports this cluster as unowned. 20261029T205102Z-compute-optimizer-getrdsdatabaserecommendations-serves-a-hardcoded-orders-primary-recommendation-that.md concerns GetRDSDatabaseRecommendations content, not the summary row set of Cause 3.

## Evidence
All three clusters replayed against a running emulator (emulator_up=true, /__admin/health 200); corpus simparity-wire-aws-v1, 70 steps, 57 passed, 13 mismatched. These three clusters carried no stub_path and no owned_by.

Cause 1 — PutRecommendationPreferences [compute-optimizer-recommendation-preferences-error-paths#task#3], expected 400, got 200:
  status: expected 400, got 200
  $: empty_body
    expected {'__type': 'com.amazonaws.computeoptimizer#InvalidParameterValueException', 'message': 'The specified resource type is not valid for "EnhancedInfrastructureMetrics" preference. Specify "Ec2Instance" or "AutoScalingGroup" in your request and try again.'}
    actual   {}
  Fixed text of the message: the whole of both sentences, including the quoted "Ec2Instance"/"AutoScalingGroup" tokens. Variable part: the quoted preference name ("EnhancedInfrastructureMetrics"), taken from which preference member the request set. Nothing else in the message comes from the request.

Cause 2 — GetRecommendationPreferences [compute-optimizer-recommendation-preferences-error-paths#task#4], expected 200, got 200:
  $.recommendationPreferencesDetails[0]: extra
    expected None (AWS: the list is empty, because the Put was refused)
    actual   {'scope': {'name': 'AccountId', 'value': '<caller account id>'}, 'resourceType': '<the unsupported resourceType from the Put>', 'enhancedInfrastructureMetrics': '<the value from the Put>'}
  (the recorded run shows scope value 000000000000, resourceType EcsService, enhancedInfrastructureMetrics Active — those are this run's request values, not part of the requirement)

Cause 3 — GetRecommendationSummaries [compute-optimizer-recommendation-summaries-and-export-job-listing#task#3], expected 200, got 200; diverging keys $.recommendationSummaries[6], [7], [8], [10], [11], [12], [13], [14], [15]:
  $.recommendationSummaries[6]: missing
    expected {'accountId': <caller account id>, 'recommendationResourceType': 'RdsDBInstanceStorage', 'summaries': [{'name': 'Optimized', 'value': 0.0}, {'name': 'Overprovisioned', 'value': 0.0}, {'name': 'Underprovisioned', 'value': 0.0}]}
    actual   a compute-shaped 'RdsDBInstance' row (summaries with reasonCodeSummaries MemoryOverprovisioned/MemoryUnderprovisioned, idleSummaries Idle/Unattached/Unused, savingsOpportunity + idleSavingsOpportunity + aggregatedSavingsOpportunity, currentPerformanceRiskRatings)
  $.recommendationSummaries[7]: missing
    expected {'accountId': <caller account id>, 'recommendationResourceType': 'AuroraDBClusterStorage', 'summaries': [{'name': 'Optimized', 'value': 0.0}, {'name': 'NotOptimized', 'value': 0.0}]}
    actual   another compute-shaped row
  and the same offset at [8] and [10]..[15]: the emulator never emits a storage row, so from index 6 on the two lists describe different resource types. All counts on both sides are 0.0 (empty account), so only row membership and row shape diverge.

Model evidence (smithy_query, services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): PutRecommendationPreferences declares InvalidParameterValueException (client, 400) and an empty output; GetRecommendationPreferences output = {nextToken, recommendationPreferencesDetails}; RecommendationSummary.recommendationResourceType -> RecommendationSourceType whose values include RdsDBInstanceStorage and AuroraDBClusterStorage; ResourceType (the Put member) does NOT restrict which preference may be set for which type, so the preference/resource-type rule is service behavior and the recorded refusal is the requirement.

## Steps to Reproduce
Boot the emulator and opt the account in (UpdateEnrollmentStatus status=Active).

Cause 1: call PutRecommendationPreferences(resourceType="EcsService", scope={"name":"AccountId","value":<caller account id>}, enhancedInfrastructureMetrics="Active"). AWS refuses it HTTP 400 InvalidParameterValueException with 'The specified resource type is not valid for "EnhancedInfrastructureMetrics" preference. Specify "Ec2Instance" or "AutoScalingGroup" in your request and try again.'; the emulator answers HTTP 200 with an empty body. The same call with resourceType="Ec2Instance" is accepted on both, confirming the write path itself works.

Cause 2: immediately after the above, call GetRecommendationPreferences(resourceType="EcsService", scope={"name":"AccountId","value":<caller account id>}). AWS returns recommendationPreferencesDetails = []; the emulator returns one detail echoing the resourceType, scope and enhancedInfrastructureMetrics of the refused Put.

Cause 3: call GetRecommendationSummaries() on the enrolled account with no analysed resources. Collect the recommendationResourceType of every row: AWS includes a row for RdsDBInstanceStorage and one for AuroraDBClusterStorage, each carrying only accountId, recommendationResourceType and a summaries list in that type's finding vocabulary; the emulator's list contains no storage row, so from the seventh row on it reports different resource types than AWS and omits the ones AWS reports last.

Replay: re-run fixture compute-optimizer-recommendation-preferences-error-paths steps task#3 (representative) and task#4, and fixture compute-optimizer-recommendation-summaries-and-export-job-listing step task#3.
