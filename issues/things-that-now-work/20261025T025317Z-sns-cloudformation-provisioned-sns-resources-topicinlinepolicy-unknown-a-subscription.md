Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy sns.deploy-cfn-subscription-filter-policy-behaves sns.deploy-cfn-subscription-redrive-policy-dead-letters-an-undeliverable-message sns.deploy-cfn-fifo-topic-delivers-in-order-and-deduplicates-by-content sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|CreateStack|ValidationError
Issue-Key: sns|GetSubscriptionAttributes|InvalidParameter
Issue-Key: sns.deploy-cfn-fifo-topic-delivers-in-order-and-deduplicates-by-content|_create_stack#4|WaiterError|cfn.get_waiter("stack_create_complete").wait(
Issue-Key: sns|DescribeKey|NotFoundException
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# CloudFormation-provisioned SNS resources: TopicInlinePolicy unknown, a Subscription's Ref is not an ARN, a FIFO topic stack fails, and a stack's KMS key is unknown to KMS (4 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
These four causes are grounded in the sns.deploy-cfn-* checks, which provision SNS topologies through CloudFormation before exercising SNS.

Cause 1 of 4: AWS::SNS::TopicInlinePolicy is not a recognized resource type.
AWS: CreateStack accepts a template declaring AWS::SNS::TopicInlinePolicy (it writes the topic's inline access policy; removing it restores the default policy).
Emulator: CreateStack is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SNS::TopicInlinePolicy]".
Why: templates that manage topic policy the modern way cannot be deployed at all.
Fix: the CloudFormation resource-type registry / SNS resource providers.
Traces: sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy.

Cause 2 of 4: an AWS::SNS::Subscription's physical id is not a subscription ARN.
AWS: Ref on an AWS::SNS::Subscription yields the subscription ARN (topic ARN + ":" + uuid), so GetSubscriptionAttributes on it succeeds.
Emulator: the value handed back has a single ARN element — GetSubscriptionAttributes refuses it with 400 InvalidParameter "Invalid parameter: SubscriptionArn Reason: An ARN must have at least 6 elements, not 1".
Why: any stack that outputs or consumes a subscription reference is unusable; nothing downstream can read or set that subscription's attributes.
Fix: the AWS::SNS::Subscription provider's physical-resource-id (and Ref) in CloudFormation.
Traces: sns.deploy-cfn-subscription-filter-policy-behaves, sns.deploy-cfn-subscription-redrive-policy-dead-letters-an-undeliverable-message.

Cause 3 of 4: a stack declaring a FIFO topic ends CREATE_FAILED.
AWS: a template with an AWS::SNS::Topic carrying FifoTopic/ContentBasedDeduplication reaches CREATE_COMPLETE.
Emulator: the StackCreateComplete waiter sees CREATE_FAILED ("Waiter encountered a terminal failure state ... CREATE_FAILED").
Why: FIFO topics cannot be deployed by template, so ordered/deduplicated pipelines cannot be stood up.
Fix: the AWS::SNS::Topic provider's handling of the FIFO attributes in CloudFormation.
Traces: sns.deploy-cfn-fifo-topic-delivers-in-order-and-deduplicates-by-content.

Cause 4 of 4: an AWS::KMS::Key created by a stack is not registered in KMS, and its physical id is a logical name.
AWS: the stack's key is a real KMS key; DescribeKey on the Ref'd value succeeds and the id is a key id/ARN, not "<stack>-Key-<suffix>".
Emulator: DescribeKey answers 404 NotFoundException: Key 'arn:aws:kms:us-west-2:000000000000:key/<stack>-Key-<suffix>' does not exist.
Why: any encrypted-topic/queue stack is undeployable end to end; the key the template created cannot be inspected, rotated or used.
Fix: the AWS::KMS::Key provider in CloudFormation (register the key with KMS and return its key id).
Traces: sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext.

Related resolved issues on the same provider surface (not covering these causes): 20261023T182128Z-cloudformation-never-provisions-aws-sns-topic-into-sns.md, 20261023T234648Z-cloudformation-provisions-a-stack-s-aws-sns-topic.md.

## Evidence
- sns.deploy-cfn-removing-a-topic-inline-policy-restores-the-default-policy: ClientError (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SNS::TopicInlinePolicy]
- sns.deploy-cfn-subscription-filter-policy-behaves and sns.deploy-cfn-subscription-redrive-policy-dead-letters-an-undeliverable-message: InvalidParameterException (InvalidParameter) when calling the GetSubscriptionAttributes operation: Invalid parameter: SubscriptionArn Reason: An ARN must have at least 6 elements, not 1
- sns.deploy-cfn-fifo-topic-delivers-in-order-and-deduplicates-by-content: WaiterError: Waiter StackCreateComplete failed: ... we matched expected path: "CREATE_FAILED" at least once
- sns.deploy-cfn-kms-encrypted-topic-and-queue-deliver-plaintext: NotFoundException (NotFoundException) when calling the DescribeKey operation: Key 'arn:aws:kms:us-west-2:000000000000:key/sim2real-behav-sns-cfnkms-...-Key-...' does not exist

## Steps to Reproduce
1. CreateStack with a template declaring AWS::SNS::TopicInlinePolicy -> 400 ValidationError.
2. CreateStack with AWS::SNS::Topic + AWS::SQS::Queue + AWS::SNS::Subscription; wait CREATE_COMPLETE; GetSubscriptionAttributes(Ref of the subscription) -> 400 InvalidParameter (ARN has 1 element).
3. CreateStack with an AWS::SNS::Topic having FifoTopic=true -> CREATE_FAILED.
4. CreateStack with an AWS::KMS::Key; DescribeKey(Ref of the key) -> 404 NotFoundException.
