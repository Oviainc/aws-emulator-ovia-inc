Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__oam.deploy-oam-sink-invalid-name-pattern-rejected-before-any-sink-is-created cloudformation__oam.deploy-oam-sink-properties-getatt-and-sink-policy-read-back-then-resource-not-found-after-delete
Issue-Key: cloudformation__oam|CreateStack|ValidationError
Issue-Commit: a2d89a6d9d220944a1fa84d232d70dc048552542

# cloudformation__oam: AWS::Oam::Sink is not a registered resource type — CreateStack refuses every OAM sink template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `a2d89a6d9d220944a1fa84d232d70dc048552542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation emulator's template resource-type registry does not contain AWS::Oam::Sink, so a template that declares one is rejected at CreateStack time and no OAM sink is ever provisioned through CloudFormation. Failing checks: cloudformation__oam.deploy-oam-sink-invalid-name-pattern-rejected-before-any-sink-is-created, cloudformation__oam.deploy-oam-sink-properties-getatt-and-sink-policy-read-back-then-resource-not-found-after-delete.

AWS: AWS::Oam::Sink is a supported CloudFormation resource type (CloudFormation Template Reference, aws-resource-oam-sink; Properties Name, Policy, Tags; return values Ref = the sink ARN and Fn::GetAtt Arn). Real AWS therefore ACCEPTS a CreateStack whose Resources contain AWS::Oam::Sink and returns a StackId; the stack then drives the OAM control plane (the sink appears in oam ListSinks / GetSink / GetSinkPolicy, and deleting the stack removes it, after which GetSink answers ResourceNotFoundException with message "Resource not found."). AWS reserves the refusal the emulator gives — HTTP 400, code ValidationError, message "Template format error: Unrecognized resource types: [<type>, ...]" where the fixed text is "Template format error: Unrecognized resource types: [" + "]" and the variable part is the list of type names taken from the request's template — for types that genuinely are not in the registry; it is the correct wording for an unknown type, but AWS::Oam::Sink is not an unknown type. Smithy: cloudformation CreateStack declares no resource-type registry (the registry is service data, not model data), so the registry contents are recorded behavior only; the status/code/message above are taken from the emulator's response and the AWS template reference.

Emulator: CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Oam::Sink]" for any template declaring that type. Both OAM-sink deploy checks abort on the very first CreateStack, so nothing downstream of the stack is exercised at all: the rejected-Name path (early PROPERTY_VALIDATION producing a VALIDATION_ERROR row "Property value [<name>] does not match pattern: ^[a-zA-Z0-9_.-]+$" at path /Resources/Sink/Properties/Name followed by ROLLBACK_COMPLETE), the positive control (CREATE_COMPLETE with Ref/GetAtt Arn equal to the listed sink's Arn), the Policy and Tags read-back through GetSink/GetSinkPolicy, and the post-delete ResourceNotFoundException are all unreached.

Why: a client cannot provision an OAM monitoring-account sink declaratively. Every stack, CDK app or pipeline that includes an AWS::Oam::Sink fails at create with a ValidationError that says the type does not exist, which a client reads as "my template is wrong" rather than "the backend lacks the type" — so cross-account observability cannot be stood up at all, and the Ref/GetAtt contract (Ref = sink ARN) that downstream resources depend on is never available.

Fix: the CloudFormation resource-type registry and its AWS::Oam::Sink resource provider — register the type with its Name/Policy/Tags properties, its Name pattern validation (fed into early property validation so a bad Name produces the VALIDATION_ERROR row and a rollback instead of a create), its Ref/GetAtt Arn return values, and create/delete wiring into the oam service state so the sink is visible to ListSinks/GetSink/GetSinkPolicy and gone after stack delete.

Traces: cloudformation__oam.deploy-oam-sink-invalid-name-pattern-rejected-before-any-sink-is-created; cloudformation__oam.deploy-oam-sink-properties-getatt-and-sink-policy-read-back-then-resource-not-found-after-delete.

## Evidence
Expected (AWS): CreateStack with Resources.Sink = {"Type": "AWS::Oam::Sink", "Properties": {"Name": <name>, "Policy": <policy doc>, "Tags": {...}}} is accepted, returns a StackId, and the stack reaches CREATE_COMPLETE (or, with a Name violating ^[a-zA-Z0-9_.-]+$, ROLLBACK_COMPLETE after a FAIL-mode VALIDATION_ERROR row ("Sink", "PROPERTY_VALIDATION", "FAILED", "FAIL", "Property value [<name>] does not match pattern: ^[a-zA-Z0-9_.-]+$", "/Resources/Sink/Properties/Name")). On success oam ListSinks lists exactly one sink whose Name is the declared Name and whose Arn equals both the stack's Ref output and its Fn::GetAtt Arn output; GetSink returns that Name plus the declared tags; GetSinkPolicy returns the declared policy; after DeleteStack, GetSink answers ResourceNotFoundException with message "Resource not found."

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Oam::Sink]. HTTP 400. Both checks error out on this first call; no stack, no sink, and none of the OAM read-backs occur.

Message parts: "Template format error: Unrecognized resource types: [" ... "]" is AWS's fixed text for a truly unknown type and the bracketed list is taken from the request's template — the defect is that AWS::Oam::Sink is being classified as unknown, not the wording.

## Steps to Reproduce
1. Confirm the account/region has no OAM sink: oam ListSinks returns an empty Items list.
2. cloudformation CreateStack with TemplateBody = {"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Sink": {"Type": "AWS::Oam::Sink", "Properties": {"Name": "<valid-name>", "Policy": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"AWS": {"Fn::Sub": "arn:${AWS::Partition}:iam::${AWS::AccountId}:root"}}, "Action": ["oam:CreateLink", "oam:UpdateLink"], "Resource": "*"}]}, "Tags": {"sim2real-oam": "<value>"}}}}, "Outputs": {"SinkRef": {"Value": {"Ref": "Sink"}}, "SinkArn": {"Value": {"Fn::GetAtt": ["Sink", "Arn"]}}}}.
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Oam::Sink]" instead of a StackId.
4. Repeat step 2 with Name = "sim2real bad name!" — AWS reaches ROLLBACK_COMPLETE with a PROPERTY_VALIDATION FAIL row at /Resources/Sink/Properties/Name; the emulator gives the same CreateStack ValidationError, so the rejection path is never reached either.
