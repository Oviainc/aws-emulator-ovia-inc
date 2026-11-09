Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target
Issue-Step: task#20
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: logs|FilterLogEvents|2xx->4xx|$.events
Issue-Key: logs|FilterLogEvents|2xx->4xx|$.searchStatistics
Issue-Key: logs|FilterLogEvents|2xx->4xx|$.searchedLogStreams
Issue-Key: logs|FilterLogEvents|2xx->4xx|status
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# CloudWatch Logs FilterLogEvents answers HTTP 400 for a Lambda log group that exists, so no invocation events can be read

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FilterLogEvents

## Description
1 net-new cause, in the FilterLogEvents handler's log-group resolution.

Cause 1 of 1: FilterLogEvents refuses a read of the log group a Lambda function wrote to.
AWS: Smithy: FilterLogEvents takes logGroupName (@length 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$) or logGroupIdentifier and returns FilterLogEventsResponse {events: FilteredLogEvents, searchedLogStreams: SearchedLogStreams, nextToken}; its only client-side failure classes are InvalidParameterException (400) for a malformed request and ResourceNotFoundException (400) for a log group that does not exist. Neither applies here: the group is the one implicitly created for the Lambda function invoked earlier in the same fixture, and the recording shows AWS answering HTTP 200 with the invocation's event in `events` (message, timestamp, eventId, ingestionTime and logStreamName all per-run values), an empty `searchedLogStreams` list and a `searchStatistics` block of {approximateTotalLogStreamCount, completedLogStreamCount}. A log group that exists but has no matching events is still a 200 with an empty `events` list, never a 400.
Emulator: answers HTTP 400 — the whole success body is absent ($.events, $.searchedLogStreams and $.searchStatistics all missing) — for a request naming that existing group.
Hypothesis: the log group created implicitly on behalf of a Lambda function is not registered in the Logs service's state, so every Logs read/mutation that resolves it by name fails; the already-open issue 20261103T165236Z-logs-cloudwatch-logs-deleteloggroup-refuses-an-existing-log-group.md reports the matching 400 from DeleteLogGroup on the same class of group (and that issue owns the DeleteLogGroup step of this very fixture, teardown#26 — it does not carry this FilterLogEvents key).
Why: FilterLogEvents is the standard way a client confirms that a function ran and what it logged, so a 400 makes log-based verification impossible and is indistinguishable, to an SDK, from "you asked for a group that does not exist" or "your parameters are invalid". A caller that retries on 400 loops, and one that treats 400 as fatal aborts a run whose Lambda actually executed.
Fix: the FilterLogEvents handler's log-group resolution and its 400-vs-200 decision — a group that exists (including one auto-created for a Lambda function) must be read successfully, returning 200 with `events` (empty when nothing matches the filter/time window), `searchedLogStreams` and the search-statistics block, and reserving ResourceNotFoundException for a group that genuinely does not exist.
Traces: elasticloadbalancing-alb-forwards-health-checks-to-lambda-target#task#20.

## Evidence
elasticloadbalancing-alb-forwards-health-checks-to-lambda-target#task#20 — FilterLogEvents: status expected 200, got 400. divergence_key service=logs|FilterLogEvents|status=200->400|keys=$.events,$.searchStatistics,$.searchedLogStreams,status.
Expected body (recording): $.events = [{eventId, ingestionTime, logStreamName, timestamp all shaped/not asserted, message 'sim2real-alb-invoke {"method": "GET", "path": "/health", "target_group": "<target-group-arn>", "user_agent": "ELB-HealthChecker/2.0"}\n'}], $.searchedLogStreams = [], $.searchStatistics = {approximateTotalLogStreamCount: 0, completedLogStreamCount: 0}.
Actual: HTTP 400, none of those members present.
Corroborating (already owned, listed only as context, not filed here): the same fixture's teardown#26 DeleteLogGroup on the same group also answers 400 instead of 200 — owned by 20261103T165236Z-logs-cloudwatch-logs-deleteloggroup-refuses-an-existing-log-group.md.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture elasticloadbalancing-alb-forwards-health-checks-to-lambda-target up to task#20: the ALB health checker invokes the Lambda target, which writes a log line to its /aws/lambda/<function-name> log group.
3. Issue that step's FilterLogEvents against that log group: the emulator answers HTTP 400 where the recording has HTTP 200 with the invocation event in `events`.
