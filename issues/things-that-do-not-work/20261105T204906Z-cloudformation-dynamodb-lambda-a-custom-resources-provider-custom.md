Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack
Issue-Key: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete|_cdk#14|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack|_create_request_id_from_logs#13|Timeout|time.sleep(3.0)
Issue-Commit: a6bd8162e254cd53a71309df843cd006b7af8737

# cloudformation__dynamodb__lambda: a custom_resources.Provider custom resource is never invoked — the provider function is left in state Pending so CloudFormation's invocation is refused ResourceConflict, and the isComplete waiter never settles

> _Note: this issue was identified against the code base at commit `a6bd8162e254cd53a71309df843cd006b7af8737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new behavioral root cause of this service group, grounded in two failing behavioral checks of this replay.

Cause 1 of 1: a CDK `custom_resources.Provider` custom resource (the two-handler onEvent/isComplete framework) is never driven, because CloudFormation invokes the provider's framework function while the emulator still reports that function in state `Pending`, and the invocation is refused ResourceConflict.
AWS: recorded behavior only. A stack declaring a Provider-backed AWS::CloudFormation::CustomResource reaches CREATE_COMPLETE only after the isComplete handler reports done; the resource's Fn::GetAtt Data comes from THAT handler (not onEvent's placeholder), its Ref is stable across an update whose onEvent response does not change it, and a Provider whose isComplete never reports done fails the create at the Provider's own `total_timeout`, with reason `Operation timed out` and the stack ending ROLLBACK_COMPLETE. The refusal text AWS relays when a function genuinely is not yet usable is `Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: <uuid>, HandlerErrorCode: ResourceConflict)` — fixed text whose only variable part is the request token — and AWS waits past that state (Lambda State Pending -> Active) rather than failing the create on it.
Emulator: `cdk deploy` of the happy-path app exits 1 and the custom resource fails with exactly that ResourceConflict/Pending message against the framework-onEvent function, so the isComplete handshake is never entered. The negative twin (isComplete permanently false, 40s total_timeout) likewise never produces the Provider's own timeout verdict and the check exhausts its whole budget (`Check timed out after 60.0s`).
Why: the Provider framework is how almost every CDK app performs work CloudFormation has no resource type for (seeding a DynamoDB table, waiting on an external condition). With the provider never invoked, no such stack deploys, nothing is written to the table the handler seeds, the Data/Ref contract is unobservable, and the Provider's timeout — the only thing that stops a stack hanging forever — never fires. A client sees a client-side ResourceConflict and will retry forever rather than learning the provider was never run.
Fix: the custom-resource provisioner's provider-readiness wait in cloudformation (retry past a ResourceConflict/Pending refusal instead of terminating the resource) together with lambda's function-state transition out of Pending; then the Provider framework's isComplete polling and total_timeout accounting on the CloudFormation side.
Traces: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete, cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack.

## Evidence
cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete [mismatch]:
  AssertionError: `cdk deploy <stack> --require-approval never` exited 1, expected 0; output tail: '… DeploymentError: Resource updates failed:\n<stack>/Waited/Default  (AWS::CloudFormation::CustomResource Waited)\n  Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: <uuid>, HandlerErrorCode: ResourceConflict)\n… Logs from /aws/lambda/<stack>-ProviderframeworkonEvent…'
  Expected: exit 0, CREATE_COMPLETE after isComplete reports done, stack output Value == 'computed-<seed>' (the isComplete handler's Data, not onEvent's 'unset-by-on-event' placeholder), and a stable Ref across an update.

cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack [mismatch]:
  Error: Check timed out after 60.0s — with isComplete permanently returning False and the Provider's total_timeout set to 40s, the create must fail with the Provider's own timeout reason ('Operation timed out') and the stack must end ROLLBACK_COMPLETE; instead no verdict is ever reached.

Fixed text of the observed refusal: 'Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (RequestToken: …, HandlerErrorCode: ResourceConflict)'; the request token is the only variable part, and the log-group/function names in the output tail are per-run values.

## Steps to Reproduce
1. `cdk bootstrap` a qualifier, then `cdk deploy` an app whose stack contains a `custom_resources.Provider` with an onEvent handler and an isComplete handler that reports done after a few polls, plus a DynamoDB table the handlers write — expect exit 0, CREATE_COMPLETE, the stack output carrying the isComplete handler's Data ('computed-<seed>'), and a Ref unchanged by an update whose onEvent response does not change it.
2. Observe instead exit 1 with the custom resource failing 'Resource handler returned message: "The operation cannot be performed at this time. The function is currently in the following state: Pending" (… HandlerErrorCode: ResourceConflict)' against the Provider's framework-onEvent function.
3. Deploy the same app with isComplete permanently returning False and the Provider's total_timeout at 40s — expect a non-zero exit, the resource failing with reason 'Operation timed out' and the stack reaching ROLLBACK_COMPLETE within that budget.
4. Re-replay: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-waits-for-iscomplete, cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack.
