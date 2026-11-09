Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack
Issue-Key: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack|_create_request_id_from_logs#13|AssertionError|raise AssertionError(
Issue-Commit: cb4018749ce62725bb0d8ace7cfa8a8c0d6da7be

# cloudformation__dynamodb__lambda: a CDK custom_resources.Provider onEvent handler produces no CloudWatch Logs records — /aws/lambda/<stack>-OnEvent… holds none of the handler's output, so the Create it answered cannot be correlated with the resource's CREATE_FAILED reason

> _Note: this issue was identified against the code base at commit `cb4018749ce62725bb0d8ace7cfa8a8c0d6da7be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new behavioral root cause of this service group, grounded in one failing behavioral check of this replay. The check is NOT marked OWNED or ALREADY FILED in the replay report: the open issue 20261105T204906Z-cloudformation-dynamodb-lambda-a-custom-resources-provider-custom.md does list this check id, but its replay key for it is the earlier whole-check timeout (`_create_request_id_from_logs#13|Timeout|time.sleep(3.0)`), recorded when the deploy never produced any verdict at all. The deploy now does produce one — `cdk deploy` exits non-zero, the stack reaches ROLLBACK_COMPLETE, and the Provider's two Lambda functions are present as stack resources — so what remains is a different, later observable with a different key, filed here rather than closed under the older one. That issue (the provider function left in state Pending so the invocation is refused ResourceConflict) is named as the nearest neighbour; it is the likely upstream of this symptom, but it does not carry this key.

Cause 1 of 1: the Provider framework's onEvent Lambda emits no log records, so its CloudWatch log group yields none of the lines the handler prints while answering the custom resource's Create.
AWS: recorded behavior only for the Provider framework; the CloudWatch Logs surface is modelled (logs DescribeLogStreams / GetLogEvents). CloudFormation invokes the Provider's framework-onEvent function for the custom resource's Create; Lambda creates the log group /aws/lambda/<function-name> and a log stream per execution environment, and everything the handler writes to stdout — here one line of the form `REQUEST_ID=Create:<RequestId>`, whose fixed text is `REQUEST_ID=` plus the request type and whose variable part is the CloudFormation RequestId of that Create — is readable back through DescribeLogStreams + GetLogEvents within seconds. That same RequestId is what CloudFormation names in the resource's CREATE_FAILED ResourceStatusReason when the Provider's isComplete handler never reports done: `Received response status [FAILED] from custom resource. Message returned: Operation timed out (RequestId: <RequestId>)` — fixed text whose only variable part is that RequestId (Smithy com.amazonaws.cloudformation#StackEvent.ResourceStatusReason carries it; ResourceStatus is CREATE_FAILED).
Emulator: for the function CloudFormation lists as the stack's OnEvent AWS::Lambda::Function, no log event containing the handler's marker exists in /aws/lambda/<that function> for 90s of polling across every stream of the group (the probe tolerates the group being absent entirely, so either the group has no stream or no stream has handler output). The handler therefore never ran, or ran without its output reaching CloudWatch Logs; consequently the Create's RequestId is unobservable and the resource's CREATE_FAILED reason cannot be matched against the Provider's own timeout verdict.
Why: the Provider framework is how a CDK app performs any work CloudFormation has no resource type for. When a Provider-backed deploy fails, the handler's CloudWatch Logs are the only place a client can see what the handler was asked and what it answered — `cdk deploy` itself points the user at exactly that log group. With no records there, a failed custom resource is undiagnosable: a client cannot tell 'onEvent was never invoked' from 'onEvent was invoked and raised', and cannot correlate the stack event's RequestId with the invocation that produced it.
Fix: the lambda Invoke execution path for the Provider framework's functions and its emission of the invocation's stdout/stderr into the function's CloudWatch Logs group and stream (log-group creation on first invoke, one stream per execution environment, START/END/REPORT plus handler output), together with the cloudformation custom-resource provisioner's invocation of the framework-onEvent function so that an invocation happens at all.
Traces: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack. Relevant components: services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning/custom_resource.rs, services/service-com-amazonaws-cloudformation-cloudformation/src/state/stabilising.rs.

## Evidence
Check cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack [mismatch].

Assertions that PASS now: `cdk deploy <stack> --require-approval never` of an app whose custom_resources.Provider has isComplete permanently returning False and total_timeout 40s exits non-zero; DescribeStacks StackStatus == 'ROLLBACK_COMPLETE'; ListStackResources shows exactly one AWS::Lambda::Function whose LogicalResourceId starts with 'OnEvent', with a PhysicalResourceId.

Assertion that FAILS: reading that function's Create RequestId back out of its log group.
Expected: at least one log event in /aws/lambda/<onEvent function> containing `REQUEST_ID=Create:<RequestId>` (exactly one distinct RequestId across all streams), and then the 'Waited' resource's CREATE_FAILED ResourceStatusReason equal to `Received response status [FAILED] from custom resource. Message returned: Operation timed out (RequestId: <that RequestId>)`.
Actual, from the replay: `AssertionError: no 'REQUEST_ID=Create:' line found in /aws/lambda/sim2real-cdkwaiter-d66a2c1942d1-OnEvent74718524-69230605B849 within 90.0s` (check line 80 -> _create_request_id_from_logs line 163). The function/log-group name and the RequestId are per-run values; the presence of the handler's own output in the function's log group is the requirement.

## Steps to Reproduce
1. `cdk bootstrap` a qualifier, then `cdk deploy` an app whose stack declares a `custom_resources.Provider` with an onEvent handler that prints a marker line including `event['RequestId']` and an isComplete handler that permanently returns `IsComplete: False`, with the Provider's `total_timeout` set to ~40s.
2. The deploy exits non-zero and DescribeStacks reports ROLLBACK_COMPLETE (both correct today).
3. Resolve the onEvent function via ListStackResources, then call logs:DescribeLogStreams on /aws/lambda/<that function> and logs:GetLogEvents on every stream.
4. Expected: the handler's printed marker line (and the runtime's START/END/REPORT lines) are present, giving the Create's RequestId. Observed: no stream in that group carries any handler output for 90s of polling, so the RequestId is unobservable and the 'Waited' resource's CREATE_FAILED reason cannot be correlated with it.
5. Re-replay: cloudformation__dynamodb__lambda.cdk-custom-resource-provider-iscomplete-timeout-fails-the-stack.
