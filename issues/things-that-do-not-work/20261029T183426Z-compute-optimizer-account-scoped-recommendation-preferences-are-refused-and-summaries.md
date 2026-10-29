Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-recommendation-preferences-rds-crud
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-recommendation-preferences-rds-crud compute-optimizer-recommendation-summaries-and-export-job-listing
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|PutRecommendationPreferences|2xx->4xx|$
Issue-Key: compute-optimizer|PutRecommendationPreferences|2xx->4xx|status
Issue-Key: compute-optimizer|GetRecommendationPreferences|2xx->2xx|$.recommendationPreferencesDetails[*]
Issue-Key: compute-optimizer|GetRecommendationSummaries|2xx->2xx|$.recommendationSummaries[*]
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Account-scoped recommendation preferences are refused and summaries come back empty, so an enrolled account has no readable preference or findings surface

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecommendationPreferences

## Description
THREE causes. The account-level read surface of this service is unusable: a preference write that AWS accepts is refused (Cause 1), so the preference read that should report it returns nothing (Cause 2), and the findings roll-up returns an empty list where AWS returns one summary per tracked resource type (Cause 3).

=== Cause 1 of 3: PutRecommendationPreferences refuses an enhancedInfrastructureMetrics preference for resourceType=RdsDBInstance that AWS accepts ===
AWS: Smithy: PutRecommendationPreferences input {resourceType (ResourceType enum, required), scope (Scope), enhancedInfrastructureMetrics (EnhancedInfrastructureMetrics enum), inferredWorkloadTypes, externalMetricsPreference, lookBackPeriod, utilizationPreferences, preferredResources, savingsEstimationMode}; output is an EMPTY structure; modeled error InvalidParameterValueException (client, 400). The model places NO resource-type restriction on enhancedInfrastructureMetrics: its documentation says only "The status of the enhanced infrastructure metrics recommendation preference to create or update. Specify the Active status to activate the preference, or specify Inactive to deactivate the preference." The resourceType documentation likewise says only "The target resource type of the recommendation preference to create." Recorded behavior: PutRecommendationPreferences with resourceType=RdsDBInstance, an AccountId scope, enhancedInfrastructureMetrics=Inactive and lookBackPeriod=DAYS_14 is ACCEPTED — HTTP 200 with an empty body `{}` — and the preference is stored.
Emulator: HTTP 400 with body {"__type": "InvalidParameterValueException", "message": "The value supplied for parameter resourceType is not valid: the RdsDBInstance resource type does not support the enhancedInfrastructureMetrics preference. Specify one of Ec2Instance, AutoScalingGroup in your request and try again."} — a blanket refusal of the member for any resource type outside Ec2Instance/AutoScalingGroup, applied regardless of the value being set. Hypothesis: the gate was introduced by the already-FIXED issue 20261024T105332Z-compute-optimizer-putrecommendationpreferences-validation.md ("Refuse an enhancedInfrastructureMetrics preference for a resource type other than Ec2Instance or AutoScalingGroup"); the recording shows that rule is at minimum too broad — deactivating the preference (Inactive) on another resource type is accepted by the service — so the gate must be narrowed, not kept, and its message is in any case the generic "value supplied for parameter" template rather than any sentence AWS emits.
Why: the write is the documented way to set an account's lookBackPeriod and other preferences for RDS recommendations. A client that issues one Put per resource type with its full preference payload is hard-failed on the RdsDBInstance iteration, so none of that account's RDS preferences are ever established and every downstream RDS recommendation is computed under defaults.
Fix: the PutRecommendationPreferences handler's resourceType/preference compatibility validation in the compute-optimizer crate (the enhancedInfrastructureMetrics gate added for the resolved validation issue above) — it must not refuse the combination the recording shows accepted, and the refusal it does raise must not use the generic parameter-template message.
Traces: compute-optimizer-recommendation-preferences-rds-crud#task#2 (the cluster also covers #task#4, the update/idempotent re-write of the same preference)

=== Cause 2 of 3: GetRecommendationPreferences reports no preference detail for an account-scoped RdsDBInstance preference ===
AWS: Smithy: GetRecommendationPreferences input {resourceType (required), scope, nextToken, maxResults}; output {nextToken, recommendationPreferencesDetails: RecommendationPreferencesDetails}; paginated(items=recommendationPreferencesDetails). Recorded behavior: after the preference of Cause 1 is written, GetRecommendationPreferences(resourceType=RdsDBInstance, scope={name: AccountId, value: <caller account>}) is answered HTTP 200 with one detail entry reporting the stored preference: {enhancedInfrastructureMetrics: 'Inactive', lookBackPeriod: 'DAYS_14', resourceType: 'RdsDBInstance', scope: {name: 'AccountId', value: <caller account id>}} — i.e. the read echoes back exactly the preference members that were set, plus the scope it was set at.
Emulator: HTTP 200 (status matches) but `recommendationPreferencesDetails` has no entry at all — the expected detail is reported missing.
Whey-not: this follows from Cause 1 on this fixture (the write was refused, so there is nothing to read), but it is a distinct observable divergence and must be verified separately: the read must report an account-scoped preference, with its scope echoed, once the write is accepted.
Why: a client reads its preferences back to render or reconcile them (and to decide whether to issue an update). Against the emulator the round-trip loses the preference entirely, so reconciliation loops re-write forever and any UI shows the account as unconfigured.
Fix: the GetRecommendationPreferences handler and the preference store it reads in the compute-optimizer crate — an account-scoped preference record must be selectable by (resourceType, scope) and rendered as a RecommendationPreferencesDetail carrying the members that were set. Verify after Cause 1 is fixed.
Traces: compute-optimizer-recommendation-preferences-rds-crud#task#3 (the cluster also covers #task#5, the read after the second write)

=== Cause 3 of 3: GetRecommendationSummaries answers an empty recommendationSummaries list for an enrolled account ===
AWS: Smithy: GetRecommendationSummaries output {nextToken, recommendationSummaries: RecommendationSummaries}; each RecommendationSummary carries accountId, recommendationResourceType (RecommendationSourceType enum) and summaries (a count per finding, where Summary$value legitimately is 0); the operation is documented as returning the account's optimization findings per resource type. Recorded behavior: an enrolled account is answered HTTP 200 with one summary entry PER TRACKED RESOURCE TYPE even when nothing has been analyzed — 16 entries in the recording, e.g. {accountId: <caller account>, recommendationResourceType: 'Ec2Instance', summaries: [{name: 'OPTIMIZED', value: 0.0}, {name: 'OVER_PROVISIONED', value: 0.0}, {name: 'UNDER_PROVISIONED', value: 0.0}], idleSummaries: [{name: 'Idle', value: 0.0}, {name: 'Unattached', value: 0.0}, {name: 'Unused', value: 0.0}]}, and per-type finding vocabularies for AutoScalingGroup, EbsVolume, LambdaFunction (with reasonCodeSummaries), EcsService and the remaining tracked types. The zero counts, not the absence of the entry, are how AWS represents "nothing analyzed yet".
Emulator: HTTP 200 with `recommendationSummaries` empty — every expected entry is reported missing.
Why: this is the entry point for reading an account's optimization posture; callers iterate it to pick resource types to drill into and to render a row per type. Against the emulator that loop body never runs, so a client cannot distinguish "enrolled, nothing to optimize yet" from "no data surface at all", and anything that indexes the first summary or derives the tracked resource-type set from the response fails or silently reports nothing.
Fix: the GetRecommendationSummaries handler and the state view it reads in the compute-optimizer crate — the summary set must be enumerated from the tracked RecommendationSourceType vocabulary for the enrolled account (emitting zero-count summaries and the per-type idleSummaries/reasonCodeSummaries vocabulary) rather than from whichever catalog rows happen to be present and visible.
Traces: compute-optimizer-recommendation-summaries-and-export-job-listing#task#3

Relation to open issues: Cause 3 is the same root cause as the open behavioral issue 20261027T115927Z-compute-optimizer-getrecommendationsummaries-returns-an-empty-recommendationsummaries-list-for-an.md, which carries no wire replay key for this corpus (the replay reported this cluster unowned); it is filed here with its fixture binding so the wire step is actually re-run, and a fixer closing either should re-replay the step named above. The resolved issues 20261024T022952Z-compute-optimizer-getrecommendationsummaries-wrong-behavior.md (content of an entry that IS emitted) and 20261024T105332Z-compute-optimizer-putrecommendationpreferences-validation.md (the over-broad gate in Cause 1) do not cover these causes.

## Evidence
All three clusters replayed against a running emulator (emulator_up=true), so the service answers; these are fidelity divergences.

Cause 1 — PutRecommendationPreferences [compute-optimizer-recommendation-preferences-rds-crud#task#2; cluster also covers #task#4]:
  status: expected 200, got 400
  body: expected {}  (empty success structure)
  actual {'__type': 'InvalidParameterValueException', 'message': 'The value supplied for parameter resourceType is not valid: the RdsDBInstance resource type does not support the enhancedInfrastructureMetrics preference. Specify one of Ec2Instance, AutoScalingGroup in your request and try again.'}
  Model: PutRecommendationPreferences output is an empty structure; enhancedInfrastructureMetrics doc states only Active/Inactive semantics and names no resource-type restriction; resourceType doc names none either.

Cause 2 — GetRecommendationPreferences [compute-optimizer-recommendation-preferences-rds-crud#task#3; cluster also covers #task#5], expected 200, got 200:
  $.recommendationPreferencesDetails[0]: missing
    expected {'enhancedInfrastructureMetrics': 'Inactive', 'lookBackPeriod': 'DAYS_14', 'resourceType': 'RdsDBInstance', 'scope': {'name': 'AccountId', 'value': '000000000000'}}
    actual   None
  (the account id 000000000000 is the recording environment's caller account — the requirement is that the scope value echoes the CALLER's account, not that literal)

Cause 3 — GetRecommendationSummaries [compute-optimizer-recommendation-summaries-and-export-job-listing#task#3], expected 200, got 200:
  $.recommendationSummaries[0..15]: all 16 entries missing (expected present, actual None), e.g.
    [0] {'accountId': <caller account>, 'recommendationResourceType': 'Ec2Instance', 'summaries': [{'name': 'OPTIMIZED', 'value': 0.0}, {'name': 'OVER_PROVISIONED', 'value': 0.0}, {'name': 'UNDER_PROVISIONED', 'value': 0.0}], 'idleSummaries': [{'name': 'Idle', 'value': 0.0}, {'name': 'Unattached', 'value': 0.0}, {'name': 'Unused', 'value': 0.0}]}
    [1] recommendationResourceType 'AutoScalingGroup' with summaries OPTIMIZED/NOT_OPTIMIZED
    [2] 'EbsVolume' with Optimized/NotOptimized
    [3] 'LambdaFunction' with Optimized/NotOptimized + reasonCodeSummaries MemoryOverprovisioned/MemoryUnderprovisioned
    [4] 'EcsService' with Optimized/Overprovisioned/Underprovisioned
  All counts are 0.0, i.e. AWS emits the entry with zero counts; the emulator emits no entry.

Model evidence (smithy_query on services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/smithy-model/com-amazonaws-computeoptimizer-computeoptimizerservice.json): GetRecommendationPreferences output {nextToken, recommendationPreferencesDetails}, paginated on recommendationPreferencesDetails; PutRecommendationPreferences output {} with InvalidParameterValueException(400) modeled; RecommendationPreferenceName enum = EnhancedInfrastructureMetrics | InferredWorkloadTypes | ExternalMetricsPreference | LookBackPeriodPreference | PreferredResources | UtilizationPreferences (no resource-type coupling).

## Steps to Reproduce
Boot the emulator and opt the account in (UpdateEnrollmentStatus status=Active; confirm GetEnrollmentStatus reports Active).

Cause 1: call PutRecommendationPreferences(resourceType="RdsDBInstance", scope={"name": "AccountId", "value": <caller account id>}, enhancedInfrastructureMetrics="Inactive", lookBackPeriod="DAYS_14"). The emulator refuses 400 InvalidParameterValueException with the resourceType/enhancedInfrastructureMetrics message; AWS answers HTTP 200 with an empty body and stores the preference. Repeat the same call to exercise the update path (cluster member #task#4).

Cause 2: after the Put above, call GetRecommendationPreferences(resourceType="RdsDBInstance", scope={"name": "AccountId", "value": <caller account id>}). The emulator answers 200 with recommendationPreferencesDetails empty; AWS returns one detail echoing enhancedInfrastructureMetrics='Inactive', lookBackPeriod='DAYS_14', resourceType='RdsDBInstance' and the AccountId scope. Re-check after the second Put (cluster member #task#5).

Cause 3: call GetRecommendationSummaries with no accountIds, nextToken or maxResults. The emulator answers 200 with recommendationSummaries empty; AWS returns one summary per tracked RecommendationSourceType for the caller's account, each with zero-valued counts and the per-type finding vocabulary (plus idleSummaries, and reasonCodeSummaries for LambdaFunction).

Replay: re-run fixture compute-optimizer-recommendation-preferences-rds-crud step task#2 (representative) plus the two bindings in trace_ids.
