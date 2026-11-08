Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions
Issue-Key: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions|run#14|AssertionError|assert payload["events"].startswith("DENIED"), (
Issue-Commit: 742d95e483c6290726dac54edb7cc16a76e76b19

# events__lambda__sqs__stepfunctions: a Lambda execution role that grants nothing is allowed both events:PutEvents and states:StartExecution — no identity-policy evaluation at all

> _Note: this issue was identified against the code base at commit `742d95e483c6290726dac54edb7cc16a76e76b19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: calls signed with a Lambda execution role's credentials are never authorized against that role's identity policy, for EventBridge and for Step Functions alike.
  AWS: both services evaluate the signing principal's identity-based policy before dispatch. A function whose execution role carries no `events:PutEvents` and no `states:StartExecution` statement gets the standard IAM refusal on each call — HTTP 400 with error code `AccessDeniedException` and the implicit-deny message `User: <assumed-role session ARN> is not authorized to perform: <action> on resource: <resource ARN> because no identity-based policy allows the <action> action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts from the request and the emulator's IAM state: the function role's assumed-role session ARN, the action (`events:PutEvents` / `states:StartExecution`), and the resource ARN the request names (the event bus ARN; the stateMachineArn). Smithy models AccessDeniedException on the EventBridge surface (eventbridge#AccessDeniedException, client, HTTP 400) but does not model it per-operation on sfn — on both this is the framework refusal, so recorded behavior only.
  Emulator: both calls are served. The handler reports `{'events': 'ALLOWED', 'states': 'ALLOWED'}` where the recording has both DENIED.
  Why: the emulator is uniformly over-permissive for role-signed traffic. Least-privilege scenarios cannot be exercised; a user's stack that is missing a grant passes locally and fails in AWS, which is the exact class of bug this emulator is used to catch.
  Fix: the shared request-authorization path for role-signed requests — the identity-policy evaluation that must run in the EventBridge PutEvents handler and the Step Functions StartExecution handler before either mutates state, plus the principal plumbing that carries the invoked function's execution-role session into that evaluation.
  Traces: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions.

Related open issues that do NOT cover this one (different service scope): 20261106T151148Z-events-lambda-s3-stepfunctions-in-function-events-putevents.md, 20261106T133436Z-events-lambda-sns-a-lambda-handler-s-events.md.

## Evidence
events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions:
Expected: payload['events'] and payload['states'] both start with 'DENIED' (each in-handler call refused HTTP 400 AccessDeniedException).
Actual: AssertionError: events:PutEvents succeeded without permission -- overly permissive: {'events': 'ALLOWED', 'states': 'ALLOWED'}

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. Create an event bus and a state machine. Create a Lambda execution role with an inline policy that grants neither events:PutEvents nor states:StartExecution.
3. Deploy a handler that calls events.put_events(Entries=[{EventBusName: <bus>, ...}]) and stepfunctions.start_execution(stateMachineArn=<sm>), recording ALLOWED/DENIED for each. Invoke it.
4. The emulator reports ALLOWED for both instead of AccessDeniedException. Re-replay the check id in trace_ids to confirm.
