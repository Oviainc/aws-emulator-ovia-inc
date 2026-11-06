Issue-Source: trace-sweep-divergence
Issue-Trace: logs-insights-query-single-log-group
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-insights-query-single-log-group
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[arn="arn:aws:logs:us-west-2:000000000000:log-group:sim2real-probe-logs-logs-insights-lg-b4142248:*"].storedBytes
Issue-Commit: e0e096f7847ddbab99498abf6c574ac0da8fbc09

# DescribeLogGroups reports live-summed storedBytes instead of the metered value AWS serves

> _Note: this issue was identified against the code base at commit `e0e096f7847ddbab99498abf6c574ac0da8fbc09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: DescribeLogGroups.logGroups[].storedBytes is computed by the emulator as a live sum of the bytes of the log events ingested into the group, so a read taken right after PutLogEvents reports a non-zero size. AWS does not serve storedBytes that way.

AWS: Smithy (com-amazonaws-cloudwatchlogs-logs-20140328): LogGroup.storedBytes is documented only as "The number of bytes stored." — the model states no relationship to just-ingested events, so this is recorded behavior only. The recording shows the same DescribeLogGroups call, issued after log events were put into the group and after an Insights query over it, returning HTTP 200 with storedBytes = 0 for that group: AWS derives storedBytes from asynchronous metering of the group's archived volume, not from the events accepted in the current request sequence, so a freshly written group still reads as 0 bytes until metering catches up. The only requirement here is the response field's value semantics; the status (200) and the rest of the LogGroup shape already match.

Emulator: for the same step the emulator answers 200 but reports storedBytes = 270 for the group addressed by logGroupIdentifier/ARN, i.e. the byte total of the events just ingested. Replay detail: `$.logGroups[arn="...:log-group:<group>:*"].storedBytes: value_mismatch (expected 0, got 270)`. The 270 is this recording's byte total and is not itself the requirement — the requirement is that the field does not track the just-ingested events.

Why: a client that polls DescribeLogGroups to decide whether a group has been metered, to drive retention/size dashboards, or to assert "newly created group is still empty" sees storage appear instantly against the emulator and never against AWS. Code that waits for storedBytes to settle, or that compares storedBytes before/after an ingest to infer metering lag, takes the opposite branch on the emulator; byte totals computed from the emulator do not correspond to any value AWS would have reported at that point in the sequence.

Fix: the DescribeLogGroups handler and the log-group state it reads in the logs service crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/ for the handler, src/state.rs for the per-group accounting). The stored-bytes figure served on the describe path must not be the running sum maintained by the PutLogEvents ingest path; it must be the metered figure, which for a group whose events were written in the same session is 0. Other LogGroup members on this response already match and must not change.

Traces: logs-insights-query-single-log-group#task#5 (DescribeLogGroups, expected 200 / actual 200, storedBytes 0 vs 270). No open issue covers this field — the DescribeLogGroups issues already filed are 20261102T090835Z-ec2-logs-s3-a-create-complete-flow-log-stack-s-cloudwatch.md and 20261106T160934Z-cloudwatch-lambda-logs-a-successful-lambda-invoke-never.md, which are both about a log group being absent from the listing, not about a present group's storedBytes value, and 20261022T220153Z-logs-describeloggroups-omits-bearertokenauthenticationenabled-and-kmskeyid-from-every-log.md is resolved and concerned different members.

## Evidence
Fixture logs-insights-query-single-log-group, step task#5, operation DescribeLogGroups.
Expected status 200; actual status 200 (status matches).
Body divergence (verbatim from replay): $.logGroups[arn="arn:aws:logs:us-west-2:000000000000:log-group:sim2real-probe-logs-logs-insights-lg-b4142248:*"].storedBytes: value_mismatch (expected 0, got 270)
Recorded AWS value for the field: 0, on a group whose log events were written earlier in the same fixture.
Emulator value: 270 — the summed size of those events.
Smithy: LogGroup.storedBytes documented as "The number of bytes stored."; no model constraint ties it to the current request sequence (recorded behavior only).
Cluster: divergence_key service=logs|DescribeLogGroups|status=200->200|keys=$.logGroups[...].storedBytes, affected_count 1, no owning open issue, no stub.

## Steps to Reproduce
1. Create a log group and a log stream in it (CreateLogGroup / CreateLogStream).
2. PutLogEvents a small batch of events into that stream.
3. Optionally run a Logs Insights query over the group, as the fixture does.
4. Call DescribeLogGroups selecting that group (by name prefix or log-group identifier/ARN).
5. Inspect logGroups[].storedBytes for the group: AWS returns 0 (metering has not run), the emulator returns the byte total of the events put in step 2 (270 in the recording).
Replay: replay_wire_corpus and look at fixture logs-insights-query-single-log-group, step task#5.
