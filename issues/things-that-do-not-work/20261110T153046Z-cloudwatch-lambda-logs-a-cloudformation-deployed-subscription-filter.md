Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# cloudwatch__lambda__logs: a CloudFormation-deployed subscription filter never forwards a log event to the destination function, so the positive deployment-order check reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: the stack's AWS::Logs::SubscriptionFilter, created together with the destination function and its logs.amazonaws.com permission, never delivers.
AWS: the stack reaches CREATE_COMPLETE, and an event written to the source log group that matches the filter pattern is forwarded to the destination function as a gzipped base64 'awslogs.data' payload within seconds; the function's own log group then shows the event's marker. Recorded behavior only for the payload framing beyond the awslogs envelope.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'; neither the stack settling nor the forwarded invocation is ever observed.
Why: log-driven Lambda processing deployed by template silently never runs, and a customer polling the destination function's logs waits forever with no refusal to explain it.
Fix: the logs subscription-filter delivery path to a Lambda destination (and the cloudformation AWS::Logs::SubscriptionFilter handler that wires it).
Traces: cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: stack CREATE_COMPLETE, then the marker written to the source group appearing in the destination function's log group after it decodes event['awslogs']['data']. Actual: no delivery and no verdict inside the cap.

## Steps to Reproduce
Replay cloudwatch__lambda__logs.cfn-deploy-logs-subscription-filter-lambda-positive: deploy the template (source log group, destination function, AWS::Lambda::Permission for logs.amazonaws.com, AWS::Logs::SubscriptionFilter with pattern MARKER), PutLogEvents a matching line, then read the function's log group.
