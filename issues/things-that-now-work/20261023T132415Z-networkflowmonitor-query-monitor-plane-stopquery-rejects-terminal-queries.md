Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.stop-query-on-completed-rejected networkflowmonitor.update-monitor-overlapping-add-remove-rejected networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal networkflowmonitor.workload-insights-data-query-lifecycle
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|StopQueryMonitorTopContributors|ValidationException
Issue-Key: networkflowmonitor.update-monitor-overlapping-add-remove-rejected|_error_code#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal|run#18|AssertionError|assert results.get("unit") is None, f"expected no unit when topContributors is empty, got {results.get('unit')!r}"
Issue-Key: networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal|run#17|AssertionError|assert message == f"There's no query with id {{{bogus_query_id}}}", (
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributorsData|ResourceNotFoundException
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# NetworkFlowMonitor query/monitor plane: StopQuery rejects terminal queries, UpdateMonitor accepts overlapping add/remove, GetQueryResults emits unit unconditionally, unknown-queryId message rule wrong (4 causes)

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopQueryMonitorTopContributors

## Description
Four distinct root causes in service networkflowmonitor, all grounded in failing behavioral checks that the suite did NOT mark ALREADY FILED and did NOT mark OWNED by an open issue. Model citations are from services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json.

=== Cause 1 of 4: StopQueryMonitorTopContributors refuses a query that has already reached a terminal status ===
AWS: Smithy: StopQueryMonitorTopContributors is DELETE /monitors/{monitorName}/topContributorsQueries/{queryId}, @idempotent, output shape is EMPTY (no members), HTTP 200 (defaulted); its modeled errors are AccessDeniedException (403), InternalServerException (500), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400) — note that it does not even model ResourceNotFoundException, i.e. the stop is modeled as an idempotent fire-and-forget. Recorded behavior: a stop issued against a query that has already finished (status SUCCEEDED) is ACCEPTED as a no-op — HTTP 200, empty body — and the query's terminal outcome is NOT overwritten to CANCELED; a subsequent GetQueryStatusMonitorTopContributors still reports SUCCEEDED. Stopping a freshly started, not-yet-terminal query also succeeds (the check's positive control).
Emulator: the call is refused with HTTP 400, error code `ValidationException`, message example `query 00000000-0000-4000-9000-000000000002 has already finished with status SUCCEEDED and cannot be stopped`. The handler evidently gates the stop on a non-terminal status instead of treating the operation as idempotent.
Why: a client that stops queries defensively on cleanup/cancel paths (the normal idempotent-stop pattern this operation is modeled for) gets a hard 400 on a benign retry, so cleanup code that works against AWS raises against the emulator, and callers must special-case a message to distinguish "already done" from a genuine bad request.
Fix: StopQueryMonitorTopContributors' handler (src/ops/stop_query_monitor_top_contributors.rs) and the query-state transition in src/state.rs — a stop on a query already in a terminal state must return 200 with an empty body and leave the stored status untouched; only a non-terminal query transitions to CANCELED.
Traces: networkflowmonitor.stop-query-on-completed-rejected.

=== Cause 2 of 4: UpdateMonitor accepts a request that adds and removes the same resource ===
AWS: Smithy: UpdateMonitor is PATCH /monitors/{monitorName} with `localResourcesToAdd`, `localResourcesToRemove`, `remoteResourcesToAdd`, `remoteResourcesToRemove`, and models ValidationException (400). Recorded behavior: a request listing the identical resource in an *ToAdd list and its matching *ToRemove list is refused with HTTP 400 `ValidationException` and the monitor's `localResources`/`remoteResources` are left unchanged; message example `invalid input -> (detected overlap between add/remove resources)`. An unambiguous, non-overlapping remove still succeeds.
Emulator: the self-contradictory update is ACCEPTED — HTTP 200 with a full UpdateMonitor payload (`monitorStatus: ACTIVE`, `localResources: [{type: AWS::Region, identifier: <region>}]`), i.e. no overlap rule exists at all. NOTE: this is a NEW failure signature for this check, which is why the suite did not mark it owned. The open issue 20261022T213607Z-networkflowmonitor-control-plane-updatescope-answers-501-updatemonitor-skips.md describes this cause as UpdateMonitor answering 501/NotImplemented (a scaffold stub); the operation now answers 200, so the stub is gone but the validation was never added. The defect to close is the missing refusal, not the 501.
Why: an overly-permissive accept is worse than a wrong error code — a client sending a contradictory patch (easy to produce when add/remove lists are computed by diffing) gets a 200 and a monitor whose resource set silently depends on the emulator's internal add/remove ordering, instead of the 400 that tells it the request was self-contradictory.
Fix: UpdateMonitor's input validation in its handler (src/ops/update_monitor.rs) — reject a request where any resource appears in both an *ToAdd and the corresponding *ToRemove list with the modeled ValidationException (HTTP 400), before any state mutation, leaving the monitor unchanged.
Traces: networkflowmonitor.update-monitor-overlapping-add-remove-rejected.

=== Cause 3 of 4: GetQueryResultsMonitorTopContributors returns `unit` even when there are no rows ===
AWS: Smithy: GetQueryResultsMonitorTopContributorsOutput has `unit` targeting the MetricUnit enum and it is OPTIONAL (not @required), while the sibling GetQueryResultsWorkloadInsightsTopContributorsDataOutput marks both `unit` and `datapoints` @required — so the model deliberately allows `unit` to be absent here. Recorded behavior: with no instrumented flow data in the queried window, `topContributors` comes back as an empty list and `unit` is OMITTED from the response entirely; it is not unconditionally `Bytes` for metricName=DATA_TRANSFERRED. When rows ARE present, `unit` is `Bytes` for that byte-count metric.
Emulator: the response always carries `unit: 'Bytes'`, including when `topContributors` is empty.
Why: `unit` is the client's signal that a result set is dimensioned/meaningful; a client that branches on the presence of `unit` (or labels a chart/axis from it) renders a unit-bearing but empty result and cannot tell "no data in window" from "data measured in bytes".
Fix: the serializer/response construction of GetQueryResultsMonitorTopContributors (src/ops/get_query_results_monitor_top_contributors.rs) — emit `unit` only when the result set is non-empty, and derive it from the query's metricName rather than hard-coding it.
Traces: networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal.

=== Cause 4 of 4: unknown-queryId ResourceNotFoundException message is not built in AWS's request-derived form ===
AWS: Smithy: GetQueryResultsWorkloadInsightsTopContributors and GetQueryResultsWorkloadInsightsTopContributorsData both model ResourceNotFoundException with httpError 404. Recorded behavior: a syntactically valid but never-issued UUID queryId is refused with HTTP 404 `ResourceNotFoundException` and a message BUILT FROM THE REQUEST — the queryId wrapped in braces after a fixed prefix: `There's no query with id {<queryId>}`. The rule is therefore known (fixed prefix + the request's queryId in braces), not just one recording's wording.
Emulator: the status and error code are correct (404 / ResourceNotFoundException); the message is rendered in the emulator's own generic form instead, e.g. `query 5014446c-f0e2-427f-9479-3771fe707077 was not found`. This is systematic across both workload-insights result readers, so it is the shared not-found renderer, not one handler's text.
Why: this is the same class of divergence already filed for the monitor not-found/conflict messages (see open issue 20261022T213607Z-networkflowmonitor-control-plane-updatescope-answers-501-updatemonitor-skips.md, causes 3-5) but for the query data plane: tooling that parses the query id out of the not-found message, or matches AWS's text, does not recognise the condition.
Fix: the not-found error construction shared by the query-results handlers (src/ops/get_query_results_workload_insights_top_contributors.rs and src/ops/get_query_results_workload_insights_top_contributors_data.rs, or their common error helper) — render AWS's `There's no query with id {<queryId>}` form from the request's queryId.
Traces: networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal, networkflowmonitor.workload-insights-data-query-lifecycle.

## Evidence
Suite: networkflowmonitor 10/24 passed (14 failed). The failures below are the ones the report left unowned and not already filed.

Cause 1 — networkflowmonitor.stop-query-on-completed-rejected [error]:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StopQueryMonitorTopContributors operation: query 00000000-0000-4000-9000-000000000002 has already finished with status SUCCEEDED and cannot be stopped
  Expected: HTTP 200, empty output shape, no error; GetQueryStatusMonitorTopContributors afterwards still SUCCEEDED.
  Actual: HTTP 400 ValidationException.

Cause 2 — networkflowmonitor.update-monitor-overlapping-add-remove-rejected [mismatch]:
  update_monitor(monitorName=<name>, localResourcesToAdd=[R], localResourcesToRemove=[R])
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'monitorName': 'sim2real-nfm-overlap-...', 'monitorStatus': 'ACTIVE', 'localResources': [{'type': 'AWS::Region', 'identifier': 'us-west-2'}], 'remoteResources': [{'type': 'AWS::AWSService', 'identifier': 'S3'}], ...}
  Expected: HTTP 400 ValidationException, monitor resource lists unchanged.
  Actual: HTTP 200 success.

Cause 3 — networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal [mismatch]:
  results = get_query_results_monitor_top_contributors(monitorName=<name>, queryId=<id>)  # topContributors == []
  assert results.get("unit") is None
  AssertionError: expected no unit when topContributors is empty, got 'Bytes'

Cause 4 — unknown queryId message (code/status already match: 404 ResourceNotFoundException):
  networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal:
    AssertionError: expected the unknown-query message for 1ab289ab-c68b-44f5-96c1-b5538628093d, got 'query 1ab289ab-c68b-44f5-96c1-b5538628093d was not found'
    Expected form: There's no query with id {1ab289ab-c68b-44f5-96c1-b5538628093d}
  networkflowmonitor.workload-insights-data-query-lifecycle:
    AssertionError: expected the unknown-query message for 5014446c-f0e2-427f-9479-3771fe707077, got 'query 5014446c-f0e2-427f-9479-3771fe707077 was not found'
    Expected form: There's no query with id {5014446c-f0e2-427f-9479-3771fe707077}

## Steps to Reproduce
Boot the emulator and point a boto3 `networkflowmonitor` client at it; take the caller's account id from STS GetCallerIdentity. Acquire the account/Region singleton scope with CreateScope (one ACCOUNT target in the caller's account and the client Region) and wait for status SUCCEEDED.

1. Cause 1: CreateMonitor(monitorName=<name>, localResources=[{type: AWS::Region, identifier: <region>}], scopeArn=<scopeArn>) and wait for monitorStatus ACTIVE. StartQueryMonitorTopContributors(monitorName=<name>, startTime=now-1h, endTime=now, metricName=DATA_TRANSFERRED, destinationCategory=UNCLASSIFIED); poll GetQueryStatusMonitorTopContributors until SUCCEEDED. Now call StopQueryMonitorTopContributors(monitorName=<name>, queryId=<id>) — observe HTTP 400 `ValidationException` instead of a 200 no-op, and confirm AWS's invariant that GetQueryStatusMonitorTopContributors still reports SUCCEEDED (not CANCELED).
2. Cause 2: CreateMonitor with localResources=[R_local={type: AWS::Region, identifier: <region>}] and remoteResources=[{type: AWS::AWSService, identifier: S3}]; wait for ACTIVE. Call UpdateMonitor(monitorName=<name>, localResourcesToAdd=[R_local], localResourcesToRemove=[R_local]) — observe HTTP 200 with a full monitor payload instead of HTTP 400 `ValidationException`; GetMonitor must show the resource lists unchanged.
3. Cause 3: with the monitor from step 1 and a query that reached SUCCEEDED over a window with no flow data, call GetQueryResultsMonitorTopContributors(monitorName=<name>, queryId=<id>) — observe `topContributors: []` together with `unit: 'Bytes'`; AWS omits `unit` entirely when there are no rows.
4. Cause 4: StartQueryWorkloadInsightsTopContributors(scopeId=<id>, startTime=now-1h, endTime=now, metricName=TIMEOUTS, destinationCategory=UNCLASSIFIED), poll to SUCCEEDED, then call GetQueryResultsWorkloadInsightsTopContributors(scopeId=<id>, queryId=<a fresh random UUID that was never started>) — observe 404 `ResourceNotFoundException` with message `query <uuid> was not found` instead of `There's no query with id {<uuid>}`. Repeat with GetQueryResultsWorkloadInsightsTopContributorsData(scopeId=<id>, queryId=<fresh UUID>) for the same divergence.
5. Re-replay to confirm: networkflowmonitor.stop-query-on-completed-rejected, networkflowmonitor.update-monitor-overlapping-add-remove-rejected, networkflowmonitor.query-monitor-top-contributors-lifecycle-terminal, networkflowmonitor.query-workload-insights-top-contributors-lifecycle-terminal, networkflowmonitor.workload-insights-data-query-lifecycle.
