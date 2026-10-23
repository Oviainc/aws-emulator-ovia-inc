Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group
Issue-Key: elasticbeanstalk__logs|PutObject|NoSuchBucket
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Elastic Beanstalk CreateStorageLocation returns an S3 bucket name that does not exist, so no source bundle can be uploaded

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStorageLocation

## Description
Cause 1 of 1: CreateStorageLocation reports a bucket it never creates.
AWS: CreateStorageLocation creates the account/region Elastic Beanstalk storage bucket (elasticbeanstalk-<region>-<account-id>) if it does not already exist and returns its name in S3Bucket; the returned bucket is immediately usable - PutObject into it succeeds and CreateApplicationVersion can reference it as the SourceBundle.
Emulator: the call returns an S3Bucket name, but the bucket is not present in S3: the immediately following PutObject fails with HTTP 404 NoSuchBucket ('The specified bucket does not exist').
Why: the documented first step of every Elastic Beanstalk deployment (upload the application source bundle to the EB storage location) cannot be performed, so no application version can be created or deployed. The response is actively misleading: it names a bucket that does not exist.
Fix: the CreateStorageLocation handler in the elasticbeanstalk service crate - create (idempotently) the storage bucket in the S3 state it names before returning S3Bucket.
Traces: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group.

## Evidence
botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the PutObject operation: The specified bucket does not exist - raised by s3.put_object(Bucket=eb.create_storage_location()['S3Bucket'], Key=..., Body=<app zip>). Expected: CreateStorageLocation's S3Bucket exists and accepts the PutObject (200).

## Steps to Reproduce
bucket = elasticbeanstalk.create_storage_location()['S3Bucket']; s3.put_object(Bucket=bucket, Key='app.zip', Body=b'...') -> 404 NoSuchBucket.
