Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative
Issue-Key: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative|_expect_rejected#5|AssertionError|raise AssertionError(f"expected {description} to be refused, but it SUCCEEDED")
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# lambda__opensearch__s3: PutBucketNotificationConfiguration skipped the invoke-permission check on a Lambda destination (RESOLVED)

> _This record is what is left of a five-cause bundle filed as
> `20261029T182036Z-lambda-opensearch-s3-lambda-driven-s3-opensearch-pipelines.md`. The
> cause below — cause 5 of that bundle — is FIXED. The bundle's other four causes are NOT
> fixed and were re-filed, verbatim and with their own trace ids and keys, as
> `issues/things-that-do-not-work/20261103T020000Z-lambda-opensearch-s3-invoke-execution-and-cloudformation-stack-causes.md`
> so that nothing about them is reported as working._

## Operation
PutBucketNotificationConfiguration

## Description
At `PutBucketNotificationConfiguration` time S3 checks that it can invoke the named
destination function. Without a resource-based policy statement on that function allowing
`s3.amazonaws.com` to `lambda:InvokeFunction` (for that source bucket/account), the call is
REFUSED with HTTP 400 and the error code `InvalidArgument`, and the notification
configuration does NOT persist; after `AddPermission` grants the invoke, the identical call
succeeds. The emulator accepted the call with no permission check at all and persisted the
configuration.

## Resolution
`services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs` now resolves each
`LambdaFunctionConfiguration` destination through `GetFunctionConfiguration`, reads that
function's resource-based policy through `GetPolicy`, and evaluates it with the shared IAM
evaluator (`service_common::iam_authorization`) for the `s3.amazonaws.com` principal, the
`lambda:InvokeFunction` action, the resolved function ARN and the `aws:SourceArn` /
`aws:SourceAccount` context of the source bucket. A destination that is not granted fails
the container's validation, so the whole put is refused with 400 `InvalidArgument` and
nothing is recorded — the atomic behaviour this operation documents.

Pinned by `services/service-com-amazonaws-s3-amazons3/tests/all/notification_destination_validation.rs`,
whose table spans no grant, a grant for this bucket, a grant for every source, a grant
naming another bucket, another principal, another action, a mixed container and a put that
skips destination validation.

## Steps to Reproduce (before the fix)
1. CreateBucket; CreateFunction.
2. PutBucketNotificationConfiguration with a `LambdaFunctionConfiguration` for
   `s3:ObjectCreated:*` and NO `AddPermission` for `s3.amazonaws.com` — the emulator
   answered 200 and `GetBucketNotificationConfiguration` reported the configuration as
   stored, where AWS answers 400 `InvalidArgument` and stores nothing.
