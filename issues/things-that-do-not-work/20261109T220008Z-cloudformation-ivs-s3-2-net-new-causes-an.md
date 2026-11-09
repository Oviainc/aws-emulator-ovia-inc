Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ivs__s3.deploy-ivs-recording-configuration-nonexistent-bucket-rejected-and-rolled-back cloudformation__ivs__s3.deploy-ivs-recording-configuration-s3-destination-round-trip-and-delete
Issue-Key: cloudformation__ivs__s3.deploy-ivs-recording-configuration-nonexistent-bucket-rejected-and-rolled-back|run#12|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__ivs__s3.deploy-ivs-recording-configuration-s3-destination-round-trip-and-delete|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__ivs__s3: 2 net-new causes — an IVS RecordingConfiguration naming a nonexistent bucket reaches CREATE_COMPLETE instead of rolling back, and stack Outputs entries carry no OutputValue

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two causes.

Cause 1 of 2: AWS::IVS::RecordingConfiguration whose S3 destination bucket does not exist is created instead of failing the stack.
AWS: IVS CreateRecordingConfiguration validates the destination bucket at create time and fails when it is absent (ValidationException / the resource's CREATE_FAILED), so a stack declaring such a configuration ends ROLLBACK_COMPLETE and leaves no recording configuration behind. Smithy: ivs CreateRecordingConfiguration takes destinationConfiguration.s3.bucketName and models ValidationException/ConflictException/AccessDeniedException — the absent-bucket case is a refusal, not a success.
Emulator: the stack ends 'CREATE_COMPLETE' — the resource handler accepts the nonexistent bucket and reports success.
Why: a caller gets a green stack that can never record anything; the misconfiguration surfaces (if ever) only at stream time, and the negative IaC test that is supposed to protect against typo'd buckets passes silently.
Fix: the AWS::IVS::RecordingConfiguration create handler (and the underlying CreateRecordingConfiguration validation) must resolve the destination bucket and fail the resource, so CloudFormation rolls the stack back.
Traces: cloudformation__ivs__s3.deploy-ivs-recording-configuration-nonexistent-bucket-rejected-and-rolled-back.

Cause 2 of 2: DescribeStacks returns Outputs entries without OutputValue.
AWS: each DescribeStacks Stacks[].Outputs[] member carries OutputKey and OutputValue (and optionally Description/ExportName) once the stack is CREATE_COMPLETE; Smithy: cloudformation Output members OutputKey, OutputValue, Description, ExportName. A resolved output always has its value.
Emulator: the output entry is present but has no 'OutputValue' member at all — reading it raises KeyError: 'OutputValue' — so the stack's declared outputs (here the recording configuration's ARN) are unreadable.
Why: every consumer that threads a stack output into the next step (Fn::ImportValue, CLI `--query 'Stacks[0].Outputs'`, CDK cross-stack references) gets nothing, and an SDK caller crashes on a member the API guarantees.
Fix: the CloudFormation DescribeStacks serializer / stack-output resolution must populate OutputValue for every resolved output.
Traces: cloudformation__ivs__s3.deploy-ivs-recording-configuration-s3-destination-round-trip-and-delete. The same missing member is filed for another heading in 20261029T065950Z-cloudformation-dynamodb-s3-describestacks-returns-stack-outputs-entries.md, which does not carry this check's replay key.

## Evidence
Cause 1: AssertionError: rejected stack ended 'CREATE_COMPLETE' (expected ROLLBACK_COMPLETE) for a stack whose AWS::IVS::RecordingConfiguration names a bucket that was never created.
Cause 2: KeyError: 'OutputValue' raised at check_cloudformation__ivs__s3_deploy_ivs_recording_configuration_s3_destination_round_trip_and_delete.py line 192 while reading DescribeStacks Stacks[0].Outputs[...] of a CREATE_COMPLETE stack.

## Steps to Reproduce
Cause 1: create_stack with an AWS::IVS::RecordingConfiguration whose DestinationConfiguration.S3.BucketName names a bucket that does not exist; wait for the terminal status -> CREATE_COMPLETE instead of ROLLBACK_COMPLETE.
Cause 2: create_stack with an AWS::IVS::RecordingConfiguration plus an Outputs section exporting its ARN; after CREATE_COMPLETE call describe_stacks and read Stacks[0].Outputs[0]['OutputValue'] -> the member is absent.
