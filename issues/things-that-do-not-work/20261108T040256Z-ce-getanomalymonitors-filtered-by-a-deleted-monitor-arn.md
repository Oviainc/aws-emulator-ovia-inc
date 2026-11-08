Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ce.anomaly-monitor-delete-orphans-subscription
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: cost-explorer|GetAnomalyMonitors|UnknownMonitorException
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# ce: GetAnomalyMonitors filtered by a deleted monitor ARN is refused 404 UnknownMonitorException instead of answering 200 with an empty AnomalyMonitors list

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAnomalyMonitors

## Description
One net-new root cause in the Cost Explorer anomaly-monitor read plane (crate services/service-com-amazonaws-costexplorer-awsinsightsindexservice). The behavioral check reaches this after its subscription assertions have already passed, so the delete-orphans-subscription semantics themselves are fine; what breaks is the confirm-deleted read.

NOTE on adjacency: the open issue 20261108T004552Z-ce-unknown-monitor-refusals-diverge-404-instead-of.md describes the same handler arm as its Cause 1, but it is grounded in the WIRE fixture cost-explorer-anomaly-monitor-lifecycle#task#13 and carries only that replay key; the replay report marks this behavioral check neither ALREADY FILED nor OWNED by it, so this check is filed here with its own replay key. A fixer closing one should re-replay both.

Cause 1 of 1: GetAnomalyMonitors asserts existence of every ARN in MonitorArnList instead of using it as a filter.
AWS: Smithy costexplorer#GetAnomalyMonitors takes MonitorArnList (target Values, a list of monitor ARNs used as a FILTER; not required), NextPageToken and MaxResults, and returns the REQUIRED output member AnomalyMonitors; it is @paginated (items=AnomalyMonitors, pageSize=MaxResults). Modeled errors: InvalidNextTokenException (400, defaulted), LimitExceededException (400, defaulted), UnknownMonitorException (httpError 404). A filter member selects among the monitors the account holds and asserts nothing about existence, so a request whose MonitorArnList names an ARN the account holds no monitor under is NOT refused: recorded behavior is HTTP 200 with AnomalyMonitors = [] (the required member present and empty, no NextPageToken, no error members). UnknownMonitorException is reserved for operations that address a single monitor (DeleteAnomalyMonitor, UpdateAnomalyMonitor), not for the ARN filter of this paginated read.
Emulator: the call is refused. Over the wire it serves UnknownMonitorException (botocore raises UnknownMonitorException) with the handler-authored message "The cost anomaly monitor <MonitorArn> does not exist for the account." and no AnomalyMonitors member at all. Fixed text of that emulator message is "The cost anomaly monitor " ... " does not exist for the account." with the full request ARN interpolated — it is the UnknownMonitorException Smithy doc sentence, not an AWS error string, and AWS serves NO error here in the first place. The ARN and account id in the replay output are one run's generated values and are not part of any requirement.
Why: the normal delete-then-confirm loop is broken for every client. A caller that deletes a monitor and re-reads the SAME ARN to confirm removal cannot distinguish "confirmed gone" (AWS: 200 + []) from "the request failed" (emulator: a modeled 404 exception), so idempotent teardown, converge/poll loops and cleanup re-runs all fault. Worse, a caller filtering on a MIXED ARN list loses the monitors that DO exist, because the first stale ARN aborts the whole page instead of simply not matching.
Fix: the filter arm of the GetAnomalyMonitors handler — remove the per-ARN registry-membership assertion and let the existing page predicate do the selecting, so an ARN matching nothing contributes no element and the operation answers 200 with the (possibly empty) required AnomalyMonitors list plus the normal cursor. Nothing in the pagination path needs to change.
Traces: behavioral check ce.anomaly-monitor-delete-orphans-subscription. Handler arm: services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/get_anomaly_monitors.rs:180-195 (the pre-page existence loop) versus :199-204 (the page predicate that already filters correctly).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service ce: 2/5 passed. Failing check ce.anomaly-monitor-delete-orphans-subscription, verdict [error] (the other two ce failures are ALREADY FILED stub blocks on TagResource / UpdateAnomalySubscription and are not part of this issue).

Sequence inside the check, all steps before the last one succeeded:
  CreateAnomalyMonitor {AnomalyMonitor:{MonitorName:<n>, MonitorType:DIMENSIONAL, MonitorDimension:SERVICE}} -> 200, MonitorArn minted
  sns CreateTopic / SetTopicAttributes(Policy) -> 200
  CreateAnomalySubscription {MonitorArnList:[<arn>], Subscribers:[{Type:SNS,Address:<topic>}], Frequency:IMMEDIATE, ThresholdExpression:{Dimensions:{Key:ANOMALY_TOTAL_IMPACT_ABSOLUTE,Values:["100"],MatchOptions:[GREATER_THAN_OR_EQUAL]}}} -> 200
  GetAnomalySubscriptions {SubscriptionArnList:[<subArn>]} -> 200, MonitorArnList == [<arn>]
  DeleteAnomalyMonitor {MonitorArn:<arn>} -> 200
  GetAnomalySubscriptions {SubscriptionArnList:[<subArn>]} -> 200, one record, MonitorArnList == [], Subscribers and SubscriptionName unchanged (these assertions PASSED)
  GetAnomalyMonitors {MonitorArnList:[<arn>]} -> DIVERGES

Expected (AWS, and the check's assertion `gone == []`): HTTP 200, body {"AnomalyMonitors": []}.
Actual (emulator): modeled exception raised by botocore —
  botocore.errorfactory.UnknownMonitorException: An error occurred (UnknownMonitorException) when calling the GetAnomalyMonitors operation: The cost anomaly monitor arn:aws:ce::000000000000:anomalymonitor/9e2d1c90-58c9-8566-83b3-f40d90e59de8 does not exist for the account.
No AnomalyMonitors member is serialized at all, although the model marks it required on the output shape.

In that message the fixed text is "The cost anomaly monitor " / " does not exist for the account."; the ARN (account 000000000000 and monitor id 9e2d1c90-...) is a per-run value taken from the request. AWS emits no message here, so no wording is correct for this call — the refusal itself is the defect.

Model facts used (smithy_query on services/service-com-amazonaws-costexplorer-awsinsightsindexservice/smithy-model/com-amazonaws-costexplorer-awsinsightsindexservice.json):
  GetAnomalyMonitors: input {MonitorArnList: list Values, NextPageToken, MaxResults}; output {AnomalyMonitors: list, required:true, NextPageToken}; paginated {inputToken:NextPageToken, outputToken:NextPageToken, items:AnomalyMonitors, pageSize:MaxResults}; errors InvalidNextTokenException(400 defaulted), LimitExceededException(400 defaulted), UnknownMonitorException(404).
  DeleteAnomalyMonitor: input {MonitorArn required}; errors LimitExceededException(400 defaulted), UnknownMonitorException(404).

Code the divergence lands on (citation only):
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/get_anomaly_monitors.rs:180-195 — `for arn in arns { if !store.anomaly_monitors.contains_key(arn) { return Err(UnknownMonitorException{...}) } }`, guarded by a comment asserting the filter "names resources, so an ARN the account holds no monitor for is a 404".
  services/service-com-amazonaws-costexplorer-awsinsightsindexservice/src/ops/get_anomaly_monitors.rs:199-204 — the page call whose predicate `filter.is_none_or(|arns| arns.contains(&monitor.arn))` already implements the correct filter semantics.

## Steps to Reproduce
Boot the emulator from this checkout and run the behavioral check ce.anomaly-monitor-delete-orphans-subscription, or reproduce by hand with any SDK pointed at the emulator's ce endpoint:
  1. ce CreateAnomalyMonitor with AnomalyMonitor = {MonitorName: <unique>, MonitorType: "DIMENSIONAL", MonitorDimension: "SERVICE"} -> note MonitorArn.
  2. ce DeleteAnomalyMonitor with MonitorArn = that ARN -> 200 (the account no longer holds the monitor).
  3. ce GetAnomalyMonitors with MonitorArnList = [that same ARN].
     AWS: HTTP 200, {"AnomalyMonitors": []}.
     Emulator: UnknownMonitorException (status 404) with Message "The cost anomaly monitor <arn> does not exist for the account." and no AnomalyMonitors member.
  4. Variant that shows the data loss: create monitors A and B, delete B, then GetAnomalyMonitors with MonitorArnList = [A, B]. AWS returns 200 with just A; the emulator refuses the whole call, so A is never returned.
