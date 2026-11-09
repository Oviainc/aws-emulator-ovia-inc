Issue-Source: trace-sweep-divergence
Issue-Trace: logs-scheduled-query-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-scheduled-query-workflow logs-scheduled-query-lifecycle logs-scheduled-query-error-paths logs-scheduled-query-errors
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateScheduledQuery|4xx->2xx|$.__type
Issue-Key: logs|CreateScheduledQuery|4xx->2xx|$.message
Issue-Key: logs|CreateScheduledQuery|4xx->2xx|status
Issue-Key: logs|GetScheduledQuery|4xx->2xx|$.__type
Issue-Key: logs|GetScheduledQuery|4xx->2xx|$.message
Issue-Key: logs|GetScheduledQuery|4xx->2xx|status
Issue-Key: logs|UpdateScheduledQuery|4xx->4xx|$.message
Issue-Key: logs|CreateScheduledQuery|4xx->4xx|$.message
Issue-Key: logs|GetScheduledQuery|4xx->4xx|$.message
Issue-Key: logs|GetScheduledQuery|2xx->2xx|$.endTimeOffset
Issue-Key: logs|UpdateScheduledQuery|2xx->2xx|$.endTimeOffset
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs scheduled-query operations accept invalid input and rewrite not-found text (7 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledQuery

## Description
Seven root causes on the scheduled-query surface: two calls AWS refuses are accepted, one read omits a modeled member, and four refusal texts are emulator-authored or mis-attributed. Cause count: 7 — all seven must be closed. Related open issues cover different causes and carry different replay keys: 20261027T002339Z-logs-createscheduledquery-stores-a-malformed-cron-scheduleexpression-and.md (scheduleExpression), 20261028T070223Z-logs-createscheduledquery-s-duplicate-name-conflictexception-message-diverges.md and cause 10 of 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md (duplicate-name wording from the behavioral corpus).

=== Cause 1 of 7: CreateScheduledQuery accepts an executionRoleArn that is not an IAM role ARN ===
AWS: refuses with HTTP 400 ValidationException and the standard constraint message "1 validation error detected: Value at 'executionRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(?:aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::([0-9]{12}):role\\/(.+)" — fixed template, variable parts the member name and the pattern (rendered unanchored, as AWS does). Smithy: CreateScheduledQuery.executionRoleArn is required (RoleArn) and the op binds ValidationException (httpError 400).
Emulator: answers HTTP 200 and creates the scheduled query.
Why: a scheduled query is created with a role ARN AWS would reject, so the whole lifecycle diverges from step one and the client never learns its role ARN is malformed.
Fix: the executionRoleArn validation in the CreateScheduledQuery handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-scheduled-query-workflow#task#3.

=== Cause 2 of 7: GetScheduledQuery by ARN for a deleted/absent query answers 200 ===
AWS: HTTP 400 ResourceNotFoundException with 'Scheduled query with identifier <identifier> not found' — fixed text plus one variable part, the identifier exactly as supplied (here the scheduled-query ARN). Smithy: GetScheduledQuery.identifier (@length 1..300) accepts a name OR an ARN, and the op binds ResourceNotFoundException (httpError 400).
Emulator: HTTP 200 with a body — the ARN form of the identifier is not resolved against deleted state.
Why: a client that deletes a scheduled query and then confirms the deletion by ARN sees the query still there; delete-verify loops never terminate correctly.
Fix: the identifier resolution (name and ARN forms) and the not-found branch of the GetScheduledQuery handler.
Traces: logs-scheduled-query-lifecycle#task#14 (also logs-scheduled-query-workflow#task#4).

=== Cause 3 of 7: UpdateScheduledQuery misattributes a missing LOG GROUP as a missing scheduled query ===
AWS: when the update names a log group that does not exist the refusal is about the log group: "Log group '<logGroupName>' does not exist for account ID '<accountId>' (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <requestId>; Proxy: null)" — fixed text plus the group name, the account id and the per-run request id (the request id is the only per-run part).
Emulator: 'The specified scheduled query does not exist.' — it blames the wrong resource.
Why: the caller retries creating the scheduled query instead of creating the missing log group, so the workflow cannot recover.
Fix: the log-group existence check (and its message) in the UpdateScheduledQuery handler, ahead of the scheduled-query lookup.
Traces: logs-scheduled-query-error-paths#task#3 (also logs-scheduled-query-errors#task#5, logs-scheduled-query-lifecycle#task#11).

=== Cause 4 of 7: CreateScheduledQuery's duplicate-name conflict text is emulator-authored ===
AWS: ConflictException (HTTP 400) with 'Scheduled query with identifier <name> already exists' — fixed text plus the `name` from the request.
Emulator: 'A scheduled query named <name> already exists'.
Why: tooling that matches AWS's 'with identifier ... already exists' phrasing to choose between adopt-existing and fail does not recognise the conflict.
Fix: the name-collision branch of the CreateScheduledQuery handler.
Traces: logs-scheduled-query-errors#task#4 (also logs-scheduled-query-lifecycle#task#9).

=== Cause 5 of 7: GetScheduledQuery's not-found text is emulator-authored ===
AWS: 'Scheduled query with identifier <identifier> not found' — fixed text plus the identifier from the request.
Emulator: 'The specified scheduled query does not exist.' — the identifier is dropped.
Why: a caller polling several scheduled queries cannot tell which identifier was missing.
Fix: the not-found branch of the GetScheduledQuery handler (same text as cause 2).
Traces: logs-scheduled-query-lifecycle#task#10.

=== Cause 6 of 7: GetScheduledQuery omits endTimeOffset ===
AWS: returns endTimeOffset — the effective value, 0 when CreateScheduledQuery omitted it. Smithy: GetScheduledQuery output endTimeOffset (EndTimeOffset, long).
Emulator: the member is absent.
Why: a client reading a scheduled query back to re-apply it loses the query window offset and silently changes the schedule's semantics on update.
Fix: default endTimeOffset at create time and serialize it in GetScheduledQuery.
Traces: logs-scheduled-query-lifecycle#task#4 (also #task#6).

=== Cause 7 of 7: UpdateScheduledQuery's success response omits endTimeOffset ===
AWS: the update response echoes the same resolved configuration, including endTimeOffset 0.
Emulator: the member is absent.
Why: a configure-then-verify client cannot confirm the offset survived the update.
Fix: the UpdateScheduledQuery serializer (same defaulting as cause 6).
Traces: logs-scheduled-query-lifecycle#task#5.

## Evidence
Cause 1: "status: expected 400, got 200; $.__type: missing (expected 'ValidationException', got None); $.message: missing (expected \"1 validation error detected: Value at 'executionRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(?:aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::([0-9]{12}):role\\/(.+)\", got None)".
Cause 2: "status: expected 400, got 200; $.__type: missing (expected 'ResourceNotFoundException', got None); $.message: missing (expected 'Scheduled query with identifier arn:aws:logs:us-west-2:000000000000:scheduled-query:<name> not found', got None)" — the ARN is the variable part.
Cause 3: "$.message: value_mismatch (expected \"Log group '<logGroupName>' does not exist for account ID '000000000000' (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <requestId>; Proxy: null)\", got 'The specified scheduled query does not exist.')" — 3 affected steps; the request id inside the suffix is per-run.
Cause 4: "$.message: value_mismatch (expected 'Scheduled query with identifier <name> already exists', got 'A scheduled query named <name> already exists')" — 2 affected steps.
Cause 5: "$.message: value_mismatch (expected 'Scheduled query with identifier <name> not found', got 'The specified scheduled query does not exist.')".
Cause 6: "$.endTimeOffset: missing (expected 0, got None)" on GetScheduledQuery — 2 affected steps.
Cause 7: "$.endTimeOffset: missing (expected 0, got None)" on UpdateScheduledQuery.

## Steps to Reproduce
1: CreateScheduledQuery(..., executionRoleArn='<a string that is not an arn:...:iam::<12 digits>:role/... ARN>') — expect 400 ValidationException with the executionRoleArn pattern message (logs-scheduled-query-workflow#task#3).
2: CreateScheduledQuery, DeleteScheduledQuery, then GetScheduledQuery(identifier=<the scheduled-query ARN>) — expect 400 ResourceNotFoundException 'Scheduled query with identifier <arn> not found' (logs-scheduled-query-lifecycle#task#14).
3: UpdateScheduledQuery on an existing scheduled query with logGroupIdentifiers naming a log group that does not exist — expect the log-group ResourceNotFoundException text (logs-scheduled-query-error-paths#task#3).
4: CreateScheduledQuery twice with the same name — compare the ConflictException message (logs-scheduled-query-errors#task#4).
5: GetScheduledQuery(identifier=<a name never created>) — expect 'Scheduled query with identifier <name> not found' (logs-scheduled-query-lifecycle#task#10).
6/7: CreateScheduledQuery without endTimeOffset, then GetScheduledQuery and UpdateScheduledQuery — assert endTimeOffset == 0 in both responses (logs-scheduled-query-lifecycle#task#4, #task#5).
