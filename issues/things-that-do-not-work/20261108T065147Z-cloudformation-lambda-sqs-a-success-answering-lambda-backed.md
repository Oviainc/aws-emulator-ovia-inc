Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals|_poll_until#20|Timeout|return last
Issue-Commit: 01d998bd46b5a2138daf20ca3b3d6fda22ea2f8c

# cloudformation__lambda__sqs: a SUCCESS-answering Lambda-backed AWS::CloudFormation::CustomResource never settles — the Create/Delete lifecycle callback, GetAtt Data and responder-minted physical id are never observable (check hangs to its 180 s budget)

> _Note: this issue was identified against the code base at commit `01d998bd46b5a2138daf20ca3b3d6fda22ea2f8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a stack containing an `AWS::CloudFormation::CustomResource` whose `ServiceToken` is a Lambda function ARN never reaches a terminal stack status, so the whole custom-resource callback protocol (Create signal on a presigned ResponseURL, response-body `Data` wired into `Fn::GetAtt`, responder-minted `PhysicalResourceId` adopted as `Ref`, and the matching Delete re-invocation on stack teardown) is unobservable. Failing check: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals.

AWS: recorded behavior only for the custom-resource protocol — CloudFormation's Smithy model carries no rule about it; it only models the terminal statuses (`StackStatus` enum: CREATE_IN_PROGRESS, CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, DELETE_COMPLETE, ...). The recording shows that on CreateStack with such a template CloudFormation (a) invokes the ServiceToken Lambda with a request document whose `ResponseURL` is genuinely presigned (the URL query carries `X-Amz-Signature`/`X-Amz-Credential`, or the SigV2 `Signature`/`AWSAccessKeyId`), (b) waits for the responder's PUT to that URL, (c) adopts the `PhysicalResourceId` from the response body as the value of `Ref` on that logical resource, (d) publishes the response body's `Data` members so `Fn::GetAtt <LogicalId>.<Key>` resolves to them in stack Outputs, (e) settles the stack in CREATE_COMPLETE, and (f) on DeleteStack re-invokes the SAME function with `RequestType: Delete` carrying the SAME physical id CloudFormation learned at Create time, then removes the stack so DescribeStacks refuses with `ValidationError` (HTTP 400) and message `Stack with id <StackName> does not exist` — fixed text `Stack with id ` / ` does not exist` around the stack name taken from the request. All of this completes well inside the check's 180 s budget against AWS.

Emulator: the check never gets past this flow and is killed by the harness — `Error: Check timed out after 180.0s`. No terminal stack status is reached inside the budget (the check's own create/delete waiters allow 900 s, so the hang is in the emulator's provisioning of the custom resource, not in a waiter that gave up early), so none of the Create record, the signed ResponseURL, the GetAtt Echo output, the responder-minted physical id, or the Delete record can be read. This is the SUCCESS leg of the same protocol; the already-open issue 20261107T081525Z-cloudformation-lambda-sqs-a-custom-resource-that-answers.md covers only the FAILED-response terminal status (ROLLBACK_FAILED vs ROLLBACK_COMPLETE) and does not carry this check, and 20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md is filed against a different service token and a CDK `Custom::CDKBucketDeployment` resource.

Why: a client that deploys any template containing a Lambda-backed custom resource — the standard extension point for CDK/SAM providers and for every bespoke provisioning hook — hangs forever instead of getting CREATE_COMPLETE. Stack Outputs that read provider `Data` through Fn::GetAtt never resolve, `Ref` on the resource never yields the provider's id, and teardown never tells the provider to release what it created, so deployment pipelines wedge and external resources leak.

Fix: the CloudFormation provisioner's custom-resource component in the cloudformation service — the resource handler for `AWS::CloudFormation::CustomResource`/`Custom::*` that must invoke the ServiceToken (Lambda) with a lifecycle request, mint and presign the ResponseURL, accept the PUT callback, record PhysicalResourceId and Data into stack state for Ref/Fn::GetAtt, drive the stack to a terminal status, and re-issue the Delete request during DeleteStack.

Traces: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals.

## Evidence
Expected (AWS, recorded): CreateStack with an SQS queue + IAM role + python3.12 responder Lambda + AWS::CloudFormation::CustomResource (CAPABILITY_IAM) settles CREATE_COMPLETE; Outputs.EchoedByCustomResource == the EchoValue parameter (via Fn::GetAtt Recorded.Echo); Outputs.CustomResourcePhysicalId == the id the responder minted (prefix `sim2real-xsvc-cr-`); the out-of-stack recorder queue holds a RequestType=Create record whose ResponseURL was presigned and whose PhysicalResourceId matches that Ref; after DeleteStack a RequestType=Delete record arrives with the SAME physical id; DescribeStacks then fails with ValidationError / `Stack with id <StackName> does not exist`.

Actual (emulator): the check never completes — harness verdict `[mismatch] ... Error: Check timed out after 180.0s`. The custom-resource create never settles, so no assertion past the deploy is ever reached; the check's internal deploy and delete waiters were given 900 s each, so the 180 s expiry is a hang, not an impatient poll.

## Steps to Reproduce
1. CreateQueue an out-of-stack recorder queue (MessageRetentionPeriod=3600) and note its URL and name.
2. CreateStack with a template containing: an SQS queue, an IAM role for the responder (sqs:SendMessage to the recorder queue), an AWS::Lambda::Function (python3.12) that records the lifecycle request to the recorder queue and PUTs {"Status":"SUCCESS","PhysicalResourceId":"sim2real-xsvc-cr-<hex>","Data":{"Echo":"<EchoValue>"}} to event['ResponseURL'], and an AWS::CloudFormation::CustomResource whose ServiceToken is !GetAtt <Responder>.Arn; Outputs expose !GetAtt Recorded.Echo and !Ref Recorded. Pass Capabilities=[CAPABILITY_IAM].
3. Poll DescribeStacks for a terminal StackStatus: AWS reaches CREATE_COMPLETE; the emulator stays non-terminal and the check is cut off at 180 s.
4. ReceiveMessage on the recorder queue: expect a RequestType=Create record with a presigned ResponseURL (X-Amz-Signature/X-Amz-Credential or Signature/AWSAccessKeyId in the query) and PhysicalResourceId equal to Ref of the custom resource.
5. DeleteStack and wait for DELETE_COMPLETE, then ReceiveMessage again: expect a RequestType=Delete record carrying the same physical id, and DescribeStacks to answer ValidationError `Stack with id <StackName> does not exist`.
