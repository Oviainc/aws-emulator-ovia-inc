Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__pipes__sqs.pipes-create-pipe-nonexistent-kms-key-identifier-rejected
Issue-Key: kms__pipes__sqs.pipes-create-pipe-nonexistent-kms-key-identifier-rejected|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# kms__pipes__sqs: CreatePipe accepts a KmsKeyIdentifier naming a KMS key that does not exist instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: CreatePipe stores KmsKeyIdentifier without resolving it in KMS, so a syntactically valid key ARN that the account never created is accepted and a pipe is created.
AWS: Smithy (com.amazonaws.pipes) — CreatePipe declares ValidationException (HTTP 400) and constrains KmsKeyIdentifier only by length/pattern, so the existence check is a runtime one. The recording shows AWS refusing the create with HTTP 400, Error.Code 'ValidationException' and the message "The provided KMS Key does not exist." — the whole message is fixed text, with no variable part — and creating nothing: the refused name does not appear in ListPipes. The same request with no KmsKeyIdentifier is accepted and the pipe reaches STOPPED.
Emulator: the create with the nonexistent key ARN answers HTTP 200 with the full pipe body ('Arn', 'Name', DesiredState 'STOPPED', CurrentState 'CREATING', CreationTime, LastModifiedTime), so the pipe exists and is bound to a key KMS does not know (DescribeKey on the same ARN correctly answers NotFoundException).
Why: a typo'd or cross-account key id produces a pipe that can never encrypt anything, and the failure surfaces later (or never) instead of at the call that can still be corrected; a client testing the validation guard passes vacuously.
Fix: the CreatePipe handler's KmsKeyIdentifier validation — resolve the key in KMS before any state is written, and refuse with the 400 ValidationException above.
Traces: kms__pipes__sqs.pipes-create-pipe-nonexistent-kms-key-identifier-rejected.

## Evidence
Expected ('ValidationException', 'The provided KMS Key does not exist.') with the pipe absent from ListPipes; actual AssertionError "expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': 'arn:aws:pipes:<region>:<account>:pipe/<name>', 'Name': '<name>', 'DesiredState': 'STOPPED', 'CurrentState': 'CREATING', ...}". The control leg in the same check confirms kms.describe_key on that ARN answers NotFoundException.

## Steps to Reproduce
1. Create two SQS queues and a pipe execution role for them; CreatePipe with no KmsKeyIdentifier as a control (reaches STOPPED).
2. Build a well-formed but unused key ARN, arn:aws:kms:<region>:<account>:key/<random uuid>, and confirm kms.describe_key answers NotFoundException.
3. CreatePipe(Name=<new name>, RoleArn, Source, Target, DesiredState='STOPPED', KmsKeyIdentifier=<that ARN>) — expect 400 ValidationException "The provided KMS Key does not exist." and the name absent from ListPipes; the emulator answers 200 and creates the pipe.
