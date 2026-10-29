Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-configured-table-analysis-rule-round-trip
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-configured-table-analysis-rule-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|GetConfiguredTableAnalysisRule|2xx->2xx|$.analysisRule.policy.v1.list.allowedJoinOperators
Issue-Key: cleanrooms|GetConfiguredTableAnalysisRule|4xx->2xx|$.message
Issue-Key: cleanrooms|GetConfiguredTableAnalysisRule|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cleanrooms|GetConfiguredTableAnalysisRule|4xx->2xx|status
Issue-Commit: a1be48b455f77208774c140f08575194e8ab84c8

# cleanrooms configured-table analysis-rule reads diverge: allowedJoinOperators dropped and an absent rule type answered 200

> _Note: this issue was identified against the code base at commit `a1be48b455f77208774c140f08575194e8ab84c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfiguredTableAnalysisRule

## Description
Two net-new root causes on the cleanrooms configured-table analysis-rule plane. Both are reached through GetConfiguredTableAnalysisRule after a rule has been created with CreateConfiguredTableAnalysisRule. 2 causes must be closed before this issue is submitted.

Cause 1 of 2: GetConfiguredTableAnalysisRule drops the LIST policy's allowedJoinOperators member.
AWS: Smithy: GetConfiguredTableAnalysisRule output member analysisRule -> ConfiguredTableAnalysisRule.policy -> ConfiguredTableAnalysisRulePolicyV1 union member `list` -> AnalysisRuleList.allowedJoinOperators, target JoinOperatorsList (@length max 2, member JoinOperator with values OR | AND), documented as "The logical operators (if any) that are to be used in an INNER JOIN match condition. Default is AND." A read-back of a rule created with allowedJoinOperators set echoes exactly the join operators that were stored, in the order they were submitted; the member is only absent when the creator never set it.
Emulator: the GET answers HTTP 200 with the analysis rule, but $.analysisRule.policy.v1.list.allowedJoinOperators is absent from the body, although the rule was created in the same fixture with a non-empty JoinOperatorsList (the recording's value is ['OR'] — the requirement is "whatever the create stored", not that literal list).
Why: a client that reads the analysis rule back to render or re-submit it sees no allowedJoinOperators and concludes the rule falls back to the documented default AND, inverting the join semantics the collaboration actually enforces; a read-modify-write round trip silently discards the operator the data owner configured.
Fix: the CreateConfiguredTableAnalysisRule/GetConfiguredTableAnalysisRule state for the AnalysisRuleList policy (the optional member is not persisted) and the output serializer for ConfiguredTableAnalysisRulePolicyV1.list, which must emit allowedJoinOperators whenever it is present in state.
Traces: cleanrooms-configured-table-analysis-rule-round-trip#task#7.

Cause 2 of 2: an analysis-rule type that does not exist on the configured table is answered 200 with the stored rule instead of 404 ResourceNotFoundException.
AWS: GetConfiguredTableAnalysisRule binds analysisRuleType as an @httpLabel and models ResourceNotFoundException (httpError 404). Recorded behavior: a GET for a rule type the configured table has no rule for is refused 404 — and the recording shows this applies to a label value that is not even a modeled ConfiguredTableAnalysisRuleType member (the modeled enum is AGGREGATION | LIST | CUSTOM), so the type is resolved as part of the lookup key rather than pre-validated into a 400. The error message is the fixed template "No such analysis rule with configuredTableId: <configuredTableId> and type <analysisRuleType>": the fixed text is "No such analysis rule with configuredTableId: " and " and type ", and the two variable parts are the configuredTableIdentifier and the analysisRuleType taken verbatim from the request path (in the recording, configuredTableId 1abbe375-99ee-42ea-af31-f80edb4d9054 and type INTERSECTION_LIST — per-run values that fill the template, not part of it). The reply also carries the restJson1 x-amzn-errortype header identifying ResourceNotFoundException, and the modeled shape additionally requires resourceId and resourceType.
Emulator: answers HTTP 200 with an analysis-rule body. Replay reports "status: expected 404, got 200", "$.message: missing (expected 'No such analysis rule with configuredTableId: ... and type INTERSECTION_LIST', got None)" and "$headers.x-amzn-errortype: missing". The handler evidently keys stored analysis rules by configured table only and ignores the analysisRuleType label, so any type returns whichever rule the table has.
Why: a client probing which analysis-rule types a configured table carries is told every type exists, and receives a rule whose `type` does not match the type it asked for; code that branches on the 404 to decide between create and update will take the update path and overwrite an unrelated rule, and no client-side error handling for the modeled ResourceNotFoundException is ever exercised.
Fix: the GetConfiguredTableAnalysisRule handler's lookup (key the rule by (configuredTableId, analysisRuleType) and raise the modeled 404 on a miss) plus the shared cleanrooms error mapper so the body carries AWS's message template and the x-amzn-errortype header.
Traces: cleanrooms-configured-table-analysis-rule-round-trip#task#9.

## Evidence
Replay of fixture cleanrooms-configured-table-analysis-rule-round-trip (trace set simparity-wire-aws-v1), emulator up (/__admin/health 200):

Step task#7 GetConfiguredTableAnalysisRule — expected 200 / actual 200, body mismatch: "$.analysisRule.policy.v1.list.allowedJoinOperators: missing (expected ['OR'], got None)".

Step task#9 GetConfiguredTableAnalysisRule — expected 404 / actual 200: "status: expected 404, got 200"; "$.message: missing (expected 'No such analysis rule with configuredTableId: 1abbe375-99ee-42ea-af31-f80edb4d9054 and type INTERSECTION_LIST', got None)"; "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)".

Model evidence: Smithy AnalysisRuleList.allowedJoinOperators -> JoinOperatorsList (@length max 2; JoinOperator = OR|AND), optional, default AND when absent. GetConfiguredTableAnalysisRule errors include ResourceNotFoundException (404) whose shape requires message, resourceId and resourceType.

## Steps to Reproduce
1. Replay fixture cleanrooms-configured-table-analysis-rule-round-trip from trace set simparity-wire-aws-v1 against the emulator.
2. Cause 1 — step task#7: create a configured-table LIST analysis rule whose analysisRulePolicy.v1.list sets allowedJoinOperators (e.g. ["OR"]), then GET /configuredTables/{configuredTableIdentifier}/analysisRule/LIST. Observe 200 whose $.analysisRule.policy.v1.list has no allowedJoinOperators member.
3. Cause 2 — step task#9: GET /configuredTables/{configuredTableIdentifier}/analysisRule/{type} for an analysis-rule type the table has no rule for. Observe 200 with the other rule's body instead of 404 ResourceNotFoundException carrying message "No such analysis rule with configuredTableId: <id> and type <type>" and the x-amzn-errortype header.
