Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.get-insight-impact-graph-unknown-id-fails
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.get-insight-impact-graph-unknown-id-fails|run#14|AssertionError|assert well_formed_code == "InvalidRequestException", (
Issue-Commit: edda120f5b31e63091be2c3166cb69bdc8220f1a

# GetInsightImpactGraph is an unimplemented scaffold stub: a well-formed but never-issued InsightId answers NotImplemented instead of 400 InvalidRequestException "Insight not found" (1 cause)

> _Note: this issue was identified against the code base at commit `edda120f5b31e63091be2c3166cb69bdc8220f1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInsightImpactGraph

## Description
One net-new root cause in the X-Ray service (the check declares SERVICES = ['xray']). Service `xray` is bootstrapped and answering (92/127 xray checks pass in this run), so this is a fidelity defect, not a bootstrap gap.

No open or resolved xray issue names this operation. The closest neighbours cover DIFFERENT operations and do not cover this one: open 20261025T032230Z-xray-x-ray-query-and-data-plane-fidelity-four.md Cause 1 covers `GetInsightEvents`; resolved 20261022T013648Z-xray-x-ray-operations-answer-notimplemented-where-a-modeled.md Cause 2 covers `GetInsight`; resolved 20261022T013648Z-xray-getinsightsummaries-stub.md covers `GetInsightSummaries`. None claimed `GetInsightImpactGraph`, so this is not a reopen.

Note on why the replay report did not mark this check ALREADY FILED: the check wraps the call in its own `_error()` helper, which swallows the ClientError and re-raises a plain AssertionError, so the scaffold's "scaffolded but not implemented" phrase never reaches the report and the stub is not auto-detected for this operation.

Cause 1 of 1: GetInsightImpactGraph is never implemented, so every request that satisfies the model's input constraints is refused with the emulator's own non-AWS `NotImplemented` error instead of the operation's modeled refusal.

AWS: Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json) models GetInsightImpactGraph with http POST /InsightImpactGraph, success status 200, required input members InsightId (shape InsightId, @pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$), StartTime and EndTime (Timestamp), optional NextToken (Token, @length 1..2000); output InsightId/StartTime/EndTime/ServiceGraphStartTime/ServiceGraphEndTime/Services (InsightImpactGraphServiceList)/NextToken; and exactly two modeled errors, InvalidRequestException (client, httpError 400 — documented "The request is missing required parameters or has invalid parameters") and ThrottledException (client, httpError 429). An InsightId that SATISFIES that pattern but names no insight ever issued for the account is refused with InvalidRequestException at HTTP 400; the error message is the fixed text "Insight not found" (recorded behavior for the message text — it has no variable parts, it interpolates neither the submitted InsightId nor the time window). A request whose InsightId VIOLATES the pattern is a separate, earlier refusal: ValidationException with AWS's standard constraint-violation template, "1 validation error detected: Value '<the submitted value>' at 'insightId' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modeled pattern>" — fixed text apart from the submitted value and the pattern. The emulator already gets that pattern case right (the check's malformed-id assertion passes, because modeled constraints are enforced before the handler runs); only the well-formed-but-unknown path diverges.

Emulator: the call is refused with error code `NotImplemented`. services/service-com-amazonaws-xray-awsxray/src/ops/get_insight_impact_graph.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker, and its `handle()` body binds and discards its arguments (`let _ = (&input, ctx, &self.engine);`) before returning `Err(crate::scaffolded::Scaffolded::not_implemented("GetInsightImpactGraph"))`. So there is no unknown-insight refusal and no success path: a genuine insight id would be refused identically.

Why: `NotImplemented` is not an error the X-Ray model declares for this (or any) operation, so no SDK error-typed catch can match it — botocore surfaces it as a bare ClientError, and a client's `except xray.exceptions.InvalidRequestException` block misses entirely. A client walking an insight's impact graph therefore cannot distinguish "this insight does not exist / my id is stale" from "the service is broken", gets a status outside the operation's modeled error set, and can never retrieve the structural service graph for a real insight at all — making the whole Insights impact-graph surface unreachable even though GetInsightSummaries (its documented way to obtain an InsightId) is implemented and answering.

Fix: implement the handler in services/service-com-amazonaws-xray-awsxray/src/ops/get_insight_impact_graph.rs — look the InsightId up in the service's insight state and return `Err(GetInsightImpactGraphErrorModeled::InvalidRequestException(..).into())` with the message "Insight not found" when it names no insight, otherwise build the GetInsightImpactGraphOutput (echoing InsightId/StartTime/EndTime and deriving ServiceGraphStartTime/ServiceGraphEndTime/Services from the stored segment-and-service index) — and delete the SCAFFOLDED marker line at the top of that file so the stub inventory stops counting it.

Traces: xray.get-insight-impact-graph-unknown-id-fails.

## Evidence
Cause 1: failing check xray.get-insight-impact-graph-unknown-id-fails [mismatch].

The check first establishes positive controls that PASS: CreateGroup with InsightsConfiguration={InsightsEnabled: True, NotificationsEnabled: False} succeeds, and GetInsightSummaries(StartTime, EndTime, GroupName, States=["ACTIVE"]) returns an InsightSummaries list — so Insights is reachable and enabled, and the failure below is specific to GetInsightImpactGraph.

It then passes the pattern-violation leg: get_insight_impact_graph(InsightId="11111111-2222-3333-4444-555555555555", StartTime, EndTime) is correctly refused with ValidationException and the message "1 validation error detected: Value '11111111-2222-3333-4444-555555555555' at 'insightId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}".

It fails on the well-formed leg:
  check_xray_get_insight_impact_graph_unknown_id_fails.py, line 73, in run
  assert well_formed_code == "InvalidRequestException", (...)
  AssertionError: expected InvalidRequestException for a well-formed but never-issued InsightId, got NotImplemented

Call made: get_insight_impact_graph(InsightId="aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", StartTime=now-3600, EndTime=now). That id satisfies the modeled pattern (version nibble 4, variant nibble 8).
Expected: HTTP 400, error code InvalidRequestException, message "Insight not found" (the check also asserts that exact message at line 79, which is never reached).
Actual: error code NotImplemented (the emulator's scaffold refusal, HTTP 501), from the `Scaffolded::not_implemented("GetInsightImpactGraph")` return at services/service-com-amazonaws-xray-awsxray/src/ops/get_insight_impact_graph.rs:177, with the file's SCAFFOLDED marker still present at line 4.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite builds and boots it); point a us-west-2 `xray` client at it.
2. Create an insights-enabled group as the positive control: xray.create_group(GroupName="<name<=32 chars>", FilterExpression='service("<name>")', InsightsConfiguration={"InsightsEnabled": True, "NotificationsEnabled": False}) — succeeds.
3. Confirm Insights is reachable: xray.get_insight_summaries(StartTime=now-3600, EndTime=now, GroupName="<name>", States=["ACTIVE"]) — returns an InsightSummaries list.
4. Control that already behaves correctly: xray.get_insight_impact_graph(InsightId="11111111-2222-3333-4444-555555555555", StartTime=now-3600, EndTime=now) — observe ValidationException with the constraint-violation message naming 'insightId' and the modeled pattern.
5. The divergence: xray.get_insight_impact_graph(InsightId="aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", StartTime=now-3600, EndTime=now) — observe error code NotImplemented (HTTP 501) instead of HTTP 400 InvalidRequestException with message "Insight not found".
6. Confirm the cause in the checkout: services/service-com-amazonaws-xray-awsxray/src/ops/get_insight_impact_graph.rs line 4 still carries `//! SCAFFOLDED, NOT IMPLEMENTED` and line 177 returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetInsightImpactGraph"))`.
7. Re-replay xray.get-insight-impact-graph-unknown-id-fails to confirm the fix.
