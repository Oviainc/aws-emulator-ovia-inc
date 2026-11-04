Issue-Source: trace-sweep-divergence
Issue-Trace: cost-explorer-anomaly-monitor-lifecycle
Issue-Step: task#16
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-explorer-anomaly-monitor-lifecycle cost-explorer-anomaly-validation-negatives
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: ce|CreateAnomalySubscription|4xx->2xx|$.Message
Issue-Key: ce|CreateAnomalySubscription|4xx->2xx|$.__type
Issue-Key: ce|CreateAnomalySubscription|4xx->2xx|status
Issue-Key: ce|GetAnomalySubscriptions|4xx->4xx|$.Message
Issue-Commit: 8be2350bb4c96a8f877db5e7883a5517ed4233d5

# ce anomaly-subscription refusals diverge: unknown-monitor create admitted, invalid-token message reworded

> _Note: this issue was identified against the code base at commit `8be2350bb4c96a8f877db5e7883a5517ed4233d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAnomalySubscription

## Description
Replay of the Cost Explorer anomaly fixtures against the emulator finds TWO net-new root causes in the anomaly-SUBSCRIPTION plane (services/service-com-amazonaws-costexplorer-awsinsightsindexservice): a create that must be refused because it names a monitor the account no longer holds is ACCEPTED, and the invalid-pagination-token refusal on the subscription read carries the emulator's own sentence instead of AWS's. Both are in one service and are filed as one per-service bundle. A fixer MUST close BOTH causes before submitting.

Related open issue that does NOT cover either cause: "ce cost anomaly monitor surface diverges: unenforced dimensional-monitor limit, emulator-worded refusals, stale reads after update and delete" (20261103T223347Z-ce-cost-anomaly-monitor-surface-diverges-unenforced-dimensional.md) is about the anomaly-MONITOR operations (CreateAnomalyMonitor / GetAnomalyMonitors) and carries no replay key for either step below; the resolved issue "Implement ce CreateAnomalySubscription: unimplemented scaffold stub" (20261103T130237Z-ce-createanomalysubscription-stub.md) is fixed and only covered the 501 the operation used to answer.

--- Cause 1 of 2: CreateAnomalySubscription naming a monitor the account no longer holds is ACCEPTED (200) instead of refused as UnknownMonitorException ---
AWS: Smithy model costexplorer#CreateAnomalySubscription takes a required AnomalySubscription whose MonitorArnList is a required list documented as "A list of cost anomaly monitors", and declares UnknownMonitorException (model httpError 404, doc "The cost anomaly monitor does not exist for the account") alongside LimitExceededException. A subscription may only alert on monitors the account actually holds, so once a monitor has been deleted, a create that still names its ARN must be refused. The status and wording are recorded behavior: HTTP 400 with __type "UnknownMonitorException" and Message "No monitor present with monitorId: <monitorId>" — note AWS answers 400 here, not the 404 the model defaults the error to. The message's fixed text is "No monitor present with monitorId: "; the only variable part is <monitorId>, the monitor-id component of the ARN taken from the request's AnomalySubscription.MonitorArnList (the bare id, NOT the full ARN). The id value in the recording is one run's generated id and is not part of the requirement.
Emulator: the create is ACCEPTED — HTTP 200 with a minted SubscriptionArn and no error members at all ($.__type and $.Message absent). The handler does hold an unknown-monitor gate (ops/create_anomaly_subscription.rs: a `store.anomaly_monitors.contains_key(monitor_arn)` test per ARN), but it never fires for this request because the preceding DeleteAnomalyMonitor never removed the monitor from the registry, so the stale monitor still satisfies the gate. Two further divergences are latent in that same gate even when it does fire: it refuses with the modeled error's defaulted HTTP 404 rather than AWS's 400, and its message is the crate's own prose "The cost anomaly monitor {monitor_arn} does not exist for the account." — the full ARN interpolated into wording AWS does not use, where AWS names only the monitor id in the fixed sentence above.
Why: a client that tears down a monitor and then (re)creates alert subscriptions relies on the 400 UnknownMonitorException to learn that its monitor reference is stale. Against the emulator it instead receives a SubscriptionArn for a subscription bound to a monitor that does not exist, so the account ends up with alert subscriptions that can never fire, and the client's dangling-reference handling is dead code. Clients that do reach the gate (monitor ARN never created) see the wrong status class (404 instead of 400) and an unrecognized message.
Fix: CreateAnomalySubscription's monitor-reference check in its operation handler must resolve against a monitor registry that reflects deletes — i.e. the shared anomaly-monitor registry in the service state module must drop an entry when DeleteAnomalyMonitor runs — and the refusal it raises must carry HTTP 400 with __type UnknownMonitorException and AWS's fixed sentence naming the monitor id from the request rather than the handler's own ARN-interpolated text.
Traces: cost-explorer-anomaly-monitor-lifecycle#task#16.

--- Cause 2 of 2: the GetAnomalySubscriptions invalid-pagination-token refusal is emulator-worded ---
AWS: Smithy model costexplorer#GetAnomalySubscriptions is @paginated (inputToken/outputToken NextPageToken, pageSize MaxResults) and declares InvalidNextTokenException (httpError 400, defaulted) for an unreadable cursor. The status (400) and the error class are the model's; the wording is recorded behavior: Message "Provided token is invalid". That message is wholly fixed text — it echoes neither the rejected token nor any member name, so nothing in it comes from the request or from state.
Emulator: status 400 and class InvalidNextTokenException are right, but $.Message is the crate's own sentence, lifted from the exception's Smithy DOC string rather than AWS's error text: "The pagination token is invalid. Try again without a pagination token." (ops/get_anomaly_subscriptions.rs, the NextPageToken decode arm).
Why: Cost Explorer's pagination failures carry no structured field reference, so callers, SDK middlewares and operators match this condition on message text; an unrecognized sentence makes a routine resume-with-a-stale-cursor look like an unknown failure, and retry/restart-without-token logic keyed on AWS's text never triggers.
Fix: the NextPageToken decode failure arm of the GetAnomalySubscriptions handler must emit AWS's fixed sentence for InvalidNextTokenException instead of the model documentation string the handler currently formats.
Traces: cost-explorer-anomaly-validation-negatives#task#6.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, 37 steps (5 passed, 32 mismatched), 8 diverged steps in 7 clusters, all service "ce". Five clusters carry owned_by (the open anomaly-monitor bundle) and are skipped; the two below carry neither stub_path nor owned_by.

Cause 1 — cost-explorer-anomaly-monitor-lifecycle#task#16, CreateAnomalySubscription:
  status: expected 400, got 200
  $.__type: missing (expected 'UnknownMonitorException', got None)
  $.Message: missing (expected 'No monitor present with monitorId: <monitorId>', got None)
  divergence_key: service=ce|CreateAnomalySubscription|status=400->200|keys=$.Message,$.__type,status

Cause 2 — cost-explorer-anomaly-validation-negatives#task#6, GetAnomalySubscriptions (400 vs 400):
  $.Message: value_mismatch
    expected: 'Provided token is invalid'
    actual:   'The pagination token is invalid. Try again without a pagination token.'
  divergence_key: service=ce|GetAnomalySubscriptions|status=400->400|keys=$.Message

Model facts used (smithy_query on services/service-com-amazonaws-costexplorer-awsinsightsindexservice/smithy-model/com-amazonaws-costexplorer-awsinsightsindexservice.json):
  CreateAnomalySubscription: input AnomalySubscription (required); errors LimitExceededException (400), UnknownMonitorException (404, doc "The cost anomaly monitor does not exist for the account.")
  AnomalySubscription$MonitorArnList: required list, doc "A list of cost anomaly monitors."
  GetAnomalySubscriptions: @paginated (NextPageToken/MaxResults); errors InvalidNextTokenException (400, defaulted; doc "The pagination token is invalid. Try again without a pagination token."), LimitExceededException, UnknownSubscriptionException

Emulator-side code the divergences land on (citations only):
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/create_anomaly_subscription.rs:176-190 (per-ARN unknown-monitor gate: registry membership test, 404 class, ARN-interpolated message)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/create_anomaly_subscription.rs:161-175 (admission before write; only the subscription-count cap precedes the monitor check)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/delete_anomaly_monitor.rs:3-12 (SCAFFOLDED marker — why the monitor is still in the registry at task#16)
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/get_anomaly_subscriptions.rs:153-168 (InvalidNextTokenException message)

## Steps to Reproduce
Boot the emulator from this checkout and replay the Cost Explorer fixtures (replay_wire_corpus, trace set simparity-wire-aws-v1).

Cause 1 (cost-explorer-anomaly-monitor-lifecycle#task#16):
  1. CreateAnomalyMonitor with AnomalyMonitor { MonitorType: DIMENSIONAL, MonitorDimension: SERVICE } -> 200, MonitorArn minted.
  2. DeleteAnomalyMonitor { MonitorArn: <arn> } (the account no longer holds that monitor).
  3. CreateAnomalySubscription with AnomalySubscription { MonitorArnList: [<arn>], Subscribers: [...], Frequency: ..., SubscriptionName: ..., ThresholdExpression: ... } -> AWS answers 400 with __type UnknownMonitorException and Message "No monitor present with monitorId: <monitorId>" (the monitor-id component of <arn>); the emulator answers 200 with a SubscriptionArn and no error members.
  4. Variant that exercises the latent half: call CreateAnomalySubscription naming a well-formed monitor ARN that was never created -> the emulator does refuse, but with HTTP 404 and message "The cost anomaly monitor <arn> does not exist for the account." where AWS sends 400 and the fixed sentence above with only the monitor id.

Cause 2 (cost-explorer-anomaly-validation-negatives#task#6):
  1. GetAnomalySubscriptions { NextPageToken: <a token that does not decode> } -> 400 InvalidNextTokenException, but $.Message is "The pagination token is invalid. Try again without a pagination token." instead of AWS's "Provided token is invalid".
