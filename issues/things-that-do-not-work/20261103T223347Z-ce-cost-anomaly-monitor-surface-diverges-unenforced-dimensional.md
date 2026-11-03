Issue-Source: trace-sweep-divergence
Issue-Trace: cost-explorer-anomaly-monitor-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-explorer-anomaly-monitor-lifecycle cost-explorer-anomaly-validation-negatives
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: ce|CreateAnomalyMonitor|4xx->2xx|$.__type
Issue-Key: ce|CreateAnomalyMonitor|4xx->2xx|$.message
Issue-Key: ce|CreateAnomalyMonitor|4xx->2xx|status
Issue-Key: ce|GetAnomalyMonitors|2xx->2xx|$.AnomalyMonitors[*].MonitorName
Issue-Key: ce|GetAnomalyMonitors|2xx->2xx|$.AnomalyMonitors[*]
Issue-Key: ce|CreateAnomalyMonitor|4xx->4xx|$.message
Issue-Key: ce|GetAnomalyMonitors|4xx->4xx|$.message
Issue-Commit: 8ab069bdff3621cfe04618790b58949e3dee53e7

# ce cost anomaly monitor surface diverges: unenforced dimensional-monitor limit, emulator-worded refusals, stale reads after update and delete

> _Note: this issue was identified against the code base at commit `8ab069bdff3621cfe04618790b58949e3dee53e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAnomalyMonitor

## Description
Replay of the Cost Explorer cost-anomaly-monitor fixtures against the emulator finds FIVE distinct root causes in the anomaly-monitor plane: one admission gate that is simply absent, two refusal messages the emulator authors itself instead of using AWS's wording, and two reads that report state the preceding lifecycle call should have changed. All five are in the same crate (services/service-com-amazonaws-costexplorer-awsinsightsindexservice) and are filed together as one per-service bundle. A fixer MUST close all FIVE before submitting.

--- Cause 1 of 5: CreateAnomalyMonitor does not enforce the per-account limit on AWS-managed (DIMENSIONAL) spend monitors ---
AWS: Smithy model costexplorer#CreateAnomalyMonitor takes a required AnomalyMonitor whose MonitorType enum is DIMENSIONAL|CUSTOM; AnomalyMonitor$MonitorType documents DIMENSIONAL as "an Amazon Web Services managed monitor". The model declares only LimitExceededException (HTTP 400) for the operation and states no numeric quota, so the quota itself is recorded behavior only: the recording shows that once the account already holds a DIMENSIONAL monitor, a further CreateAnomalyMonitor for a DIMENSIONAL monitor is REFUSED with HTTP 400, __type "ValidationException" and message "Limit exceeded on dimensional spend monitor creation". That message is entirely fixed text — it names no monitor, ARN, dimension or number, so nothing in it comes from the request or from state.
Emulator: the create is ACCEPTED — HTTP 200 with a minted MonitorArn and no error body at all ($.__type and $.message are absent). The only admission gate in the handler is a store-size cap (`if store.anomaly_monitors.len() >= crate::state::MAX_ANOMALY_MONITORS`, MAX_ANOMALY_MONITORS = 100, refused as LimitExceededException with the emulator's own sentence "You have reached the limit of 100 cost anomaly monitors for this account"); there is no gate counting DIMENSIONAL monitors, so the second AWS-managed monitor is persisted.
Why: a client that provisions cost anomaly detection idempotently relies on the second DIMENSIONAL create failing with the 400 ValidationException to detect "the managed monitor already exists" and reuse it. Against the emulator it instead gets a second MonitorArn, so the account ends up with duplicate managed monitors, duplicate anomaly evaluation, and a client that believes it created a monitor AWS would never have created. Error-handling paths keyed on ValidationException are dead code and never exercised.
Fix: CreateAnomalyMonitor's admission/validation step in the operation handler, backed by a DIMENSIONAL-monitor count over the shared monitor registry in the service state (the same place MAX_ANOMALY_MONITORS is checked today), refusing with 400 ValidationException and AWS's fixed sentence rather than the handler's own LimitExceededException text.
Traces: cost-explorer-anomaly-monitor-lifecycle#task#4 (expected 400 / ValidationException / "Limit exceeded on dimensional spend monitor creation"; got 200 with no error members).

--- Cause 2 of 5: a renamed monitor is still reported under its old MonitorName ---
AWS: Smithy model costexplorer#UpdateAnomalyMonitor takes required MonitorArn plus MonitorName and documents "The new name for the cost anomaly monitor"; its only lookup failure is UnknownMonitorException (HTTP 404). A subsequent GetAnomalyMonitors must therefore report the monitor's AnomalyMonitors[].MonitorName as the name the update set (and its LastUpdatedDate as the time of that update).
Emulator: GetAnomalyMonitors answers 200 but reports the monitor's ORIGINAL name — AnomalyMonitors[0].MonitorName is the create-time name where the renamed value is expected. Root cause: the rename is never applied to the registry at all, because services/.../src/ops/update_anomaly_monitor.rs is still a scaffold whose handle() returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateAnomalyMonitor"))`, so the monitor record in state.rs (AnomalyMonitorRecord.monitor_name / last_updated_date) is never rewritten. The 501 on the Update call itself is reported separately as an unimplemented-operation finding; this cause is the OBSERVABLE STATE divergence on the read side, which no open issue names: GetAnomalyMonitors serves a monitor whose name does not match the account's monitor.
Why: any client that renames a monitor and then reads it back (console-style refresh, or a converge loop that compares desired vs actual name) sees the rename silently lost and will re-issue the rename forever, never converging.
Fix: implement the UpdateAnomalyMonitor handler so it resolves MonitorArn in the shared monitor registry (404 UnknownMonitorException when absent), writes monitor_name and refreshes last_updated_date on the stored AnomalyMonitorRecord, and returns MonitorArn — the read path in get_anomaly_monitors.rs already projects the record faithfully.
Traces: cost-explorer-anomaly-monitor-lifecycle#task#10 ($.AnomalyMonitors[0].MonitorName: expected the renamed value, got the create-time name).

--- Cause 3 of 5: a deleted monitor remains visible to GetAnomalyMonitors ---
AWS: Smithy model costexplorer#DeleteAnomalyMonitor removes the monitor (UnknownMonitorException, HTTP 404, documents "The cost anomaly monitor does not exist for the account"), after which GetAnomalyMonitors for the account must answer 200 with an EMPTY required AnomalyMonitors list — the monitor is gone from the registry, not tombstoned or hidden.
Emulator: GetAnomalyMonitors answers 200 but still returns the monitor as AnomalyMonitors[0], with its full projection (MonitorArn, MonitorName, CreationDate, LastUpdatedDate, MonitorType DIMENSIONAL, MonitorDimension SERVICE, DimensionalValueCount), where the expected list has no element at that index. Root cause: services/.../src/ops/delete_anomaly_monitor.rs is still a scaffold returning `Scaffolded::not_implemented("DeleteAnomalyMonitor")`, so the entry is never removed from the `anomaly_monitors` BTreeMap in state.rs. The 501 on the Delete call is reported separately as an unimplemented-operation finding; this cause is the read-side consequence — the registry keeps serving a resource the account deleted.
Why: teardown never converges. A client that deletes its monitors and then polls GetAnomalyMonitors to confirm an empty account loops forever; a test fixture that asserts a clean account after cleanup fails; and a re-create flow that first lists, finds the stale monitor and skips creation ends up with no monitor at all.
Fix: implement the DeleteAnomalyMonitor handler to remove the ARN's entry from the shared monitor registry in the service state (404 UnknownMonitorException when the ARN names no monitor), so the existing GetAnomalyMonitors range scan reports the account's real contents.
Traces: cost-explorer-anomaly-monitor-lifecycle#task#13 ($.AnomalyMonitors[0]: expected absent, got the full monitor projection).

--- Cause 4 of 5: the missing-MonitorSpecification refusal on CreateAnomalyMonitor is emulator-worded and dimension-interpolated ---
AWS: Smithy model AnomalyMonitor$MonitorSpecification documents that for an AWS-managed monitor, "If MonitorDimension is TAG, set this field to { \"Tags\": { \"Key\": \"your tag key\" } }" and "If MonitorDimension is COST_CATEGORY, set this field to { \"CostCategories\": { \"Key\": \"your cost category key\" } }" (MonitorDimension enum: SERVICE, LINKED_ACCOUNT, TAG, COST_CATEGORY). The model declares no client-input error for the operation, so the refusal class and wording are recorded behavior only: HTTP 400 with message "MonitorSpecification must be present for TAG and COST_CATEGORY dimensional monitors". The recording shows that ONE sentence for BOTH dimensions (the cluster covers the TAG case and the COST_CATEGORY case and expects identical text), so the whole message is fixed text — it does not name the offending dimension and has no variable part taken from the request.
Emulator: the status (400) and class are right but the message is the crate's own prose, built per dimension in `crate::state::validate_anomaly_monitor`: `"MonitorSpecification must specify {selector} with a Key when MonitorDimension is {dimension}"` — e.g. "MonitorSpecification must specify Tags with a Key when MonitorDimension is TAG" for TAG and the CostCategories/COST_CATEGORY variant for COST_CATEGORY. It interpolates request values into a message AWS keeps constant.
Why: clients and SDK middlewares that match Cost Explorer validation failures on message text (Cost Explorer's ValidationException carries no structured field reference) do not match, so a well-known misconfiguration is surfaced to operators as an unrecognized error; and the per-dimension wording teaches users a message AWS never sends.
Fix: the shared AnomalyMonitor validation helper in the service state module (`validate_anomaly_monitor`, the TAG / COST_CATEGORY missing-key arm) must emit AWS's single fixed sentence for both dimensions instead of composing one from the selector and dimension.
Traces: cost-explorer-anomaly-validation-negatives#task#1 and cost-explorer-anomaly-validation-negatives#task#2 (both expect "MonitorSpecification must be present for TAG and COST_CATEGORY dimensional monitors").

--- Cause 5 of 5: the GetAnomalyMonitors MaxResults refusal is emulator-worded and echoes the rejected value ---
AWS: Smithy model costexplorer#GetAnomalyMonitors binds MaxResults to PageSize and is @paginated (pageSize MaxResults). A page size below 1 is refused with HTTP 400 and message "Number of results per page cannot be smaller than 1" (recorded behavior; the model states the member but no range trait, and declares only InvalidNextTokenException / LimitExceededException / UnknownMonitorException). The message is wholly fixed text: the minimum 1 is the rule's own limit and the REJECTED VALUE IS NOT ECHOED.
Emulator: status 400 is right, but the message is the handler's own sentence, formatted with the rejected number: `"MaxResults {requested} is not a page size; it must be at least 1"` — e.g. "MaxResults -1 is not a page size; it must be at least 1". Two divergences in one: non-AWS wording, and a request value interpolated into a message AWS keeps constant.
Why: callers that assert on Cost Explorer's paging-validation text (and operators reading it) see an unrecognized message; the echoed value also means the text varies per request where AWS's does not, so any exact-match handling is doubly wrong.
Fix: GetAnomalyMonitors' input validation in its operation handler (the `page_size` guard that maps a non-positive/unconvertible MaxResults to `Scaffolded::validation`) must emit AWS's fixed sentence without interpolating the rejected value.
Traces: cost-explorer-anomaly-validation-negatives#task#3 (expected "Number of results per page cannot be smaller than 1").

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, 37 steps (5 passed, 32 mismatched), 6 diverged steps in 5 clusters — all service "ce", none carrying stub_path or owned_by.

Cause 1 — cost-explorer-anomaly-monitor-lifecycle#task#4, CreateAnomalyMonitor:
  expected status 400, actual 200
  $.__type: missing (expected 'ValidationException', got None)
  $.message: missing (expected 'Limit exceeded on dimensional spend monitor creation', got None)

Cause 2 — cost-explorer-anomaly-monitor-lifecycle#task#10, GetAnomalyMonitors (200 vs 200):
  $.AnomalyMonitors[0].MonitorName: value_mismatch (expected the renamed value '<name>-renamed-<suffix>', got the create-time name '<name>')

Cause 3 — cost-explorer-anomaly-monitor-lifecycle#task#13, GetAnomalyMonitors (200 vs 200):
  $.AnomalyMonitors[0]: extra (expected None, got a full monitor projection: MonitorArn arn:aws:ce::<account>:anomalymonitor/<id>, MonitorName, CreationDate, LastUpdatedDate, MonitorType DIMENSIONAL, MonitorDimension SERVICE, DimensionalValueCount 0)

Cause 4 — cost-explorer-anomaly-validation-negatives#task#1 (+ #task#2), CreateAnomalyMonitor (400 vs 400):
  $.message: value_mismatch
    expected: 'MonitorSpecification must be present for TAG and COST_CATEGORY dimensional monitors'
    actual:   'MonitorSpecification must specify Tags with a Key when MonitorDimension is TAG'

Cause 5 — cost-explorer-anomaly-validation-negatives#task#3, GetAnomalyMonitors (400 vs 400):
  $.message: value_mismatch
    expected: 'Number of results per page cannot be smaller than 1'
    actual:   'MaxResults -1 is not a page size; it must be at least 1'

Emulator-side code the divergences land on (citations only):
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/create_anomaly_monitor.rs:154-175 (validation call + the only admission gate, a MAX_ANOMALY_MONITORS store cap)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/state.rs:548-596 (validate_anomaly_monitor; the TAG/COST_CATEGORY arm at 590-596 composes the message), :723-730 (MAX_ANOMALY_MONITORS = 100, "CHOSEN" bound), :702-709 (anomaly_monitors registry)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/get_anomaly_monitors.rs:176-186 (MaxResults guard message), :216-233 (registry range scan the stale reads come from)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/update_anomaly_monitor.rs:141-159 (scaffold returning not_implemented — why the rename never lands)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/delete_anomaly_monitor.rs:3-12 (SCAFFOLDED marker — why the delete never lands)

Related open issues that do NOT cover any of these five: "Implement ce CreateAnomalySubscription: unimplemented scaffold stub" (20261103T130237Z-ce-createanomalysubscription-stub.md) is a different operation; the auto-filed unimplemented-operation findings for ce UpdateAnomalyMonitor / DeleteAnomalyMonitor cover the 501 those calls return, not the read-side state divergences in causes 2 and 3.

## Steps to Reproduce
Boot the emulator from this checkout and replay the two Cost Explorer fixtures (replay_wire_corpus, trace set simparity-wire-aws-v1).

Cause 1 (cost-explorer-anomaly-monitor-lifecycle#task#4):
  1. CreateAnomalyMonitor with AnomalyMonitor { MonitorName: <name-a>, MonitorType: DIMENSIONAL, MonitorDimension: SERVICE } -> 200, MonitorArn minted.
  2. CreateAnomalyMonitor again with a DIMENSIONAL monitor for the same account -> AWS answers 400 ValidationException "Limit exceeded on dimensional spend monitor creation"; the emulator answers 200 with a second MonitorArn.

Cause 2 (cost-explorer-anomaly-monitor-lifecycle#task#10):
  1. Create a DIMENSIONAL/SERVICE monitor.
  2. UpdateAnomalyMonitor { MonitorArn: <arn>, MonitorName: <new-name> }.
  3. GetAnomalyMonitors -> AnomalyMonitors[0].MonitorName is still the create-time name instead of <new-name>.

Cause 3 (cost-explorer-anomaly-monitor-lifecycle#task#13):
  1. Create a DIMENSIONAL/SERVICE monitor.
  2. DeleteAnomalyMonitor { MonitorArn: <arn> }.
  3. GetAnomalyMonitors -> expected 200 with an empty AnomalyMonitors list; the emulator still returns the deleted monitor at AnomalyMonitors[0].

Cause 4 (cost-explorer-anomaly-validation-negatives#task#1, #task#2):
  1. CreateAnomalyMonitor with AnomalyMonitor { MonitorType: DIMENSIONAL, MonitorDimension: TAG } and NO MonitorSpecification -> 400, but the message is "MonitorSpecification must specify Tags with a Key when MonitorDimension is TAG" instead of AWS's "MonitorSpecification must be present for TAG and COST_CATEGORY dimensional monitors".
  2. Repeat with MonitorDimension: COST_CATEGORY -> the CostCategories/COST_CATEGORY variant of the same emulator prose, where AWS sends the identical fixed sentence as step 1.

Cause 5 (cost-explorer-anomaly-validation-negatives#task#3):
  1. GetAnomalyMonitors with MaxResults: -1 -> 400, but the message is "MaxResults -1 is not a page size; it must be at least 1" instead of AWS's "Number of results per page cannot be smaller than 1" (note AWS does not echo the rejected value).
