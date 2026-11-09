Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage|_poll_log_events#16|Timeout|_wait_for_200(traffic_url, timeout_s=30)
Issue-Commit: 4d52475541f700a1053a505adbc9b4b259e3075a

# API Gateway: the per-stage access-log isolation scenario reaches no verdict — two stages of one REST API with their own accessLogSettings destinations never converge within the 180 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `4d52475541f700a1053a505adbc9b4b259e3075a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One net-new root cause. The closest open issues — 20261108T120220Z-apigateway-dynamodb-lambda-logs-a-rest-stage-with.md and 20261109T033427Z-apigateway-dynamodb-lambda-logs-apigateway-a-lambda-proxy-rest-stage-with-accesslogsettings.md (a REST stage with AccessLogSettings delivers no access-log events), 20261107T212447Z-apigateway-dynamodb-lambda-logs-rest-stage-access-logging-never-converges-enabling-accesslogsettings.md, and 20261107T070300Z-apigateway-dynamodb-lambda-logs-cloudformation-deployed-rest-api-stages-are-unservable-the.md (deployed stage invoke URLs answer 404 UnknownService) — carry other checks and do NOT carry this one; the suite reports it unowned. It is filed here because losing it would lose the only check that grades access-log ISOLATION between two stages of the same API.

Cause 1 of 1: the per-stage access-log scenario never reaches a verdict.
AWS: recorded behavior only (the Smithy model states the shape, not the delivery). A REST API with two deployed stages, each stage's accessLogSettings naming its OWN pre-created CloudWatch Logs group (destinationArn arn:aws:logs:<region>:<account>:log-group:<name>) and a custom format '{"requestId":"$context.requestId","stage":"$context.stage"}', behaves as follows after the account's cloudwatchRoleArn is set to a role trusting apigateway.amazonaws.com with AmazonAPIGatewayPushToCloudWatchLogs: requests to stage A's invoke URL answer HTTP 200 from the Lambda-proxy backend; within seconds each request appears as exactly one access-log event in stage A's OWN log group, with $context.stage rendered as `stagea`; stage B's log group receives no event and — the second, independent observable — never has a log stream opened in it at all (DescribeLogStreams returns an empty logStreams list for an untrafficked stage's destination).
Emulator: the check reaches no verdict at all. It exhausts the harness cap and is reported `Error: Check timed out after 180.0s`, with no refusal, no status and no partial result to grade. The only two unbounded waits in the body are (a) `_wait_for_200` on the deployed stage A invoke URL and (b) the 180 s `_poll_log_events` on stage A's dedicated group, so the emulator either never serves 200 from the deployed stage or never delivers an access-log event to the configured destination. Hypothesis: it is the same access-log delivery gap the related open issues record, extended to a two-stage API; the timeout is what makes it net-new, because nothing graded the isolation half of the scenario.
Why: a client cannot observe which stage produced a request. Multi-stage deployments (dev/prod on one API) route their access logs to separate groups and separate retention/subscription pipelines; with no delivery at all, per-stage dashboards, metric filters and subscription filters built on those groups stay permanently empty, and — because the absence half of the check also never runs — a cross-stage leak would be indistinguishable from the correct empty result. A caller blocked on waiting for an access-log record hangs rather than failing.
Fix: the REST stage access-logging delivery path — the stage's accessLogSettings (destinationArn + format) must be resolved per stage when a request is served on that stage's invoke URL, and one rendered record emitted to that stage's log group only (creating the stream on first delivery, and creating nothing in a stage's group that served no traffic). The serving of a deployed stage's invoke URL is the upstream precondition in the same path.
Traces: apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage.

## Evidence
Failing check apigateway__dynamodb__lambda__logs.apigw-access-log-group-created-per-stage:
  Error: Check timed out after 180.0s
(no exception, no HTTP status, no refusal — the check produced no verdict)

What the scenario asserts, and therefore what is expected:
  1. GET <stage A invoke URL>/echo answers 200 (asserted 5 times): `assert status == 200, f"expected GET {url_a} to return 200, got {status}"`.
  2. `assert len(events_a) > 0, "stage A traffic never reached stage A's dedicated log group"` — at least one access-log event in stage A's group within 180 s.
  3. Each such event parses as JSON with `parsed["stage"] == "stagea"`.
  4. `assert len(events_b) == 0, "stage A traffic leaked into stage B's log group"` over a window of max(3x the measured stage-A delivery latency, 15 s).
  5. `assert not streams_b, "stage B's log group opened a log stream despite no traffic"` — DescribeLogStreams on stage B's group returns an empty list.
Actual: the run never gets past step 1 or 2 inside the 180 s cap, so assertions 3-5 are never reached.

## Steps to Reproduce
1. iam CreateRole with a trust policy for apigateway.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs, and apigateway UpdateAccount to set cloudwatchRoleArn to it.
2. iam CreateRole for a Lambda execution role; lambda CreateFunction for a python echo handler returning {'statusCode': 200, 'body': json.dumps({'ok': True, 'path': event.get('path')})}.
3. apigateway CreateRestApi with an AWS_PROXY method integration on /echo pointing at that function.
4. For each of the two stage names `stagea` and `stageb`: CreateDeployment into that stage; logs CreateLogGroup for a distinct group name; apigateway UpdateStage setting accessLogSettings.destinationArn to arn:aws:logs:<region>:<account>:log-group:<that group> and accessLogSettings.format to '{"requestId":"$context.requestId","stage":"$context.stage"}'.
5. GET <stagea invoke URL>/echo five times; each must answer 200.
6. logs FilterLogEvents on stage A's group from just before the traffic. Expected: at least one event, each parsing as JSON with "stage":"stagea". Observed: nothing arrives and the check is killed at the 180 s cap.
7. logs FilterLogEvents on stage B's group over max(3x the stage-A delivery latency, 15 s): expected zero events. logs DescribeLogStreams on stage B's group: expected an empty logStreams list.
