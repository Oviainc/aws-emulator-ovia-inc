Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.deploy-cfn-stream-s3-destination-config-applied
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|PutRecord|ResourceNotFoundException
Issue-Commit: 9fe95bf5a6f42fe93a77d740b9cb51055f43a772

# REOPENS: a CFN-deployed Firehose delivery stream never leaves CREATING, so PutRecord on it is refused after the stack reports CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `9fe95bf5a6f42fe93a77d740b9cb51055f43a772`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
REOPENS 20261022T064119Z-firehose-cloudformation-deployed-firehose-delivery-streams-are-never-provisioned.md: its fix left the call refused.

That issue carried this very check id (firehose.deploy-cfn-stream-s3-destination-config-applied) and the replay key `firehose|PutRecord|ResourceNotFoundException`. The stream declared by the template is now materialised in Firehose state (PutRecord resolves it by name instead of answering 'not found'), but it never reaches ACTIVE while the CloudFormation stack already reports CREATE_COMPLETE, so the same operation is still refused with the same error code.

Cause 1 of 1: a stack whose template declares an AWS::KinesisFirehose::DeliveryStream is reported complete while the stream is still in status CREATING, and the stream does not transition to ACTIVE on its own in time, so the first data-plane call against it is refused.
AWS: recorded behavior only (CloudFormation resource stabilization is not expressed in the Firehose Smithy model). The recording shows that when the CreateStack waiter for stack_create_complete returns, DescribeDeliveryStream on the template's stream reports DeliveryStreamStatus ACTIVE and PutRecord against it succeeds with a RecordId (Smithy: PutRecord output member RecordId is required; modeled errors are InvalidArgumentException, InvalidKMSResourceException, InvalidSourceException, ResourceNotFoundException (client, HTTP 400) and ServiceUnavailableException (server, 503)). CloudFormation does not mark a delivery-stream resource CREATE_COMPLETE until the stream has stabilized to ACTIVE.
Emulator: the stack create waiter returns, and the immediately following PutRecord is refused with HTTP 400, error code ResourceNotFoundException (example message: 'Firehose stream <name> is in status CREATING; it accepts data only in status ACTIVE'). The check therefore never reaches its S3 prefix/content assertions.
Why: every infrastructure-as-code workflow breaks at its first use of what it just deployed — a client that correctly waits for the stack to complete and then writes to the stream it declared gets a 400 that, against AWS, cannot happen. Worse, the error code is ResourceNotFoundException, so a client's retry/error branch concludes the stream does not exist rather than that it is not ready.
Fix: two places, either of which closes the divergence and both of which should be correct — (a) the CloudFormation stack-create resource provisioner must not report the AWS::KinesisFirehose::DeliveryStream resource (and therefore the stack) complete until the created stream has reached ACTIVE in Firehose state, and (b) the Firehose delivery-stream lifecycle in the Firehose service crate must actually advance a created stream from CREATING to ACTIVE. Related but not covering this check: the open issue 20261025T002604Z-cloudwatch-firehose-s3-cloudformation-deployed-metric-stream-pipeline-the-stack-reports.md describes the same CREATE_COMPLETE-while-CREATING symptom for the cloudwatch__firehose__s3 check ids and does not carry this check's replay key.
Traces: firehose.deploy-cfn-stream-s3-destination-config-applied (handler under services/service-com-amazonaws-firehose-firehose-20150804/src/ops/put_record.rs plus the CloudFormation create-stack resource dispatch in services/service-com-amazonaws-cloudformation-cloudformation/).

## Evidence
Expected: after cfn.get_waiter('stack_create_complete').wait(...) on a template declaring an AWS::KinesisFirehose::DeliveryStream (DirectPut, S3DestinationConfiguration with BucketARN/RoleARN/Prefix/BufferingHints), firehose.put_record(DeliveryStreamName=<templated name>, Record={'Data': b'marker\n'}) returns 200 with a RecordId, and the record later lands under the templated prefix.
Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutRecord operation: Firehose stream sim2real-behav-fh-stream-f7a05f712d7e is in status CREATING; it accepts data only in status ACTIVE (HTTP 400). The status and error code are the requirement here; the quoted message is an example.

## Steps to Reproduce
1. Boot the emulator from this checkout and point botocore at it.
2. cfn.create_stack with a template declaring a bucket, a Firehose delivery role and an AWS::KinesisFirehose::DeliveryStream (DeliveryStreamType DirectPut, S3DestinationConfiguration with BucketARN/RoleARN/Prefix and BufferingHints IntervalInSeconds=60, SizeInMBs=1), Capabilities=['CAPABILITY_NAMED_IAM'].
3. Wait with the stack_create_complete waiter.
4. Call firehose.describe_delivery_stream on the templated stream name — observe DeliveryStreamStatus CREATING rather than ACTIVE.
5. Call firehose.put_record on it — observe HTTP 400 ResourceNotFoundException instead of a 200 with a RecordId.
6. Re-replay firehose.deploy-cfn-stream-s3-destination-config-applied.
