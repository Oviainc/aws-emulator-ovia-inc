Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive
Issue-Key: cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive|_poll_until#10|Timeout|last = probe()
Issue-Commit: defa25c70e8324a13a1647a7aa41447c10b54fda

# cloudformation: a stack whose only resource is a Lambda-backed custom resource never reaches a terminal CREATE status, so the custom resource's S3 write never happens

> _Note: this issue was identified against the code base at commit `defa25c70e8324a13a1647a7aa41447c10b54fda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CreateStack on a template whose sole resource is a Lambda-backed custom resource leaves the stack non-terminal indefinitely.

AWS: CloudFormation provisions a custom resource by invoking its ServiceToken Lambda with an event carrying RequestType 'Create', the ResponseURL, the StackId, the LogicalResourceId and the ResourceProperties, then waits for the function to signal SUCCESS to that URL. The stack then leaves CREATE_IN_PROGRESS for a terminal status — CREATE_COMPLETE on success, CREATE_FAILED/ROLLBACK_COMPLETE on failure — within the provisioning window, and DescribeStacks reports that terminal StackStatus. A subsequent UpdateStack that changes the custom resource's properties drives an 'Update' invocation and converges to UPDATE_COMPLETE, leaving the resource's earlier side effects intact. Recorded behavior only: the CloudFormation model binds no error or status machinery to custom resources; the requirement is that DescribeStacks converges to a terminal status and that the function's side effect (here, PutObject of the declared key and body, under a role scoped to s3:PutObject on that one bucket) is observable afterwards.

Emulator: the stack never reaches any of CREATE_COMPLETE / CREATE_FAILED / ROLLBACK_COMPLETE. The check polls DescribeStacks for a terminal status and is still polling when the harness cap fires, so the whole check ends with 'Check timed out after 60.0s'. Nothing downstream is reachable: the object the custom resource is supposed to write is never created, the UpdateStack leg is never driven, and no status or failure reason is ever reported to the caller.

Why: a client cannot distinguish "still provisioning" from "will never finish". Every CloudFormation workflow built on custom resources — CDK bucket deployments, SAM/CDK bootstrap assets, config seeding, anything that runs code during a stack lifecycle — blocks forever rather than succeeding or failing, and the waiter/poller in every SDK and in the AWS CLI (`cloudformation wait stack-create-complete`) never returns. A deploy that AWS completes in seconds hangs, and the least-privilege role under test is never exercised at all, so the policy's sufficiency cannot be observed.

Fix: the custom-resource provisioning path of the CloudFormation stack engine — it must invoke the resource's ServiceToken Lambda for the Create and Update request types, consume the SUCCESS/FAILED signal, and advance the stack to a terminal status (with a failure reason when the signal is FAILED or absent) so DescribeStacks converges. Neighbouring open issues cover other gaps in this area but the replay reports this check unowned by them: 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md, 20261105T085348Z-cloudformation-iam-lambda-s3-2-net-new-root-causes.md and 20261105T143253Z-cloudformation-lambda-s3-sqs-a-stack-whose-custom.md.

Traces: cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive.

## Evidence
Behavioral replay, check cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive (verdict: mismatch):

  Error: Check timed out after 60.0s

The check creates an IAM role whose only extra statement is {Effect: Allow, Action: s3:PutObject, Resource: arn:aws:s3:::<bucket>/*}, creates a Lambda with that role, then CreateStack on a template declaring a custom resource whose ServiceToken is that function, and polls:

  _poll_until(lambda: cfn.describe_stacks(StackName=stack)['Stacks'][0]['StackStatus'],
              lambda s: s in {'CREATE_COMPLETE','CREATE_FAILED','ROLLBACK_COMPLETE'},
              timeout_s=180, interval_s=5.0)

Expected (AWS): the poll returns CREATE_COMPLETE, GetObject on the declared key returns the declared body ('least-privilege-write-v1'); UpdateStack with a new key/body converges to UPDATE_COMPLETE, the v2 object exists and the v1 object is unchanged.
Actual (emulator): no terminal status is ever reported; the harness cap ends the check while DescribeStacks is still non-terminal, so neither object is written and the UpdateStack leg is never reached.

## Steps to Reproduce
1. Create a bucket and an IAM role trusting lambda.amazonaws.com whose only extra permission is s3:PutObject on arn:aws:s3:::<bucket>/*.
2. CreateFunction with that role; the handler PutObjects a key/body taken from the custom resource's ResourceProperties and signals SUCCESS to the ResponseURL.
3. CreateStack with a template whose only resource is a Custom:: resource with ServiceToken = that function's ARN and the key/body as properties.
4. Poll DescribeStacks for StackStatus. AWS reaches CREATE_COMPLETE; the emulator never leaves a non-terminal status.
5. GetObject the declared key: AWS returns the declared body; on the emulator the object does not exist.
6. UpdateStack with a different key/body and poll again: AWS reaches UPDATE_COMPLETE with both objects present.
7. Re-replay behavioral check cloudformation__iam__lambda__s3.cfn-custom-resource-role-least-privilege-positive to confirm.
