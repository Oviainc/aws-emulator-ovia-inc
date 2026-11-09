Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__iot__s3.cfn-iot-stream-create-and-delete-with-stack cloudformation__iam__iot__s3.cfn-iot-ota-update-create-and-delete-with-stack cloudformation__iam__iot__s3.cfn-iot-stream-nonexistent-s3-object-rejected
Issue-Key: cloudformation__iam__iot__s3|CreateStream|NotImplemented
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# IoT CreateStream answers NotImplemented, so AWS::IoT::Stream and AWS::IoT::OTAUpdate stacks cannot be created

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStream

## Description
Cause 1 of 1: the IoT CreateStream handler is an unimplemented scaffold stub, and both the stream stack and the OTA-update stack (which creates a stream for its file) depend on it.

AWS: CreateStream creates an IoT stream from one or more S3 file locations and a role ARN, answering 200 with streamId, streamArn, description and streamVersion 1; the file's S3 object must exist, and a nonexistent key is refused with a modelled client error, not a server error. Smithy: iot#CreateStream takes streamId, files[].{fileId, s3Location{bucket,key,version}} and roleArn, returns CreateStreamResponse{streamId, streamArn, description, streamVersion} and declares InvalidRequestException, ResourceNotFoundException, ResourceAlreadyExistsException, ThrottlingException, UnauthorizedException, InternalFailureException, ServiceUnavailableException, LimitExceededException. `NotImplemented` is not among them.

Emulator: CreateStream answers the non-AWS error code `NotImplemented` with message `operation CreateStream is scaffolded but not implemented in this emulator`. No AWS response ever carries that code or message.

Why: a client cannot create an IoT stream or an OTA update at all, and the error it receives is not in the operation's modelled error set, so its generated SDK error handling (retry classification, exception type) cannot match it.

Fix: implement the CreateStream handler and its stream state (including the nonexistent-S3-object refusal), in the IoT service's create_stream operation handler.

Traces: cloudformation__iam__iot__s3.cfn-iot-stream-create-and-delete-with-stack, cloudformation__iam__iot__s3.cfn-iot-ota-update-create-and-delete-with-stack, cloudformation__iam__iot__s3.cfn-iot-stream-nonexistent-s3-object-rejected.

## Evidence
Expected: CreateStream returns 200 with streamId/streamArn/streamVersion (and a modelled client error for a nonexistent S3 object).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStream operation: operation CreateStream is scaffolded but not implemented in this emulator

## Steps to Reproduce
1. Put a firmware file into an S3 bucket and create a role IoT can assume to read it.
2. Call iot:CreateStream with streamId, files[0].s3Location pointing at the object and roleArn (directly, or via a stack declaring AWS::IoT::Stream / AWS::IoT::OTAUpdate).
3. Observe the NotImplemented error.
