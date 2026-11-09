Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: account__apigatewayv2__events__lambda__stepfunctions.sfn-aws-sdk-resolves-distinct-services-with-prefixed-errors
Issue-Key: account__apigatewayv2__events__lambda__stepfunctions.sfn-aws-sdk-resolves-distinct-services-with-prefixed-errors|run#6|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Commit: fb003e0a3fdd2a31392ea928ed9540deb9fe4c34

# Step Functions aws-sdk Task integrations resolve no service beyond the handful the dispatcher knows: account/apigatewayv2/eventbridge/lambda Tasks all fail States.Runtime (1 net-new cause)

> _Note: this issue was identified against the code base at commit `fb003e0a3fdd2a31392ea928ed9540deb9fe4c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause, grounded in a failing check this report marks neither ALREADY FILED nor OWNED. Open issue 20261107T185637Z-s3-stepfunctions-step-functions-aws-sdk-service-integrations-are-not.md describes the same dispatcher for the `s3` prefix only and does not carry this check's replay key; this check shows the gap is not s3-specific — four further service prefixes are unresolvable in one execution.

NOTE ON SERVICE ATTRIBUTION: the suite lists this check under the composite heading `account__apigatewayv2__events__lambda__stepfunctions`, which is the heading used for the `service` field here. Every constituent service (account, apigatewayv2, eventbridge/events, lambda, sfn) is reported bootstrapped, and `pending` is empty, so the per-service bootstrap gate defers nothing here. The defect itself lives in the Step Functions Task-state integration dispatcher.

Cause 1 of 1: a Task state whose `Resource` is an AWS-SDK integration ARN for a service other than the few the dispatcher hard-codes is never dispatched; the state fails immediately with an emulator-authored `States.Runtime`.

AWS: a Task state with Resource `arn:aws:states:::aws-sdk:<servicePrefix>:<action>` calls that service's API with the state's `Parameters` under the state machine's execution role and succeeds with the API's own output shape. For the four prefixes this check uses, the modelled outputs are: account:listRegions -> `Regions`, a list of RegionOpt each carrying `RegionName` (Smithy: account ListRegions output members NextToken, Regions:RegionOptList; errors AccessDeniedException 403, ValidationException 400, TooManyRequestsException 429, InternalServerException 500); apigatewayv2:getApis -> `Items`; eventbridge:listApiDestinations -> `ApiDestinations`; lambda:listCodeSigningConfigs -> `CodeSigningConfigs`. When the execution role lacks the matching IAM action the state fails with the service-prefixed error name `<ServicePrefix>.<Exception>` (here `Account.AccessDeniedException`) and a cause naming the calling account; it never fails with a Step Functions interpreter error. AWS resolves EVERY aws-sdk prefix the SDK knows, not a fixed subset — recorded behavior plus the Smithy models above.

Emulator: none of the four branches is dispatched. The parallel execution ends `FAILED` with error `States.Runtime` and the emulator-authored cause "state 'Account': 'arn:aws:states:::aws-sdk:account:listRegions' names an integration with 'account', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it". The fixed text of that message is the emulator's own invention ("names an integration with ... which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"); its variable parts are the state name, the Resource ARN and the service prefix taken from the definition. AWS emits no such error code/message pair for a valid aws-sdk ARN. Because the state machine never succeeds, the second half of the check — the service-prefixed denial name `Account.AccessDeniedException` under a role missing `account:ListRegions` — is never reached, so that behavior is unverified rather than known-good.

Why: the aws-sdk integration is the general-purpose way a state machine touches any AWS service. A client building a fan-out workflow over several services gets a hard interpreter failure instead of per-service results, and cannot distinguish a genuine authorization failure of its workflow (`Account.AccessDeniedException`) from the emulator refusing to dispatch at all. Catch/Retry rules keyed on `<ServicePrefix>.<Exception>` never match, and least-privilege role testing is impossible because every call fails the same way whether the grant is present or not.

Fix: the Step Functions Task-state integration dispatcher (and its service-resolution table) — resolve `arn:aws:states:::aws-sdk:<service>:<action>` generically to the emulator's handler for that service instead of a hard-coded allow-list, invoke it under the execution role, return the API's modelled output as the state result, and surface an authorization failure as `<ServicePrefix>.<Exception>` with the service's own message as the cause.

Traces: account__apigatewayv2__events__lambda__stepfunctions.sfn-aws-sdk-resolves-distinct-services-with-prefixed-errors.

## Evidence
Check account__apigatewayv2__events__lambda__stepfunctions.sfn-aws-sdk-resolves-distinct-services-with-prefixed-errors [mismatch].

Expected: the 4-branch Parallel execution under a role granting account:ListRegions, apigateway:GET, events:ListApiDestinations and lambda:ListCodeSigningConfigs ends status SUCCEEDED with 4 branch outputs — account branch with a non-empty `Regions` list whose entries each have `RegionName`, apigatewayv2 branch with `Items`, eventbridge branch with `ApiDestinations`, lambda branch with `CodeSigningConfigs`. Then, under a role missing only account:ListRegions, the single-state machine ends FAILED with error exactly `Account.AccessDeniedException` and a cause naming the caller's own account id.

Actual: the full-grant execution ended FAILED. AssertionError: "the 4-branch execution under the least-privilege role ended 'FAILED': 'States.Runtime' \"state 'Account': 'arn:aws:states:::aws-sdk:account:listRegions' names an integration with 'account', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it\"". The assertions on the four branch outputs and on the `Account.AccessDeniedException` denial name were never reached.

## Steps to Reproduce
1) Create an IAM role trusted by states.amazonaws.com with an inline policy allowing account:ListRegions, apigateway:GET, events:ListApiDestinations and lambda:ListCodeSigningConfigs on Resource "*".
2) CreateStateMachine (STANDARD) with a single Parallel state holding four branches, each one Task with Parameters {} and Resource arn:aws:states:::aws-sdk:account:listRegions, arn:aws:states:::aws-sdk:apigatewayv2:getApis, arn:aws:states:::aws-sdk:eventbridge:listApiDestinations and arn:aws:states:::aws-sdk:lambda:listCodeSigningConfigs respectively.
3) StartExecution with input {} and poll DescribeExecution to terminal. Observe status FAILED with error States.Runtime naming the 'account' integration as unknown, instead of SUCCEEDED with the four services' outputs.
4) Create a second role granting only the apigateway/events/lambda actions, and a state machine with the single arn:aws:states:::aws-sdk:account:listRegions Task. StartExecution and poll: the failure should be error `Account.AccessDeniedException` with a cause naming the calling account id, not States.Runtime.
