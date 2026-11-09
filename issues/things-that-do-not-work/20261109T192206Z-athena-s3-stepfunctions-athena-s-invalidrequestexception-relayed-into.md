Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3__stepfunctions.get-query-execution-for-an-unknown-id-fails-as-task-failed
Issue-Key: athena__s3__stepfunctions.get-query-execution-for-an-unknown-id-fails-as-task-failed|run#9|AssertionError|assert masked == (
Issue-Commit: fdb8a4e953f01d6c4e0b3bc9b573d323716eabbc

# athena__s3__stepfunctions: Athena's InvalidRequestException relayed into a Step Functions task cause omits the "Request ID: <id>;" segment of the AWS error envelope

> _Note: this issue was identified against the code base at commit `fdb8a4e953f01d6c4e0b3bc9b573d323716eabbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
Cause 1 of 1: the service-error envelope Step Functions relays as a failed task's `cause` is missing its Request ID segment, because Athena's refusal does not carry a request id into the integration's error text.
Failing check id: athena__s3__stepfunctions.get-query-execution-for-an-unknown-id-fails-as-task-failed
  AWS:      Smithy (services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json): GetQueryExecution takes a required QueryExecutionId and models InvalidRequestException (400) and InternalServerException (500); an id Athena never created is refused with InvalidRequestException. A direct `arn:aws:states:::athena:getQueryExecution` Task that hits that refusal fails the execution with error "Athena.InvalidRequestException" and a `cause` that is the AWS SDK's rendering of the service exception. That rendering has a fixed template: "<service message> (Service: <sdkId>; Status Code: <http status>; Error Code: <error code>; Request ID: <request id>; Proxy: null)" — the parenthesised key names, their order, the separators and "Proxy: null" are fixed text; the service message, sdkId (AmazonAthena), status code (400), error code (InvalidRequestException) and the request id are values from the response. The request id is a per-call value, so its CONTENT is not a requirement — but the "Request ID: <...>;" segment's PRESENCE is, since the envelope always carries one.
  Emulator: the task does fail with error "Athena.InvalidRequestException" and the correct message head, but the relayed cause is "QueryExecution <id> was not found (Service: AmazonAthena; Status Code: 400; Error Code: InvalidRequestException; Proxy: null)" — the "Request ID: <id>; " segment is absent, so the envelope jumps straight from the error code to "Proxy: null". After masking the request id (the check masks exactly that per-call value and nothing else), the string still does not match.
  Why:      Clients and operators parse this cause string: SFN failure handlers, log scrapers and support tooling extract the Request ID from it to correlate a failed task with the underlying service call. Against the emulator that field is simply missing, so correlation code either finds nothing or fails its parse, and any Choice/Catch rule or test that matches the full envelope shape diverges from AWS even though the error name and message are right.
  Fix:      Athena's refusal path must emit a request id on the error response (the Athena error serializer / the InvalidRequestException construction in services/service-com-amazonaws-athena-amazonathena/src/ops/get_query_execution.rs and the service's shared error envelope), and the Step Functions optimized-integration error relay must include it when it renders the task `cause`. Open issue 20261104T183813Z-athena-s3-stepfunctions-athena-a-query-over-a-nonexistent-table-reports.md covers a different Athena message (TABLE_NOT_FOUND text) and 20261104T083348Z-athena-s3-stepfunctions-an-athena-startqueryexecution-sync-task.md covers the startQueryExecution.sync payload/result object; neither covers the missing Request ID segment in the relayed error envelope.
  Traces:   athena__s3__stepfunctions.get-query-execution-for-an-unknown-id-fails-as-task-failed

## Evidence
Check athena__s3__stepfunctions.get-query-execution-for-an-unknown-id-fails-as-task-failed (verdict: mismatch).
State machine: single Task, Resource "arn:aws:states:::athena:getQueryExecution", Parameters {"QueryExecutionId.$": "$.qid"}; input qid = a UUID Athena never minted.
Expected (AWS), after masking only the per-call request id:
  describe_execution.status == "FAILED"
  describe_execution.error == "Athena.InvalidRequestException"
  describe_execution.cause == "QueryExecution <bogus-id> was not found (Service: AmazonAthena; Status Code: 400; Error Code: InvalidRequestException; Request ID: <id>; Proxy: null)"
Actual (emulator):
  cause == "QueryExecution <bogus-id> was not found (Service: AmazonAthena; Status Code: 400; Error Code: InvalidRequestException; Proxy: null)"
Replay failure: AssertionError: the task's cause was 'QueryExecution 8e58232c-89af-4f1d-89b5-0ebc79aa8b03 was not found (Service: AmazonAthena; Status Code: 400; Error Code: InvalidRequestException; Proxy: null)' (check line 39).
Fixed text of the diverging message: "(Service: AmazonAthena; Status Code: 400; Error Code: InvalidRequestException; Request ID: ...; Proxy: null)" — the key names, order, semicolon separators and "Proxy: null" are fixed; the leading "QueryExecution <id> was not found" takes <id> from the request; the request id value is per-call and only its presence is asserted. The positive control (the identical task against a real completed query execution) is what the check checks next and is not the divergence reported here.

## Steps to Reproduce
1. iam:CreateRole trusting states.amazonaws.com with an inline policy allowing athena:GetQueryExecution on "*".
2. states:CreateStateMachine(type=STANDARD, definition = one Task with Resource "arn:aws:states:::athena:getQueryExecution" and Parameters {"QueryExecutionId.$": "$.qid"}, End=true).
3. states:StartExecution(input = {"qid": "<a random UUID that Athena never minted>"}) and poll states:DescribeExecution until terminal.
4. Observe status FAILED and error "Athena.InvalidRequestException" (both correct), then read `cause`: the "Request ID: <id>; " segment between "Error Code: InvalidRequestException;" and "Proxy: null" is missing, where AWS always emits one.
5. Positive control: run the same state machine with a real, SUCCEEDED QueryExecutionId and confirm it still SUCCEEDS with output.QueryExecution.QueryExecutionId equal to that id.

Replay with: athena__s3__stepfunctions.get-query-execution-for-an-unknown-id-fails-as-task-failed
