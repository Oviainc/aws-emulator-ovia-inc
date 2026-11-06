Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create
Issue-Key: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes|_poll#32|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create|run#16|AssertionError|assert _REASON.fullmatch(failed[0][1] or ""), (
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# cloudformation__lambda__sqs__ssm: a SAM SQS-event mapping still never consumes the queue, and the failed-resource reason drops CloudFormation's "Invalid request provided: " prefix (2 causes)

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the SAM (AWS::Serverless-2016-10-31) SQS-event provisioning path. The fixed issue 20261102T094631Z-cloudformation-lambda-sqs-ssm-2-net-new-causes.md closed the unresolved-intrinsics and never-rolled-back causes for the same two checks; both checks now get further and fail on the causes below.

Cause 1 of 2: the AWS::Lambda::EventSourceMapping generated from a SAM function's SQS event never becomes a working trigger — the check cannot reach a verdict inside the harness budget.
AWS: the transform expands the function's SQS Event into an AWS::Lambda::EventSourceMapping (BatchSize 1, EventSourceArn the queue, FunctionName the function) and attaches the AWSLambdaSQSQueueExecutionRole managed policy to the generated role; after CREATE_COMPLETE, ListEventSourceMappings reports exactly one mapping for the queue, GetEventSourceMapping reports State 'Enabled' (Smithy: lambda CreateEventSourceMapping output members State, FunctionArn, EventSourceArn, UUID; the transform itself is not modelled, so the expansion and the polling behaviour are recorded behavior only), and a message sent to the queue is delivered to the handler, which writes the body to its SSM marker parameter, after which ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible both return to '0'.
Emulator: the check exhausts the whole 60 s budget ('Check timed out after 60.0s') waiting on that sequence — the generated mapping does not converge to Enabled and/or never invokes the function, so the marker parameter is never written and the queue never drains.
Why: the standard SAM SQS-event pattern deploys clean but is inert: a client sees a CREATE_COMPLETE stack with a mapping that never consumes, and no error anywhere says so.
Fix: the SQS event-source-mapping poller/runtime in the Lambda service and the CloudFormation provisioner that creates the generated mapping (the mapping must reach Enabled and poll the queue, invoking the function).
Traces: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes.

Cause 2 of 2: when the generated mapping is refused because the execution role may not read the queue, the failed resource's ResourceStatusReason omits the 'Invalid request provided: ' prefix CloudFormation puts on the provider's message.
AWS: the create is refused by Lambda (Smithy: lambda CreateEventSourceMapping error InvalidParameterValueException, httpError 400), the resource fails and the stack rolls back, and the failed resource's ResourceStatusReason is 'Resource handler returned message: "Invalid request provided: The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'. Fixed text: 'Resource handler returned message: "Invalid request provided: ', the Lambda sentence 'The function execution role does not have permissions to call ReceiveMessage on SQS', '(Service: Lambda, Status Code: 400, Request ID: ', ') (SDK Attempt Count: ', ')" (RequestToken: ', ', HandlerErrorCode: InvalidRequest)'. Variable parts are the two request/token uuids and the attempt count, which come from the call and are per-run values; the wrapper wording is recorded behavior only.
Emulator: the same reason without the prefix — 'Resource handler returned message: "The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: 1)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'. The status (resource failed, stack rolled back) and the HandlerErrorCode are right; only the message text diverges.
Why: a client (or deployment tool) that matches CloudFormation's documented reason text to classify a failure as an invalid-request handler error does not recognise the emulator's sentence, so the failure is reported as unknown.
Fix: the resource-provider error wrapper that builds ResourceStatusReason in CloudFormation's provisioner — an InvalidRequest handler error must be prefixed 'Invalid request provided: '.
Traces: cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create.

## Evidence
Cause 1 — Expected: the generated FnWork mapping reaches State 'Enabled', a sent message is written to the SSM marker by the handler, and the queue returns to ('0','0'). Actual: 'Check timed out after 60.0s'.
Cause 2 — Expected ResourceStatusReason: 'Resource handler returned message: "Invalid request provided: The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'. Actual: 'AssertionError: FnWork must fail on the missing ReceiveMessage permission; got \'Resource handler returned message: "The function execution role does not have permissions to call ReceiveMessage on SQS (Service: Lambda, Status Code: 400, Request ID: 4c945199-c0f4-ba3b-1527-23604a6ace39) (SDK Attempt Count: 1)" (RequestToken: f5aa2e16-1fed-96e1-d5bd-e565ccb056ef, HandlerErrorCode: InvalidRequest)\'' (the uuids are per-run values, not part of the requirement).

## Steps to Reproduce
Cause 1: CreateStack with Transform AWS::Serverless-2016-10-31 and a function carrying Events: {Type: SQS, Properties: {Queue: <queue arn>, BatchSize: 1}} plus an SSM marker parameter name; after CREATE_COMPLETE call ListEventSourceMappings(EventSourceArn=<queue arn>) and GetEventSourceMapping(UUID=...) until State=='Enabled', then SendMessage and poll GetParameter for the marker and GetQueueAttributes for ApproximateNumberOfMessages/NotVisible.
Cause 2: deploy the same template with an explicit execution role that holds no sqs:ReceiveMessage on the queue; poll DescribeStacks to the terminal status and read the failed resource's ResourceStatusReason from DescribeStackEvents.
Replay checks cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes and cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create.
