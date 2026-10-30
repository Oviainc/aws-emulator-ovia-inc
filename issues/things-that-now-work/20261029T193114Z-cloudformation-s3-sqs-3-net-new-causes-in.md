Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__sqs.deploy-cloudformation-3-include-malformed-snippet-fails cloudformation__s3__sqs.deploy-cloudformation-3-include-snippet-snapshotted-until-stack-update cloudformation__s3__sqs.deploy-cloudformation-3-include-without-auto-expand-rejected
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-malformed-snippet-fails|run#16|AssertionError|assert expected in reasons, (
Issue-Key: cloudformation__s3__sqs|UpdateStack|ValidationError
Issue-Key: cloudformation__s3__sqs.deploy-cloudformation-3-include-without-auto-expand-rejected|_error_code#15|AssertionError|raise AssertionError(f"expected this call to fail, but it succeeded and returned {ok!r}")
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# cloudformation__s3__sqs: 3 net-new causes in the AWS::Include transform path — wrong failure text, snippet never re-fetched on update, and CAPABILITY_AUTO_EXPAND not required

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes, all in the CloudFormation AWS::Include macro path.

Cause 1 of 3: the stack-event reason recorded when AWS::Include cannot parse the referenced S3 object is emulator-authored.
AWS: recorded behavior only (AWS::Include is a transform, not a modelled shape). CloudFormation records the transform failure as a stack-level status reason whose fixed text is "Transform AWS::Include failed with: The specified S3 object's content should be valid Yaml/JSON". Nothing in that sentence varies.
Emulator: the stack's reasons are instead `Transform AWS::Include failed with: S3 key [<key>] does not hold a JSON or YAML template snippet.` — a different sentence that additionally interpolates the request's S3 key.
Why: operators and CI gates match on CloudFormation's documented transform-failure text to classify a bad snippet; a rewritten sentence makes that classification fail and an unrelated transform error indistinguishable from this one.
Fix: the CloudFormation AWS::Include transform handler's error path — emit AWS's fixed reason text.
Traces: cloudformation__s3__sqs.deploy-cloudformation-3-include-malformed-snippet-fails

Cause 2 of 3: the Include snippet is snapshotted and never re-read, so an UpdateStack after the S3 object changed is refused as a no-op.
AWS: recorded behavior only. The snippet is expanded at each operation, so changing the referenced S3 object and calling UpdateStack with the same template body yields a real change set and the resource is updated.
Emulator: UpdateStack answers HTTP 400 `ValidationError` with message "No updates are to be performed." — the emulator compares only the outer template and never re-expands AWS::Include.
Why: the whole point of AWS::Include is sharing a snippet that can be revised independently; a client that revises the snippet and re-deploys is told there is nothing to do and silently keeps the stale configuration.
Fix: the CloudFormation UpdateStack change-computation path — expand transforms (AWS::Include) before diffing, so the expanded template is what is compared.
Traces: cloudformation__s3__sqs.deploy-cloudformation-3-include-snippet-snapshotted-until-stack-update

Cause 3 of 3: CreateStack with a template that uses AWS::Include is accepted without CAPABILITY_AUTO_EXPAND.
AWS: a template containing a macro/transform requires the caller to acknowledge CAPABILITY_AUTO_EXPAND; without it CreateStack is refused with HTTP 400 `InsufficientCapabilitiesException`, whose message names the required capability.
Emulator: the call returns 200 with a StackId and an OperationId, so the capability gate does not exist.
Why: the capability acknowledgement is a security control (a macro can rewrite the template arbitrarily); accepting the stack without it means a pipeline that must fail closed proceeds.
Fix: the CloudFormation CreateStack/UpdateStack capability validation, before the transform is expanded.
Traces: cloudformation__s3__sqs.deploy-cloudformation-3-include-without-auto-expand-rejected

## Evidence
Cause 1 — expected the reason "Transform AWS::Include failed with: The specified S3 object's content should be valid Yaml/JSON"; got `['User Initiated', 'Transform AWS::Include failed with: S3 key [snippets/queue-properties.yaml] does not hold a JSON or YAML template snippet.', ... ]`.
Cause 2 — `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the UpdateStack operation: No updates are to be performed.` after the referenced snippet object was replaced in S3.
Cause 3 — `AssertionError: expected this call to fail, but it succeeded and returned {'StackId': 'arn:aws:cloudformation:...', 'OperationId': '...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.

## Steps to Reproduce
1. Put a snippet object in S3 and CreateStack with a template whose Resources use `Fn::Transform: {Name: AWS::Include, Parameters: {Location: s3://.../snippet}}`.
2. Omit Capabilities=['CAPABILITY_AUTO_EXPAND'] — the create is accepted (cause 3).
3. Replace the snippet object with non-YAML/JSON bytes and re-create — the recorded stack reason is the emulator's sentence, not AWS's (cause 1).
4. Replace the snippet object with a different valid snippet and call UpdateStack with the unchanged outer template — ValidationError "No updates are to be performed." (cause 2).
