Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.execute-statement-nonexistent-workgroup-not-found redshift-data.execute-statement-invalid-target-combination-rejected redshift-data.describe-statement-invalid-id-and-wait-time-rejected
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.execute-statement-nonexistent-workgroup-not-found|_error_code_and_message#5|AssertionError|raise AssertionError(f"expected a rejection but the call SUCCEEDED, returning {result!r}")
Issue-Key: redshift-data.execute-statement-invalid-target-combination-rejected|run#7|AssertionError|assert msg == _ONE_TARGET_MESSAGE, msg
Issue-Key: redshift-data.describe-statement-invalid-id-and-wait-time-rejected|run#8|AssertionError|assert msg == (
Issue-Commit: 8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6

# redshift-data ExecuteStatement admits a workgroup that does not exist and states the wrong target-selection rule; constraint-violation message quotes the @pattern anchors (3 causes)

> _Note: this issue was identified against the code base at commit `8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Three redshift-data checks in this behavioral replay survived the already-filed/owned filter and cluster into three causes, all on the request-validation surface. This bundle carries THREE causes; all three must be closed before submitting.

NOTE FOR THE FIXER: the open issue 20261022T164520Z-redshift-data-error-responses-diverge-wrong-http-status.md describes the same three root causes from the WIRE corpus (its Cause 1, Cause 5 and Cause 6). The suite did not mark these behavioral checks as OWNED by it — its replay keys are wire fixture#step bindings, not these check ids — so they are filed here with their behavioral grounding. If that issue is fixed first, re-replay the check ids in trace_ids and close this one; the fix location is the same in both.

=== Cause 1 of 3: ExecuteStatement accepts a WorkgroupName that does not exist, answering HTTP 200 SUBMITTED ===
AWS: Smithy (com-amazonaws-redshiftdata-redshiftdata): ExecuteStatementInput.WorkgroupName is an optional WorkgroupNameString (@length 3..128, @pattern for a name or a redshift-serverless workgroup ARN); the existence of the target is service-enforced, and the service-common ValidationException carries @httpError 400. Recorded behavior: ExecuteStatement resolves the named workgroup to a real endpoint in the caller's region before admitting the statement, and refuses a workgroup that was never created with HTTP 400 / error code ValidationException (recorded message, as an example: "Redshift endpoint doesn't exist in this region."), registering no statement.
Emulator: HTTP 200 with a full ExecuteStatement success body — {'Id': '650b9113-...', 'Status': 'SUBMITTED', 'Database': 'dev', 'WorkgroupName': 'sim2real-nonexistent-wg-...', 'RedshiftPid': 1000} — for a workgroup that does not exist.
Why: the most severe cause here. A typo in WorkgroupName, or a workgroup that was never provisioned (or was deleted), is reported as a successfully submitted query. The caller gets an Id for a statement AWS would never have created, its ListStatements gains a record AWS does not create, and every negative test asserting the rejection passes vacuously. The follow-on DescribeStatement/GetStatementResult sequence then runs against compute that is not there.
Fix: ExecuteStatement's handler must resolve the selected target (WorkgroupName, and equally ClusterIdentifier) against the emulator's serverless-workgroup / provisioned-cluster state before admitting the statement, and raise the 400 ValidationException when it does not resolve. The same gate belongs on BatchExecuteStatement, which shares the target-selection input.
Traces: redshift-data.execute-statement-nonexistent-workgroup-not-found.

=== Cause 2 of 3: the target-selection ValidationException states a different rule than AWS's ===
AWS: Smithy: ClusterIdentifier, WorkgroupName and SessionId on ExecuteStatementInput are all optional, so mutual exclusion is service-enforced; ValidationException is @httpError 400. Recorded behavior: AWS answers every bad target selection — none supplied, or more than one supplied — with HTTP 400 and one canonical, request-independent message, 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'. Because that text is fixed and produced by a known rule (at most one of four enumerated targets), it is a requirement rather than an example.
Emulator: HTTP 400 with the correct error code ValidationException but the message 'one of ClusterIdentifier (a provisioned cluster) or WorkgroupName (a serverless workgroup) is required'.
Why: the emulator asserts a different rule than AWS. AWS says at most one of FOUR targets may be given; the emulator says at least one of TWO is required, and does not acknowledge the Glue-resource or SessionId targets at all. A client (or negative test) reading the message to learn why its request was refused is told the wrong rule, and the 'both ClusterIdentifier and WorkgroupName supplied' leg of the check is never reached.
Fix: ExecuteStatement's input validation (shared with BatchExecuteStatement) — make the target-selection check cover all four targets and emit AWS's single canonical message for both the none-supplied and the more-than-one-supplied case.
Traces: redshift-data.execute-statement-invalid-target-combination-rejected.

=== Cause 3 of 3: the constraint-violation message quotes the @pattern with its ^ and $ anchors ===
AWS: Smithy: DescribeStatementRequest.Id targets UUID with @pattern ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$ . This message's rule is known because it is built from the request: AWS emits the standard constraint-violation text "1 validation error detected: Value '<value>' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", and the pattern it quotes is the member's @pattern with the leading ^ and trailing $ stripped. Error code ValidationException, HTTP 400 — both of which the emulator already gets right.
Emulator: HTTP 400, code ValidationException, the right overall text, but the quoted pattern retains the anchors: '... regular expression pattern: ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$'.
Why: lowest severity of the bundle — status and code are correct. But the quoted pattern is the model-derived part of the message, so a client or conformance harness that extracts it and compares it with the service's advertised constraint sees a pattern AWS never reports. It also blocks the rest of this check, whose second leg (WaitTimeSeconds=999 outside the modelled @range 1..30 must be refused ValidationException) is never reached.
Fix: the shared constraint-violation message formatter, not this operation — strip the ^ / $ anchors when rendering a @pattern into a validation message, so every redshift-data member gets AWS's rendering.
Traces: redshift-data.describe-statement-invalid-id-and-wait-time-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. redshift-data 8/24 (33%), 16 failed (13 of those are already-filed scaffold stubs or OWNED by open issues and are excluded here).

Cause 1 — check_redshift_data_execute_statement_nonexistent_workgroup_not_found.py:26
  expected: execute_statement(WorkgroupName='sim2real-nonexistent-wg-...', Database='dev', Sql='SELECT 1') -> rejection (ValidationException, HTTP 400), no statement registered
  actual:   HTTP 200, {'Id': '650b9113-99de-451c-a2f5-6ce59da7365e', 'CreatedAt': ..., 'Database': 'dev', 'WorkgroupName': 'sim2real-nonexistent-wg-e50b8db62141', 'Status': 'SUBMITTED', 'RedshiftPid': 1000}

Cause 2 — check_redshift_data_execute_statement_invalid_target_combination_rejected.py:32
  expected: code ValidationException, Message 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'
  actual:   code ValidationException (correct), Message 'one of ClusterIdentifier (a provisioned cluster) or WorkgroupName (a serverless workgroup) is required'

Cause 3 — check_redshift_data_describe_statement_invalid_id_and_wait_time_rejected.py:30
  expected: "1 validation error detected: Value 'not-a-valid-statement-id' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\\d{0,2})?"
  actual:   same text but with the anchored pattern: '... pattern: ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\\d{0,2})?$'

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json, awsJson1_1, sdkId 'Redshift Data'):
  ExecuteStatementInput.ClusterIdentifier / .WorkgroupName / .SessionId all optional (target mutual exclusion is service-enforced)
  WorkgroupNameString @length 3..128 with the name-or-ARN @pattern
  ValidationException: errorKind=client, httpError=400 (service-common)
  DescribeStatementRequest.Id -> UUID, required, @pattern ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\\d{0,2})?$ ; DescribeStatementRequest.WaitTimeSeconds @range 1..30

## Steps to Reproduce
1. Clone the repo, boot the emulator, and point a botocore redshift-data client (and a redshift-serverless client) at it.
2. Create a namespace and a workgroup and wait for the workgroup status AVAILABLE.
3. Cause 1: execute_statement(WorkgroupName='<a name never created>', Database='dev', Sql='SELECT 1'). Expect HTTP 400 ValidationException and no new statement in list_statements; observe HTTP 200 with Status 'SUBMITTED'.
4. Cause 2: execute_statement(Database='dev', Sql='SELECT 1') with no ClusterIdentifier/WorkgroupName/SecretArn at all. Expect 400 ValidationException with Message 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'; observe the 'one of ... is required' text. Repeat supplying BOTH ClusterIdentifier and WorkgroupName — the same canonical message is expected.
5. Cause 3: describe_statement(Id='not-a-valid-statement-id'). Expect 400 ValidationException whose message quotes the UUID pattern WITHOUT the ^ / $ anchors; observe the anchored pattern. Then describe_statement(Id=<a valid finished id>, WaitTimeSeconds=999) and expect 400 ValidationException 'Value '999' at 'waitTimeSeconds' failed to satisfy constraint: Member must have value less than or equal to 30'.
6. Re-replay every check id in trace_ids to confirm a fix.
