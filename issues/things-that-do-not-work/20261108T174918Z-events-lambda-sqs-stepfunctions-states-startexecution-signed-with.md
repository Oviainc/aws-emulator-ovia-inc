Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions
Issue-Key: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions|run#18|AssertionError|assert payload["states"].startswith("DENIED"), (
Issue-Commit: 1c23821fb3d164cb2173886e6d0cb83ac85c4445

# events__lambda__sqs__stepfunctions: states:StartExecution signed with a Lambda execution role that grants nothing is still served ALLOWED — the Step Functions handler runs no identity-policy evaluation (the sibling events:PutEvents deny now works)

> _Note: this issue was identified against the code base at commit `1c23821fb3d164cb2173886e6d0cb83ac85c4445`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: Step Functions StartExecution does not authorize the signing principal against its identity-based policy, so a Lambda function whose execution role carries no `states:StartExecution` statement starts an execution anyway.

Note on an existing open issue: 20261108T042219Z-events-lambda-sqs-stepfunctions-a-lambda-execution-role.md describes the same check with BOTH legs (events:PutEvents and states:StartExecution) over-permissive, and its replay key is bound to the PutEvents assertion. The EventBridge leg now behaves correctly — the handler reports the exact AWS implicit-deny message for events:PutEvents — so that key no longer matches and the suite did not mark this failure as owned. The Step Functions leg is still unauthorized, and that is what this issue carries. It does not cover it, so it is filed here.

AWS: Step Functions evaluates the signing principal's identity-based policy before it creates an execution. A call signed with an assumed-role session for a role that has no statement allowing `states:StartExecution` on the target state machine is refused with HTTP 400 and error code `AccessDeniedException`, carrying the standard implicit-deny message. Fixed text, exactly: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts come from the request and the emulator's IAM state — the assumed-role session ARN of the invoked function's execution role (`arn:aws:sts::<account>:assumed-role/<role-name>/<function-name>`), the action `states:StartExecution`, and the `stateMachineArn` the request names. Smithy: sfn#StartExecution models ExecutionAlreadyExists, ExecutionLimitExceeded, InvalidArn, InvalidExecutionInput, InvalidName, Kms*, StateMachineDeleting, StateMachineDoesNotExist and ValidationException — AccessDeniedException is NOT modelled per-operation on sfn, so this is the framework-level IAM refusal: recorded behavior only, taken from the recording's response. No execution is created, so ListExecutions on that state machine stays empty.

Emulator: the call succeeds. The in-handler probe records `{'events': 'DENIED:...', 'states': 'ALLOWED'}` where the recording has `states` beginning with `DENIED`; the check fails with `AssertionError: states:StartExecution succeeded without permission -- overly permissive`. The emulator also creates the execution, so the follow-on "no execution started" assertion has nothing to hold. The EventBridge leg in the same handler is refused correctly with the full implicit-deny sentence, which shows the principal plumbing exists for role-signed traffic and only the Step Functions path skips the evaluation.

Why: a client relying on AWS's behavior gets least-privilege validation inverted. A stack whose state-machine-invoking role is missing `states:StartExecution` passes against the emulator and is refused by AWS on deploy; conversely, negative tests that assert a denial can never go green, and any code path that reacts to AccessDeniedException (catch/retry/alarm) is never exercised. Executions also appear that AWS would never have started, so downstream state diverges.

Fix: the Step Functions StartExecution handler's pre-dispatch authorization step — the identity-policy evaluation must run for role-signed requests (the same evaluation the EventBridge PutEvents handler already performs) before any execution record is created, and the refusal must be serialized as HTTP 400 `AccessDeniedException` with the implicit-deny message templated from the signing session ARN, the action and the requested stateMachineArn. The principal plumbing that carries the invoked function's execution-role session into the authorizer is already proven by the EventBridge leg; the gap is the sfn side of it.

Traces: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions.

## Evidence
Check events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions

Expected (recorded AWS): the in-handler result for the no-permission role is `{'events': 'DENIED:ClientError:An error occurred (AccessDeniedException) when calling the PutEvents operation: User: <assumed-role session ARN> is not authorized to perform: events:PutEvents on resource: <event-bus ARN> because no identity-based policy allows the events:PutEvents action', 'states': 'DENIED:ClientError:An error occurred (AccessDeniedException) when calling the StartExecution operation: User: <assumed-role session ARN> is not authorized to perform: states:StartExecution on resource: <stateMachineArn> because no identity-based policy allows the states:StartExecution action'}`, and ListExecutions on that state machine stays empty.

Actual (emulator): `{'events': 'DENIED:AccessDeniedException:An error occurred (AccessDeniedException) when calling the PutEvents operation: User: arn:aws:sts::<account>:assumed-role/<role>/<fn> is not authorized to perform: events:PutEvents on resource: arn:aws:events:<region>:<account>:event-bus/default because no identity-based policy allows the events:PutEvents action', 'states': 'ALLOWED'}`

AssertionError raised at check line 111:
`assert payload["states"].startswith("DENIED"), ("states:StartExecution succeeded without permission -- overly permissive: ...")`

The events leg matches AWS; only the states leg diverges. The account id, region, role name and function name above are per-run values filling the message template, not part of the requirement.

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. Create an IAM role trusting states.amazonaws.com and a trivial state machine (`{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}`) with that role.
3. Create a Lambda execution role with NO inline or attached policy (no states:StartExecution statement) and deploy a handler that calls `boto3.client("stepfunctions").start_execution(stateMachineArn=<sm arn>)`, recording ALLOWED on success and DENIED:<type>:<message> on exception. Set SM_ARN in the function environment.
4. Invoke the function. The emulator returns `states: ALLOWED`; AWS returns a 400 AccessDeniedException whose message is `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: states:StartExecution on resource: <stateMachineArn> because no identity-based policy allows the states:StartExecution action`.
5. Call stepfunctions ListExecutions on that state machine: the emulator shows an execution that AWS never started.
6. Re-replay the check id in trace_ids to confirm a fix.
