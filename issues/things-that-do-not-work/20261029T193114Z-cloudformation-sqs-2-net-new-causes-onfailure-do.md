Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cfn-on-failure-do-nothing-preserves-partial-resources-that-still-work cloudformation__sqs.cfn-update-replacement-changes-physical-id
Issue-Key: cloudformation__sqs.cfn-on-failure-do-nothing-preserves-partial-resources-that-still-work|run#14|AssertionError|assert queue_url, (
Issue-Key: sqs|GetQueueUrl|AWS.SimpleQueueService.NonExistentQueue
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# cloudformation__sqs: 2 net-new causes — OnFailure=DO_NOTHING discards the resources already created, and an update requiring replacement never creates the new queue

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes.

Cause 1 of 2: when a create fails under OnFailure=DO_NOTHING, the resources that were already created are not preserved and the stack reports no resources at all.
AWS: recorded behavior only for the retention semantics; the model fixes the surface (cloudformation CreateStack takes OnFailure with values DO_NOTHING | ROLLBACK | DELETE). With DO_NOTHING the stack settles in CREATE_FAILED and every resource that reached CREATE_COMPLETE before the failure is left in place and still listed by DescribeStackResources, so it remains usable.
Emulator: after the failure the stack's resource list is empty (`the stack's resources were {}`) and the queue created before the failing resource does not survive.
Why: DO_NOTHING exists precisely so an operator can inspect and reuse the partial deployment; deleting or forgetting those resources turns the documented debugging mode into an undocumented rollback and leaks or destroys live resources.
Fix: the CloudFormation create-failure path and the stack's resource state — honour OnFailure=DO_NOTHING by retaining created resources and keeping them enumerated.
Traces: cloudformation__sqs.cfn-on-failure-do-nothing-preserves-partial-resources-that-still-work

Cause 2 of 2: an UpdateStack that changes an immutable property (requiring replacement) does not create the replacement queue, so the new physical id does not resolve.
AWS: recorded behavior only. Changing a create-only property of AWS::SQS::Queue (e.g. QueueName) replaces the resource: CloudFormation creates the queue under the new physical id, repoints the logical id, then deletes the old one; GetQueueUrl on the new name succeeds after UPDATE_COMPLETE.
Emulator: sqs GetQueueUrl for the post-update physical id answers HTTP 400 `AWS.SimpleQueueService.NonExistentQueue` with message "The specified queue does not exist." — the replacement resource was never created.
Why: replacement is the normal consequence of renaming or re-keying a resource; a stack that reports a successful update while the new resource does not exist leaves every consumer of the new name broken.
Fix: the CloudFormation update/replacement planner and the AWS::SQS::Queue provider — a create-only property change must provision the new physical resource before the logical id is repointed.
Traces: cloudformation__sqs.cfn-update-replacement-changes-physical-id

## Evidence
Cause 1 — `AssertionError: the queue created before the failure must survive DO_NOTHING; the stack's resources were {}`.
Cause 2 — `botocore.errorfactory.QueueDoesNotExist: An error occurred (AWS.SimpleQueueService.NonExistentQueue) when calling the GetQueueUrl operation: The specified queue does not exist.`

## Steps to Reproduce
Cause 1: CreateStack(OnFailure='DO_NOTHING') with a template whose first resource is a valid AWS::SQS::Queue and whose second resource fails; after CREATE_FAILED call DescribeStackResources and GetQueueUrl on the first queue — the resource list is empty and the queue is gone.
Cause 2: deploy a stack with AWS::SQS::Queue QueueName=A, then UpdateStack with QueueName=B; after the update, GetQueueUrl(B) answers AWS.SimpleQueueService.NonExistentQueue.
