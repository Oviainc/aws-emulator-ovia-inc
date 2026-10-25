Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.e2e-model-package-approval-to-endpoint-deploy-flow
Issue-Key: sagemaker|GetObject|NoSuchBucket
Issue-Commit: 41791a89e1771892a99a46c0bbc6fb2f1f57a7b6

# SageMaker e2e flow cannot stage training input: the AWS-managed public sample-data bucket `sagemaker-sample-files` does not exist in the emulator's S3 (GetObject -> 404 NoSuchBucket)

> _Note: this issue was identified against the code base at commit `41791a89e1771892a99a46c0bbc6fb2f1f57a7b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: a read of the AWS-managed, world-readable SageMaker sample-data bucket is refused 404 NoSuchBucket, so the whole model-package -> endpoint end-to-end flow dies before it issues a single SageMaker call. Failing check: sagemaker.e2e-model-package-approval-to-endpoint-deploy-flow.

AWS: `sagemaker-sample-files` is an AWS-owned public bucket that SageMaker's documentation and tutorials read training data from; an unauthenticated-or-any-caller GetObject on an existing key in it returns 200 with the object body. Smithy (com-amazonaws-s3-amazons3): GetObject GET /{Bucket}/{Key+}, modeled errors InvalidObjectState (403) and NoSuchKey (404); NoSuchBucket (404) is S3's common error for a bucket that does not exist, so by the model the only way AWS answers NoSuchBucket is when the bucket is genuinely absent. The bucket's existence and the presence of the published sample objects under `datasets/...` is recorded behavior only (the repo's models do not enumerate managed buckets) — the recording shows the GetObject of `datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv` succeeding and returning CSV rows.

Emulator: the same GetObject is answered 404 `NoSuchBucket` ("The specified bucket does not exist"), raised by botocore as `botocore.errorfactory.NoSuchBucket` from the check's `_stage_sample_data` helper. The emulator's S3 state is seeded only with buckets a client creates, so no AWS-managed public bucket resolves. Nothing SageMaker-side was ever exercised in this check as a result — CreateTrainingJob, CreateModelPackageGroup, CreateModelPackage, CreateModel, CreateEndpointConfig, CreateEndpoint, InvokeEndpoint and the teardown assertions after it are all unreached.

Why: any client that follows the documented SageMaker path — copy a published sample dataset out of the AWS-managed sample bucket, then train/deploy from its own bucket — fails at step one against the emulator, with an error (NoSuchBucket) that tells the client its *input data source* is gone rather than that the emulator lacks the managed fixture. It also masks the fidelity of the whole downstream SageMaker surface: the e2e check can never report on it. Hypothesis: a pre-seeded read-only bucket holding the handful of published sample keys (or a resolver that synthesizes well-known AWS-managed public buckets) is what is missing; the recording does not state how AWS hosts it.

Fix: S3 state seeding / bucket resolution in the s3 service crate (`services/service-com-amazonaws-s3-amazons3/src/...` state + the GetObject/HeadObject lookup path) so AWS-managed public buckets such as `sagemaker-sample-files` resolve for read. This is NOT the same cause as the already-open `20261022T210606Z-sagemaker-associatetrialcomponent-deletenotebookinstance-createpipeline-from-s3-answer-notimplemented.md`, which covers the absent managed SageMakerPublicHub (a SageMaker-side managed resource); that issue does not cover this bucket and this check is not marked owned by it.

Traces: sagemaker.e2e-model-package-approval-to-endpoint-deploy-flow (verdict: error; GetObject on Bucket=`sagemaker-sample-files`, Key=`datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv`).

## Evidence
Expected (real AWS): s3.get_object(Bucket='sagemaker-sample-files', Key='datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv') -> HTTP 200 with a CSV body, whose first line the check splits into label + feature row.

Actual (emulator): HTTP 404, error code `NoSuchBucket`. Replay output:
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the GetObject operation: The specified bucket does not exist

The recorded message text above is an example; the requirements that diverge are the status (200 vs 404) and the error code (none vs NoSuchBucket). Failure site in the check: `_stage_sample_data()` -> `s3.get_object(Bucket=_SAMPLE_DATA_BUCKET, Key=_SAMPLE_DATA_KEY)`, which runs before the first SageMaker API call, so 100% of the check's SageMaker assertions are unreached.

## Steps to Reproduce
1. Boot the emulator and point a botocore/boto3 S3 client at it.
2. Call GetObject with Bucket='sagemaker-sample-files', Key='datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv' (no prior CreateBucket).
3. Observe HTTP 404 NoSuchBucket; real AWS returns 200 and the CSV body for this public AWS-managed bucket.
4. Equivalently, replay behavioral check sagemaker.e2e-model-package-approval-to-endpoint-deploy-flow: it aborts in _stage_sample_data before issuing any SageMaker call.
