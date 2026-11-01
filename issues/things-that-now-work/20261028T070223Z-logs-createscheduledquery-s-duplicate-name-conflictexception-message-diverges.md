Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.conflict-exception-concurrent-state
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.conflict-exception-concurrent-state|run#56|AssertionError|assert msg == f"Scheduled query with identifier {name} already exists", (
Issue-Commit: 7da7036bd0c3fb1b6531d1fc1180304bed725812

# logs: CreateScheduledQuery's duplicate-name ConflictException message diverges from AWS's wording (1 cause)

> _Note: this issue was identified against the code base at commit `7da7036bd0c3fb1b6531d1fc1180304bed725812`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScheduledQuery

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the duplicate-name ConflictException carries the emulator's own sentence instead of AWS's message for this error class ===

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) models CreateScheduledQuery.name as @required targeting ScheduledQueryName (@length 1..300) and binds ConflictException (client error, HTTP 400) to the operation, documented as "This operation attempted to create a resource that already exists". A CreateScheduledQuery under a name the account already holds a scheduled query under is refused with ConflictException, and the message AWS returns for this class is "Scheduled query with identifier <identifier> already exists" — recorded behavior for the message text. Message rule: the fixed text is "Scheduled query with identifier " + " already exists"; the only variable part is the identifier, which is the `name` member taken verbatim from the request (no ARN, account id or timestamp appears in it). This is the same identifier-phrasing family the service uses for the not-found case of the same resource ("Scheduled query with identifier <identifier> not found").

Emulator: the refusal classification is right (ConflictException, HTTP 400) but the message is the emulator's own sentence, "A scheduled query named <name> already exists". Replay reported: AssertionError: unexpected duplicate-name CreateScheduledQuery message: 'A scheduled query named sim2real-behav-sq-conflict-060810c544ce already exists' (the suffix in that string is this run's generated name, not part of the requirement). The message is constructed in the handler's name-collision branch rather than from AWS's template for the class.

Why: a client that distinguishes conflict causes by message text — CLI/SDK wrappers that surface the message to users, IaC and CDK/CloudFormation custom-resource handlers that match the "Scheduled query with identifier ... already exists" phrasing to decide between adopt-existing and fail, and test suites asserting the error contract — sees a message no AWS error ever produces. It must either hard-code the emulator's wording or fall through to a generic failure path, which is exactly the divergence that makes emulator-backed negative-path tests non-transferable to AWS.

Fix: the name-collision branch of the CreateScheduledQuery handler (the operation's validation stage, before any store mutation): build the ConflictException message from AWS's template for this class, interpolating only the requested identifier. The state behavior around it is already correct — the pre-existing query must remain untouched and no record may be inserted on the refused path. Note the open issue 20261027T002339Z-logs-createscheduledquery-stores-a-malformed-cron-scheduleexpression-and.md touches the same handler but is a different cause (no scheduleExpression format validation at all); it does not cover this message.

Traces: logs.conflict-exception-concurrent-state.

## Evidence
Failing check `logs.conflict-exception-concurrent-state` (verdict: mismatch).

Sequence (all earlier legs of the check passed): CreateLogGroup <group>; create an execution role trusted by logs.amazonaws.com; CreateScheduledQuery name=<new-name> state=ENABLED queryLanguage=CWLI queryString="fields @timestamp, @message | limit 10" scheduleExpression="cron(0 12 * * ? *)" startTimeOffset=3600 executionRoleArn=<role> logGroupIdentifiers=[<group ARN>] -> accepted; then a SECOND CreateScheduledQuery with the SAME name and a different queryString ("fields @timestamp | limit 1").

Expected (AWS): HTTP 400, error code `ConflictException`, message exactly "Scheduled query with identifier <name> already exists" — fixed text "Scheduled query with identifier " + " already exists", variable part = the `name` from the request.

Actual (emulator): HTTP 400 with error code `ConflictException` (correct) but message "A scheduled query named <name> already exists". Replay output: `AssertionError: unexpected duplicate-name CreateScheduledQuery message: 'A scheduled query named sim2real-behav-sq-conflict-060810c544ce already exists'` (check_logs_conflict_exception_concurrent_state.py line 259, asserting `msg == f"Scheduled query with identifier {name} already exists"`). The name in that line is this run's generated value.

Handler evidence: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_scheduled_query.rs lines 214-224 — the `store.scheduled_queries.contains_key(&input.name)` branch returns ConflictException with `format!("A scheduled query named {} already exists", input.name)`.

## Steps to Reproduce
1. logs CreateLogGroup logGroupName=<group>; read its ARN with DescribeLogGroups.
2. Create an IAM role whose trust policy allows logs.amazonaws.com to assume it (scheduled-query execution role).
3. logs CreateScheduledQuery name=<new-name> state=ENABLED queryLanguage=CWLI queryString="fields @timestamp, @message | limit 10" scheduleExpression="cron(0 12 * * ? *)" startTimeOffset=3600 executionRoleArn=<role ARN> logGroupIdentifiers=[<group ARN>]. Expect 200.
4. Repeat step 3 with the SAME name but queryString="fields @timestamp | limit 1".
   Observed: HTTP 400 ConflictException with message "A scheduled query named <new-name> already exists".
   Expected: HTTP 400 ConflictException with message "Scheduled query with identifier <new-name> already exists".
5. Positive control (must keep working): logs GetScheduledQuery identifier=<new-name> still reports state=ENABLED and queryString="fields @timestamp, @message | limit 10" — the refused duplicate must not overwrite the stored query.
