Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions
Issue-Key: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions|run#9|AssertionError|assert allowed_payload["states"] == "ALLOWED", (
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# events__lambda__sqs__stepfunctions: the in-process Lambda runtime's SDK has no Step Functions client, so a granted states:StartExecution is reported DENIED (1 cause)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the AWS SDK the emulator's in-process Lambda python runtime supplies only knows four services, so any handler call to Step Functions fails with a client-construction error instead of reaching the API — and the check's positive control (a role that DOES grant states:StartExecution) is therefore recorded as denied.
AWS: a python Lambda handler can construct a client for any AWS service and call it with the execution role's credentials. With states:StartExecution granted, StartExecution answers 200 with {executionArn, startDate}; without the grant it is refused by IAM (HTTP 400, code `AccessDeniedException`, message "User: <assumed-role session ARN> is not authorized to perform: states:StartExecution on resource: <state machine ARN> because no identity-based policy allows the states:StartExecution action"). Smithy: StartExecution → StartExecutionOutput{executionArn, startDate}, modeled errors ExecutionLimitExceeded, ExecutionAlreadyExists, InvalidArn, InvalidExecutionInput, InvalidName, StateMachineDoesNotExist, StateMachineDeleting, ValidationException — an unknown-service failure is not among them.
Emulator: the handler never gets a client. The replay records the two arms as {'events': 'ALLOWED', 'states': "DENIED:UnknownServiceError:Unknown service: 'stepfunctions'. Valid service names are: sqs, events, eventbridge, dynamodb"} — i.e. the runtime's SDK enumerates exactly sqs, events, eventbridge and dynamodb, and every other service raises UnknownServiceError before a request is signed. Because the harness classifies that raise as a denial, a granted call is scored as denied and an ungranted one would be "denied" for the wrong reason.
Why: any authorization conclusion drawn from this emulator for a non-enumerated service is meaningless — allowed and denied are indistinguishable, and a handler that works on AWS crashes here with an error type (UnknownServiceError) that does not exist in AWS's SDK contract for a valid service.
Fix: the in-process Lambda python runtime's bundled AWS SDK — its service-client registry must cover the services the function's handler calls (here Step Functions), so the call is signed, routed to the emulator's SFN endpoint, and answered by StartExecution's handler or refused by the authorization layer. Sibling open issues name the same runtime limitation for other suites (20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md, 20261105T094343Z-events-lambda-sqs-the-lambda-python-runtime-provides.md) but neither carries this check's replay key.
Traces: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions.

## Evidence
Expected: with the role granting states:StartExecution the handler's call succeeds (200, executionArn returned), so the positive control is ALLOWED; a role without the grant is refused AccessDeniedException with AWS's implicit-deny message. Actual: "AssertionError: positive control: states:StartExecution was denied even though the role grants it: {'events': 'ALLOWED', 'states': \"DENIED:UnknownServiceError:Unknown service: 'stepfunctions'. Valid service names are: sqs, events, eventbridge, dynamodb\"}" — the runtime SDK's service list is the fixed text here; the four names are the emulator's own enumeration.

## Steps to Reproduce
1. Create a state machine, an SQS queue and a Lambda function whose execution role grants both events:PutEvents and states:StartExecution.
2. Have the handler construct a 'stepfunctions' client and call start_execution; invoke the function.
3. The handler raises UnknownServiceError ("Valid service names are: sqs, events, eventbridge, dynamodb") instead of starting an execution.
4. Replay events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions.
