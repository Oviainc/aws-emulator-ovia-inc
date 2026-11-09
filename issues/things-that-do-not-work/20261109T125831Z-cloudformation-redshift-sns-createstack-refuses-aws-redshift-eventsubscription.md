Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__redshift__sns.cfn-redshift-eventsubscription-round-trip-and-delete
Issue-Key: cloudformation__redshift__sns|CreateStack|ValidationError
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudformation__redshift__sns: CreateStack refuses AWS::Redshift::EventSubscription as an unrecognized resource type, so no Redshift event subscription can be deployed to an SNS topic from a template

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — AWS::Redshift::EventSubscription is not a registered CloudFormation resource type.
AWS: recorded behavior only (CloudFormation's resource-type registry is not part of the Smithy model): CreateStack accepts a template declaring AWS::Redshift::EventSubscription alongside an AWS::SNS::Topic, returns a StackId, provisions the subscription through Redshift CreateEventSubscription with the template's SnsTopicArn/SourceType/Severity/Enabled, resolves Ref to the subscription name, and DeleteStack removes it (DescribeEventSubscriptions then answers SubscriptionNotFound).
Emulator: CreateStack refuses the template with HTTP 400, code ValidationError and message 'Template format error: Unrecognized resource types: [AWS::Redshift::EventSubscription]' — fixed text 'Template format error: Unrecognized resource types: [' + the comma-separated list of the template's unregistered types + ']'.
Why: a Redshift event subscription cannot be deployed from a template at all, so the SNS-notified stack topology every client uses for Redshift events is undeployable; a template valid on AWS fails at parse time.
Fix: the CloudFormation resource-type registry and an AWS::Redshift::EventSubscription resource handler (create/read/update/delete mapped onto Redshift CreateEventSubscription / DescribeEventSubscriptions / ModifyEventSubscription / DeleteEventSubscription), with the Redshift refusal surfaced as the resource's failure reason.
Traces: cloudformation__redshift__sns.cfn-redshift-eventsubscription-round-trip-and-delete.

## Evidence
Replay of simparity-behavioral-aws-v1 (cloudformation__redshift__sns: 0/1 passed).

cloudformation__redshift__sns.cfn-redshift-eventsubscription-round-trip-and-delete: 'botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Redshift::EventSubscription]'.

Expected: CreateStack returns a StackId, the stack reaches CREATE_COMPLETE, and the subscription reads back through Redshift under the Ref name before DeleteStack removes it. Actual: no stack is created.

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::SNS::Topic and an AWS::Redshift::EventSubscription whose SnsTopicArn Refs that topic.
2. Observe 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::Redshift::EventSubscription]' instead of a StackId.
3. Re-replay the check listed in trace_ids.
