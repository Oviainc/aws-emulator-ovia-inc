Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs__s3.serverless-remove-bucket-not-empty-blocks-stack-delete
Issue-Key: cloudformation__iam__lambda__logs__s3.serverless-remove-bucket-not-empty-blocks-stack-delete|_npm_install_serverless#12|Timeout|r = subprocess.run(
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# cloudformation__iam__lambda__logs__s3: a stack delete blocked by a non-empty S3 bucket reaches no verdict — the delete never settles in DELETE_FAILED (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: deleting a stack whose own S3 bucket still holds an object never reaches a terminal stack status, so neither the blocked delete nor the successful retry can be graded.
AWS: DeleteStack on a stack owning a non-empty AWS::S3::Bucket settles in DELETE_FAILED, and the bucket's resource status reason is S3's BucketNotEmpty refusal ('The bucket you tried to delete is not empty'); after the bucket is emptied the retried DeleteStack reaches DELETE_COMPLETE and every resource is gone. The fixed text is the BucketNotEmpty sentence; the variable part is the logical/physical bucket id in the resource-status row.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'. The stack never reports DELETE_FAILED (nor DELETE_COMPLETE) within the budget.
Why: a teardown pipeline cannot distinguish 'delete blocked because a bucket is not empty' from a hung delete, so the documented remediate-and-retry loop cannot be implemented.
Fix: the cloudformation DeleteStack orchestration and the AWS::S3::Bucket delete handler that must surface S3's BucketNotEmpty refusal as the resource's status reason and settle the stack in DELETE_FAILED.
Traces: cloudformation__iam__lambda__logs__s3.serverless-remove-bucket-not-empty-blocks-stack-delete.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: stack status DELETE_FAILED with the bucket's ResourceStatusReason carrying S3's BucketNotEmpty text, then DELETE_COMPLETE after the bucket is emptied. Actual: no terminal stack status inside the cap. (The check drives the stack through the serverless CLI via npm/node, which it declares as required tools; the tools were present — the run reached the delete phase and hung there rather than failing to start.)

## Steps to Reproduce
Replay cloudformation__iam__lambda__logs__s3.serverless-remove-bucket-not-empty-blocks-stack-delete: deploy the serverless stack, put an object in the stack's own bucket, run the stack delete and poll DescribeStacks for a terminal status.
