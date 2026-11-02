Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service cloudformation__cloudtrail.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited
Issue-Key: cloudformation__cloudtrail.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service|_wait_stack#43|AssertionError|raise AssertionError(
Issue-Key: cloudformation__cloudtrail.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited|run#40|AssertionError|assert msg == "The specified queue does not exist.", (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudFormation cannot update AWS::SQS::Queue properties in place (stack reaches UPDATE_FAILED) and SQS QueueDoesNotExist uses a non-AWS message

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two distinct causes, both surfaced by the CloudTrail/CloudFormation cross-service checks. The check ids carry the composite service token 'cloudtrail__cloudformation', so the per-service bootstrap gate could not place them by exact equality; both constituent services (cloudtrail, cloudformation) and the SQS service the second cause belongs to are stood up, and neither appears in the pending set, so this is filed rather than deferred. There are 2 distinct causes in this issue; both must be closed.

Cause 1 of 2 — updating an AWS::SQS::Queue's properties in a stack fails the whole stack update.
AWS: recorded behavior only: an UpdateStack that changes a mutable property of an existing AWS::SQS::Queue (for example a queue attribute) is applied in place and the stack reaches UPDATE_COMPLETE; the resource is not replaced and no resource failure is reported.
Emulator: the stack reached UPDATE_FAILED with the status reason 'The following resource(s) failed to update: [Queue]. NotImplemented: this emulator does not change the properties of an existing AWS::SQS::Queue in place; the resource was left as it is.' The check's subject stack is left in a rolled-back state and every downstream assertion about the update being audited is unreachable.
Why: any infrastructure-as-code flow that evolves a queue's configuration cannot be exercised; a client sees a terminal stack failure for a change AWS applies, and must assume its template is wrong.
Fix: the CloudFormation resource provider for AWS::SQS::Queue must implement the in-place update path (SetQueueAttributes / tag updates) instead of returning NotImplemented from its update hook.
Traces: cloudtrail__cloudformation.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service.

Cause 2 of 2 — SQS GetQueueUrl's QueueDoesNotExist refusal carries a non-AWS message.
AWS: recorded behavior only: GetQueueUrl for a queue name that does not exist is refused with error code AWS.SimpleQueueService.NonExistentQueue / QueueDoesNotExist and the fixed message 'The specified queue does not exist.' The message is a constant AWS string that deliberately does not echo the queue name, so the rule producing it is known.
Emulator: the refusal carries 'The queue named sim2real-xsvc-qbad-0934df36cb53 does not exist in this account and Region.' — it echoes the requested name, which AWS does not.
Why: a client matching on AWS's documented text does not recognise the absence, and the emulator's message discloses the probed name back to the caller where AWS's does not.
Fix: the GetQueueUrl handler's QueueDoesNotExist error construction in the SQS crate must emit AWS's fixed message.
Traces: cloudtrail__cloudformation.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited.

## Evidence
Cause 1: AssertionError: stack arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-stk-8cbeecd57140/f2b214fe-bcee-3178-3545-55fe9c280a29 reached 'UPDATE_FAILED' instead of 'UPDATE_COMPLETE': 'The following resource(s) failed to update: [Queue]. NotImplemented: this emulator does not change the properties of an existing AWS::SQS::Queue in place; the resource was left as it is.'
Cause 2: assert msg == "The specified queue does not exist." failed — AssertionError: GetQueueUrl('sim2real-xsvc-qbad-0934df36cb53') refused with an unexpected message: 'The queue named sim2real-xsvc-qbad-0934df36cb53 does not exist in this account and Region.'

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay cloudtrail__cloudformation.obs-cloudformation-1-stack-lifecycle-is-audited-as-the-caller-and-provisioning-as-the-service and cloudtrail__cloudformation.create-stack-with-a-role-cloudformation-cannot-assume-is-rejected-and-audited.
3. Manually: (a) CreateStack with a template containing an AWS::SQS::Queue, wait for CREATE_COMPLETE, then UpdateStack changing one of the queue's properties -> expect UPDATE_COMPLETE, observe UPDATE_FAILED with the NotImplemented resource reason; (b) sqs GetQueueUrl(QueueName='<a name that does not exist>') and compare the error message against 'The specified queue does not exist.'
