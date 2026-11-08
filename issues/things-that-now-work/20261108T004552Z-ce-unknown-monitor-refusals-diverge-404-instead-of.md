Issue-Source: trace-sweep-divergence
Issue-Trace: cost-explorer-anomaly-monitor-lifecycle
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-explorer-anomaly-monitor-lifecycle cost-explorer-anomaly-validation-negatives
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: ce|GetAnomalyMonitors|2xx->4xx|$.AnomalyMonitors
Issue-Key: ce|GetAnomalyMonitors|2xx->4xx|status
Issue-Key: ce|DeleteAnomalyMonitor|4xx->4xx|$.Message
Issue-Key: ce|DeleteAnomalyMonitor|4xx->4xx|status
Issue-Key: ce|CreateAnomalySubscription|4xx->4xx|$.Message
Issue-Key: ce|CreateAnomalySubscription|4xx->4xx|status
Issue-Commit: 06ab0242402a8d038c4677fdb4da18af84e7f2d9

# ce unknown-monitor refusals diverge: 404 instead of AWS's 400, emulator-worded messages, and an ARN filter that 404s instead of paging empty

> _Note: this issue was identified against the code base at commit `06ab0242402a8d038c4677fdb4da18af84e7f2d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnomalyMonitors

## Description
Replay of the Cost Explorer anomaly fixtures against the emulator finds THREE net-new root causes, all of them in how the anomaly-monitor plane answers a request that names a monitor the account does not hold (services/service-com-amazonaws-costexplorer-awsinsightsindexservice). They share one fault: the crate treats "this ARN names no monitor" as the model's defaulted 404 UnknownMonitorException carrying a handler-authored, ARN-interpolated sentence, where AWS answers HTTP 400 with its own fixed "No monitor present with ..." wording — and on the ARN-filtered read AWS does not refuse at all. All three are in one service and are filed as one per-service bundle. A fixer MUST close all THREE before submitting.

These clusters are net-new: the replay reports no stub_path and no owned_by for any of them. Two open issues are adjacent but carry neither cluster's replay key and do NOT cover them: "ce cost anomaly monitor surface diverges: unenforced dimensional-monitor limit, emulator-worded refusals, stale reads after update and delete" (20261103T223347Z-ce-cost-anomaly-monitor-surface-diverges-unenforced-dimensional.md) owns the earlier 200-vs-200 stale-read clusters at task#10/task#13 (back when Delete never removed the monitor), not today's 200->404 and 400->404 refusal clusters; "ce anomaly-subscription refusals diverge: unknown-monitor create admitted, invalid-token message reworded" (20261104T005255Z-ce-anomaly-subscription-refusals-diverge-unknown-monitor-create.md) owns the 400->200 admitted-create cluster at task#16, i.e. the gate never firing — now that the delete lands, the gate fires and refuses WRONGLY, which is Cause 3 here.

--- Cause 1 of 3: GetAnomalyMonitors filtered by an ARN the account holds no monitor for is refused 404 instead of answering 200 with an empty list ---
AWS: Smithy model costexplorer#GetAnomalyMonitors takes MonitorArnList (a list of monitor ARNs used as a FILTER), returns the required AnomalyMonitors list and is @paginated (items AnomalyMonitors, pageSize MaxResults); it declares UnknownMonitorException (model httpError 404). The recorded behavior for a read whose MonitorArnList names a monitor that no longer exists is HTTP 200 with AnomalyMonitors = [] — an ARN filter selects the monitors the account holds and simply matches nothing, it does not assert existence, so no error is raised and no error members appear in the body.
Emulator: the read is REFUSED with HTTP 404 and no AnomalyMonitors member at all. ops/get_anomaly_monitors.rs tests each filter ARN for registry membership before paging and raises UnknownMonitorException("The cost anomaly monitor <arn> does not exist for the account.") for the first miss.
Why: the normal teardown/confirm loop breaks. A client that deletes a monitor and then re-reads the SAME ARN to confirm it is gone gets a 404 where AWS reports an empty result set, so "confirm deleted" is indistinguishable from "request failed"; converge loops that list by a known ARN set retry forever, and a caller paging a mixed ARN list loses the monitors that DO exist because one stale ARN fails the whole call.
Fix: the filter arm of the GetAnomalyMonitors handler — drop the per-ARN existence assertion and let the page predicate select only matching monitors, so an ARN naming nothing contributes no element and the operation answers 200 with the (possibly empty) required AnomalyMonitors list.
Traces: cost-explorer-anomaly-monitor-lifecycle#task#13 (expected status 200 with $.AnomalyMonitors = []; got 404 with $.AnomalyMonitors missing).

--- Cause 2 of 3: DeleteAnomalyMonitor's unknown-monitor refusal uses status 404 and an emulator-authored, ARN-interpolated message ---
AWS: Smithy model costexplorer#DeleteAnomalyMonitor takes required MonitorArn and declares UnknownMonitorException (model httpError 404, doc "The cost anomaly monitor does not exist for the account.") and LimitExceededException. The status and wording actually served are recorded behavior: HTTP 400 with Message "No monitor present with resourceId: <resourceId>" — AWS answers 400 here, NOT the 404 the model defaults the error to. Fixed text is "No monitor present with resourceId: "; the single variable part is <resourceId>, the bare monitor-id component of the MonitorArn taken from the request (not the full ARN). The id in the recording is one run's generated value and is not part of the requirement.
Emulator: the error class matches ($.__type does not diverge) but the status is 404 and the message is the crate's own prose with the whole ARN interpolated: "The cost anomaly monitor arn:aws:ce::<account>:anomalymonitor/<id> does not exist for the account." — the exception's Smithy doc sentence, not AWS's error text.
Why: a deleting client (idempotent teardown, re-run of a cleanup step, delete of an ARN that never existed) classifies by status first: AWS's 400 is a client-input refusal, while 404 is routinely treated by SDK/retry middleware and HTTP proxies as a routing/resource-path failure, so "already deleted" is mis-handled as a transport or endpoint problem. Cost Explorer's refusals carry no structured field reference, so text matching on "No monitor present with resourceId:" is the only discriminator available and it never matches.
Fix: the unknown-monitor refusal path of the DeleteAnomalyMonitor handler — raise UnknownMonitorException with HTTP status 400 (an explicit status override for this error, since the modeled default is 404) and compose AWS's fixed sentence from the monitor-id component of the request's MonitorArn instead of the doc string plus full ARN.
Traces: cost-explorer-anomaly-monitor-lifecycle#task#14 and cost-explorer-anomaly-validation-negatives#task#7 (both expect 400 / "No monitor present with resourceId: <resourceId>"; got 404 / the ARN-interpolated sentence).

--- Cause 3 of 3: CreateAnomalySubscription's unknown-monitor refusal uses status 404 and the same emulator-authored message ---
AWS: Smithy model costexplorer#CreateAnomalySubscription takes a required AnomalySubscription whose MonitorArnList is a required list of the monitors the subscription alerts on, and declares UnknownMonitorException (model httpError 404) and LimitExceededException. Recorded behavior for a create naming a monitor the account does not hold is HTTP 400 with Message "No monitor present with monitorId: <monitorId>" — again 400, not the modeled 404, and note the member word differs from Delete's: "monitorId" here, "resourceId" there. Fixed text is "No monitor present with monitorId: "; the only variable part is <monitorId>, the bare monitor-id component of the ARN named in the request's AnomalySubscription.MonitorArnList.
Emulator: the per-ARN gate in ops/create_anomaly_subscription.rs now fires (the delete removes the monitor, so the stale-registry condition the older issue described is gone), but it refuses with HTTP 404 and the same handler-authored sentence "The cost anomaly monitor <arn> does not exist for the account." The error class itself matches; status and message do not.
Why: a client wiring alert subscriptions after a monitor teardown must tell "your monitor reference is stale, fix the request" (AWS: 400) from "the endpoint/resource path is wrong" (404). Getting 404 sends it down transport/endpoint-retry paths instead of its dangling-reference handling, and the re-worded message defeats the text match that is the only classifier Cost Explorer offers.
Fix: the per-ARN monitor-reference gate in the CreateAnomalySubscription handler — raise UnknownMonitorException with HTTP status 400 and AWS's fixed "monitorId" sentence built from the monitor id in the request, ideally through one shared unknown-monitor refusal helper in the service state module so Delete, Create-subscription and any future caller cannot drift apart again (keeping each operation's own member word).
Traces: cost-explorer-anomaly-monitor-lifecycle#task#16 (expected 400 / "No monitor present with monitorId: <monitorId>"; got 404 / the ARN-interpolated sentence).

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, score_pct 16.2, 37 steps (6 passed, 31 mismatched, 0 errored, 0 blocked), 10 diverged steps in 8 clusters — every cluster service "ce". Five clusters carry owned_by (the two open ce bundles) and are skipped; the three below carry neither stub_path nor owned_by.

Cause 1 — cost-explorer-anomaly-monitor-lifecycle#task#13, GetAnomalyMonitors:
  status: expected 200, got 404
  $.AnomalyMonitors: missing (expected [], got None)
  divergence_key: service=ce|GetAnomalyMonitors|status=200->404|keys=$.AnomalyMonitors,status

Cause 2 — cost-explorer-anomaly-monitor-lifecycle#task#14 (+ cost-explorer-anomaly-validation-negatives#task#7), DeleteAnomalyMonitor:
  status: expected 400, got 404
  $.Message: value_mismatch
    expected: 'No monitor present with resourceId: <monitorId>'   (recorded: 'No monitor present with resourceId: 968b991d-18bf-834d-b5b6-30d38ef68eb1')
    actual:   'The cost anomaly monitor arn:aws:ce::<account>:anomalymonitor/<monitorId> does not exist for the account.'
  divergence_key: service=ce|DeleteAnomalyMonitor|status=400->404|keys=$.Message,status

Cause 3 — cost-explorer-anomaly-monitor-lifecycle#task#16, CreateAnomalySubscription:
  status: expected 400, got 404
  $.Message: value_mismatch
    expected: 'No monitor present with monitorId: <monitorId>'   (recorded: 'No monitor present with monitorId: 968b991d-18bf-834d-b5b6-30d38ef68eb1')
    actual:   'The cost anomaly monitor arn:aws:ce::<account>:anomalymonitor/<monitorId> does not exist for the account.'
  divergence_key: service=ce|CreateAnomalySubscription|status=400->404|keys=$.Message,status

In causes 2 and 3 the generated monitor id and the account number are per-run values filling the message template; the requirement is the fixed text plus the monitor id taken from the request's ARN. $.__type does not diverge in any cluster: the class UnknownMonitorException is right, the status and the message text are not.

Model facts used (smithy_query on services/service-com-amazonaws-costexplorer-awsinsightsindexservice/smithy-model/com-amazonaws-costexplorer-awsinsightsindexservice.json):
  GetAnomalyMonitors: input MonitorArnList (list of ARNs), NextPageToken, MaxResults; output AnomalyMonitors (required list); @paginated items=AnomalyMonitors pageSize=MaxResults; errors InvalidNextTokenException (400, defaulted), LimitExceededException (400, defaulted), UnknownMonitorException (404)
  DeleteAnomalyMonitor: input MonitorArn (required); errors LimitExceededException (400, defaulted), UnknownMonitorException (404)
  CreateAnomalySubscription: input AnomalySubscription (required), ResourceTags; output SubscriptionArn (required); errors LimitExceededException (400, defaulted), UnknownMonitorException (404)
  UnknownMonitorException: client error, httpError 404, member Message, doc "The cost anomaly monitor does not exist for the account." — the sentence the emulator serves as its message text

Emulator-side code the divergences land on (citations only):
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/get_anomaly_monitors.rs:172-195 (filter ARN existence assertion raising 404) and :197-216 (the page walk whose predicate already filters correctly)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/delete_anomaly_monitor.rs:142-158 (unknown-monitor refusal: modeled 404 + ARN-interpolated doc sentence)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/create_anomaly_subscription.rs:176-190 (per-ARN monitor gate: modeled 404 + the same sentence)

## Steps to Reproduce
Boot the emulator from this checkout and replay the Cost Explorer fixtures (replay_wire_corpus, trace set simparity-wire-aws-v1).

Cause 1 (cost-explorer-anomaly-monitor-lifecycle#task#13):
  1. CreateAnomalyMonitor with AnomalyMonitor { MonitorName: <name>, MonitorType: DIMENSIONAL, MonitorDimension: SERVICE } -> 200, MonitorArn minted.
  2. DeleteAnomalyMonitor { MonitorArn: <arn> } -> the account no longer holds that monitor.
  3. GetAnomalyMonitors { MonitorArnList: [<arn>] } -> AWS answers 200 with AnomalyMonitors = []; the emulator answers 404 UnknownMonitorException with no AnomalyMonitors member.

Cause 2 (cost-explorer-anomaly-monitor-lifecycle#task#14, same cluster as cost-explorer-anomaly-validation-negatives#task#7):
  1. Take a monitor ARN the account holds no monitor under (either already deleted as above, or a well-formed ARN never created).
  2. DeleteAnomalyMonitor { MonitorArn: <arn> } -> AWS answers HTTP 400, __type UnknownMonitorException, Message "No monitor present with resourceId: <monitorId>" where <monitorId> is the id component of <arn>; the emulator answers HTTP 404 with Message "The cost anomaly monitor <arn> does not exist for the account."

Cause 3 (cost-explorer-anomaly-monitor-lifecycle#task#16):
  1. Create a DIMENSIONAL/SERVICE monitor, then DeleteAnomalyMonitor its ARN.
  2. CreateAnomalySubscription with AnomalySubscription { SubscriptionName: <n>, MonitorArnList: [<arn>], Subscribers: [...], Frequency: ..., ThresholdExpression: ... } -> AWS answers HTTP 400, __type UnknownMonitorException, Message "No monitor present with monitorId: <monitorId>"; the emulator answers HTTP 404 with Message "The cost anomaly monitor <arn> does not exist for the account."
