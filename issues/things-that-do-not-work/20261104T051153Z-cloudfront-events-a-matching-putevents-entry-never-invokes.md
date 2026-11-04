Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.cloudfront-distribution-event-to-lambda-invoked cloudfront__events.cloudfront-event-target-missing-lambda-permission-not-invoked
Issue-Key: cloudfront__events.cloudfront-distribution-event-to-lambda-invoked|_poll_logs_for_marker#9|Timeout|time.sleep(3.0)
Issue-Key: cloudfront__events.cloudfront-event-target-missing-lambda-permission-not-invoked|_poll_logs_for_marker#11|Timeout|time.sleep(3.0)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudfront__events: a matching PutEvents entry never invokes the rule's Lambda target, so no function log output ever appears and a denied target is indistinguishable from a permitted one

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: a PutEvents entry that matches an ENABLED rule's event pattern never invokes the rule's Lambda target, so the function never runs and its CloudWatch Logs log group never receives the handler's output. Both checks observe the invocation only through `/aws/lambda/<function>` and therefore hang to the harness cap.
Failing checks: cloudfront__events.cloudfront-distribution-event-to-lambda-invoked, cloudfront__events.cloudfront-event-target-missing-lambda-permission-not-invoked.

AWS: PutEvents places the entry on the bus (PutEventsResponse reports FailedEntryCount 0 and an EventId per entry); EventBridge then matches it against the bus's rules and, for each matching ENABLED rule, delivers the full event envelope to every configured target. For a Lambda target whose resource policy grants Principal events.amazonaws.com the action lambda:InvokeFunction with SourceArn = the rule ARN (AddPermission), EventBridge invokes the function asynchronously, normally within seconds; the function's stdout is written to the log group `/aws/lambda/<function-name>`, so FilterLogEvents on that group returns the handler's line. The envelope the handler receives carries `source`, `detail-type` and `detail` exactly as PutEvents supplied them (here source `sim2real.cloudfront`, detail-type `CloudFront Distribution State Change`, detail.distributionId and detail.state unchanged). A sibling target on the SAME rule whose function has NO such resource-policy grant is NOT invoked — that is how the permission boundary is observable: one function's log group gets the line, the other's never does. Recorded behavior only for the delivery latency and the log-group population; Smithy (eventbridge) models PutEvents -> PutEventsResponse{FailedEntryCount, Entries}, and nothing in the model expresses the delivery itself.

Emulator: PutEvents is accepted, but nothing is ever invoked: FilterLogEvents on `/aws/lambda/<function>` keeps answering ResourceNotFoundException / an empty `events` list for the whole poll, and both checks end `Check timed out after 60.0s` (the inner poll budget is 120 s). On the negative check neither the permitted nor the denied target produces anything, so the assertion that only the permitted one ran can never be reached.

Why: a client whose automation is PutEvents -> rule -> Lambda sees its events accepted and silently dropped — no invocation, no log output, no error. And because the denied target behaves exactly like the permitted one (both silent), the resource-policy boundary that is supposed to block delivery cannot be verified at all; a misconfigured production rule would look identical to a correct one.

Fix: the EventBridge rule-target delivery path in the events crate — pattern match -> per-target dispatch -> Lambda Invoke, honouring the target function's resource-based policy so an ungranted target is skipped while its sibling is invoked — in services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs, plus the Lambda invoke path it drives, which must run the function and emit its output into `/aws/lambda/<function-name>` so a logs reader observes the invocation. Related open issues own neighbouring checks but NOT these two replay keys: 20261102T091831Z-cloudfront-events-a-matched-rule-never-invokes-its.md, 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md, 20261101T214154Z-cloudfront-events-two-more-root-causes-a-matched.md, and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (the Lambda side of the same path).

Traces: the two failing check ids above; both fail in _poll_logs_for_marker after PutEvents.

## Evidence
Expected (AWS): after put_events(Entries=[{Source:"sim2real.cloudfront", DetailType:"CloudFront Distribution State Change", Detail:{distributionId:<marker>, state:"Deployed", eTag:…}}]) on a rule whose pattern matches source/detail-type/detail.state, FilterLogEvents on /aws/lambda/<fn> returns a message containing `EVENT_MARKER:` whose JSON has source == "sim2real.cloudfront", detail-type == "CloudFront Distribution State Change", detail.distributionId == the marker and detail.state == "Deployed"; on the two-target rule, only the function granted events.amazonaws.com invoke rights produces such a line.
Actual (emulator): no log group content ever appears for either function — FilterLogEvents answers ResourceNotFoundException/empty for the whole poll and both checks terminate with `Check timed out after 60.0s`. No error is surfaced by PutEvents, PutTargets or AddPermission; the delivery simply never happens.

## Steps to Reproduce
1. Create an IAM role for Lambda, create a python3.12 function that prints `EVENT_MARKER:` + the event JSON, and wait for State Active.
2. events.put_rule(Name=…, EventPattern={"source":["sim2real.cloudfront"],"detail-type":["CloudFront Distribution State Change"],"detail":{"state":["Deployed"]}}, State="ENABLED").
3. lambda.add_permission(FunctionName=…, Principal="events.amazonaws.com", Action="lambda:InvokeFunction", SourceArn=<rule arn>), then events.put_targets with that function ARN. For the negative case add a second target function WITHOUT the add_permission grant.
4. events.put_events a matching entry, then poll logs.filter_log_events(logGroupName="/aws/lambda/<fn>").
5. On AWS the permitted function's log group gets the EVENT_MARKER line (and the ungranted one never does); on the emulator neither log group ever produces anything and the check times out.
