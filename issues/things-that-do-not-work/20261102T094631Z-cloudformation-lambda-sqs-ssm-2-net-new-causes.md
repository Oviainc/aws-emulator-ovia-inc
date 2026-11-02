Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create
Issue-Key: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes|run#22|AssertionError|assert mapping["Properties"] == {"BatchSize": 1, "EventSourceArn": {"Ref": "QueueArn"},
Issue-Key: cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create|run#13|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__lambda__sqs__ssm: 2 net-new causes — a SAM SQS event's generated event source mapping keeps unresolved intrinsics, and an ESM whose role cannot read the queue is created instead of failing the stack

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the SAM-transform / event-source-mapping provisioning path.

Cause 1 of 2: the AWS::Lambda::EventSourceMapping generated from a SAM function's SQS event is stored with the template's intrinsic functions unresolved.
AWS: the AWS::Serverless-2016-10-31 transform expands a function's SQS Event into an AWS::Lambda::EventSourceMapping whose properties are fully resolved at provisioning time — EventSourceArn is the queue's ARN string and FunctionName the function's name/ARN string — and the mapping then consumes from the queue. Smithy (cloudformation CreateStack; the transform itself is not modeled) says nothing about the expansion, so this is recorded behavior only: the recorded resource's properties are concrete strings.
Emulator: DescribeStackResource reports the generated mapping with the raw intrinsics still in place — 'FnWork properties OrderedDict({"BatchSize": 1, "EventSourceArn": OrderedDict({"Ref": "QueueArn"}), "FunctionName": OrderedDict({"Fn::GetAtt": ["Fn", "Arn"]})})' — so nothing binds the mapping to a real queue and the function never consumes.
Why: a client that deploys the standard SAM SQS-event pattern gets a stack that claims success while the trigger is inert; worse, the properties read back are template source, not resolved state, so tooling that diffs live configuration against the template sees phantom drift.
Fix: the SAM/Serverless transform expansion plus intrinsic resolution for generated resources in CloudFormation's template-processing layer.
Traces: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes.

Cause 2 of 2: CreateEventSourceMapping inside a stack is not authorized against the function's execution role, so the stack succeeds where AWS rolls it back.
AWS: creating an event source mapping requires the function's execution role to hold sqs:ReceiveMessage / sqs:DeleteMessage / sqs:GetQueueAttributes on the source queue; without them the Lambda call is refused InvalidParameterValueException and the resource fails, so the stack ends ROLLBACK_COMPLETE (the create is rolled back). Smithy (lambda CreateEventSourceMapping errors InvalidParameterValueException, HTTP 400) models the refusal class; the stack status is recorded behavior only.
Emulator: the stack with a deliberately unprivileged role ends CREATE_COMPLETE — 'a role without queue access must roll the create back; ended CREATE_COMPLETE'.
Why: the single most common SAM/SQS misconfiguration (a role missing the queue grants) deploys clean in the emulator and then silently never processes messages in AWS.
Fix: the role/queue permission gate in the CreateEventSourceMapping handler (shared by the direct API and the CloudFormation provisioner).
Traces: cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create.

## Evidence
Cause 1 — Expected: the generated mapping's EventSourceArn/FunctionName read back as resolved strings and the function consumes the queue's messages. Actual: 'AssertionError: FnWork properties OrderedDict({'BatchSize': 1, 'EventSourceArn': OrderedDict({'Ref': 'QueueArn'}), 'FunctionName': OrderedDict({'Fn::GetAtt': ['Fn', 'Arn']})})'.
Cause 2 — Expected: ROLLBACK_COMPLETE after the mapping create is refused InvalidParameterValueException for the missing sqs:ReceiveMessage grant. Actual: 'AssertionError: a role without queue access must roll the create back; ended CREATE_COMPLETE'.

## Steps to Reproduce
Cause 1: CreateStack with Transform AWS::Serverless-2016-10-31 and a function carrying an Events: {Type: SQS, Properties: {Queue: !GetAtt Q.Arn}} block; after CREATE_COMPLETE read the generated AWS::Lambda::EventSourceMapping via DescribeStackResource/DescribeStackResources and compare its properties with the resolved ARNs; send a message and poll for consumption. Cause 2: same template but with an execution role that holds no sqs permissions on the queue; poll DescribeStacks for the terminal status. Replay checks cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes and cloudformation__lambda__sqs__ssm.sam-sqs-event-role-without-queue-access-fails-the-mapping-create.
