Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__personalize.cfn-personalize-datasetgroup-schema-dataset-eventtracker-read-back-then-deleted cloudformation__personalize.cfn-personalize-dataset-invalid-type-rejected-leaving-valid-dataset
Issue-Key: cloudformation__personalize|CreateStack|ValidationError
Issue-Key: cloudformation__personalize.cfn-personalize-dataset-invalid-type-rejected-leaving-valid-dataset|run#15|AssertionError|assert dg_name_a in groups and schema_name_a in schemas, (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__personalize: 2 net-new causes — AWS::Personalize::EventTracker unregistered, and dataset groups/schemas a stack creates are not listed by Personalize

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes. Each is numbered with its own failing check id; both must be closed before this issue is done.

Cause 1 of 2: AWS::Personalize::EventTracker is not a recognized resource type.
AWS: it is a public CloudFormation resource type, so CreateStack accepts a template declaring it alongside the dataset group, schema and dataset, and provisions the event tracker.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::Personalize::EventTracker]`.
Why: the whole stack is refused, so a Personalize estate with an event tracker cannot be deployed at all.
Fix: register AWS::Personalize::EventTracker in the CloudFormation resource-type registry and add its resource handler.
Traces: cloudformation__personalize.cfn-personalize-datasetgroup-schema-dataset-eventtracker-read-back-then-deleted.

Cause 2 of 2: the dataset group and schema a stack reports CREATE_COMPLETE are not listed by Personalize.
AWS: after CREATE_COMPLETE, ListDatasetGroups returns the dataset group the stack created and ListSchemas returns its schema, each findable by the ARN the stack's Ref reports.
Emulator: neither is listed — the lookup yields None for both the control dataset group and the control schema.
Why: a client that provisions a Personalize dataset group and schema with CloudFormation and then creates datasets or solutions against them finds nothing in Personalize.
Fix: the AWS::Personalize::DatasetGroup and AWS::Personalize::Schema resource handlers must create the entities in Personalize state.
Traces: cloudformation__personalize.cfn-personalize-dataset-invalid-type-rejected-leaving-valid-dataset.

## Evidence
Cause 1: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Personalize::EventTracker].
Cause 2: AssertionError: 'control dataset group / schema not listed: None, None' — expected both present with the ARNs the stack reported.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateStack with a template declaring AWS::Personalize::EventTracker.
3. Cause 2: deploy AWS::Personalize::DatasetGroup + Schema, then call ListDatasetGroups and ListSchemas and look for the ARNs the stack reports.
