Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.cfn-state-machine-alias-routing-weights-not-summing-to-100-rejected cloudformation__stepfunctions.cfn-state-machine-version-and-alias-round-trip-and-removed-on-stack-delete cloudformation__stepfunctions.deploy-pass-state-with-next-and-end-together-rejected-at-stack-create cloudformation__stepfunctions.deploy-cdk-choice-default-unknown-state-rejected
Issue-Key: cloudformation__stepfunctions.cfn-state-machine-alias-routing-weights-not-summing-to-100-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected alias stack ended {status!r}"
Issue-Key: cloudformation__stepfunctions.cfn-state-machine-version-and-alias-round-trip-and-removed-on-stack-delete|run#13|AssertionError|assert got == ([version_arn], version_arn), (
Issue-Key: cloudformation__stepfunctions.deploy-pass-state-with-next-and-end-together-rejected-at-stack-create|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__stepfunctions.deploy-cdk-choice-default-unknown-state-rejected|run#27|AssertionError|assert match is not None, f"the state machine's CREATE_FAILED reason is not a handler message: {reason!r}"
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__stepfunctions: 4 net-new causes — invalid alias routing weights accepted, state-machine versions not listed, Pass with both Next and End accepted, CREATE_FAILED reason omits the Step Functions service suffix

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four distinct root causes. Each is numbered with its own failing check id; all four must be closed before this issue is done.

Cause 1 of 4: an AWS::StepFunctions::StateMachineAlias whose RoutingConfiguration weights do not sum to 100 is accepted and the stack reaches CREATE_COMPLETE.
AWS: CreateStateMachineAlias refuses a routing configuration whose weights do not total 100 with a ValidationException, so CloudFormation marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: the client believes traffic is split as declared; the real routing is undefined.
Fix: the AWS::StepFunctions::StateMachineAlias handler's routing validation / propagation of the Step Functions refusal.
Traces: cloudformation__stepfunctions.cfn-state-machine-alias-routing-weights-not-summing-to-100-rejected.

Cause 2 of 4: a state machine version a stack creates is not listed by Step Functions.
AWS: after CREATE_COMPLETE, ListStateMachineVersions(stateMachineArn=<the machine>) returns the version the stack created, and its ARN matches the resource's Fn::GetAtt Arn.
Emulator: ListStateMachineVersions returns an empty list while Fn::GetAtt .Arn reports `arn:aws:stepfunctions:<region>:<account>:state-machine-version/<generated name>` — an ARN no listing knows.
Why: a client that publishes a version through CloudFormation cannot enumerate or route to it, and the ARN it is handed is not a real version ARN.
Fix: the AWS::StepFunctions::StateMachineVersion handler must publish the version into Step Functions state and mint the service's own version ARN.
Traces: cloudformation__stepfunctions.cfn-state-machine-version-and-alias-round-trip-and-removed-on-stack-delete.

Cause 3 of 4: a state machine whose Pass state declares both Next and End is accepted and the stack reaches CREATE_COMPLETE.
AWS: CreateStateMachine refuses a definition where a state declares both Next and End (InvalidDefinition), so the resource is CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: an invalid Amazon States Language definition is accepted, so the client ships a state machine that cannot execute, with no deploy-time signal.
Fix: the AWS::StepFunctions::StateMachine handler's definition validation.
Traces: cloudformation__stepfunctions.deploy-pass-state-with-next-and-end-together-rejected-at-stack-create.

Cause 4 of 4: the CREATE_FAILED reason for a rejected state-machine definition omits the service/status/request-id suffix AWS includes.
AWS: a resource failure caused by a service refusal is reported as `Resource handler returned message: "<service message> (Service: <ServiceId>, Status Code: <code>, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: <code>)` — the parenthesised service/status/request-id block is part of the shape, with the values taken from the failed call.
Emulator: the reason is `Resource handler returned message: "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: Missing 'Next' target: <state> at /States/<path>'" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the (Service: …, Status Code: …, Request ID: …) and (SDK Attempt Count: …) parts are missing.
Why: tooling that parses the owning service, HTTP status or request id out of a CloudFormation failure reason finds none of them, and cannot attribute the failure.
Fix: the handler-failure-reason wrapper in the CloudFormation provisioner that must append the service, status code, request id and SDK attempt count of the refused call.
Traces: cloudformation__stepfunctions.deploy-cdk-choice-default-unknown-state-rejected.

## Evidence
Cause 1: AssertionError: rejected alias stack ended 'CREATE_COMPLETE'.
Cause 2: AssertionError: (ListStateMachineVersions, GetAtt Arn) ([], 'arn:aws:stepfunctions:us-west-2:000000000000:state-machine-version/<stack>-MyVersion-<suffix>').
Cause 3: AssertionError: stack with a Pass state declaring both Next and End ended in 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE.
Cause 4: AssertionError: the state machine's CREATE_FAILED reason is not a handler message: 'Resource handler returned message: "Invalid State Machine Definition: ..." (RequestToken: ..., HandlerErrorCode: InvalidRequest)' — no (Service: ..., Status Code: ..., Request ID: ...) block.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateStack with an AWS::StepFunctions::StateMachineAlias whose RoutingConfiguration weights sum to something other than 100.
3. Cause 2: deploy AWS::StepFunctions::StateMachineVersion and call ListStateMachineVersions.
4. Cause 3: CreateStack with a definition whose Pass state has both Next and End.
5. Cause 4: `cdk deploy` a state machine whose Choice Default names an unknown state and read the resource's CREATE_FAILED reason.
