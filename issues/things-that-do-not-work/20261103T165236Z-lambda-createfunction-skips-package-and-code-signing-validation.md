Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-code-signing-warn-then-enforce
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-code-signing-warn-then-enforce lambda-create-function-ephemeral-storage-above-maximum-rejected lambda-create-function-imageconfig-on-zip-package-rejected lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling lambda-container-image-handler-and-runtime-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|CreateFunction|4xx->2xx|status
Issue-Key: lambda|CreateFunction|4xx->2xx|$.message
Issue-Key: lambda|CreateFunction|4xx->4xx|$.message
Issue-Related: 20261029T233239Z-lambda-code-signing-is-never-enforced-and-code.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda CreateFunction skips package and code-signing validation

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
CreateFunction's input validation is incomplete: three request shapes AWS refuses with 400 are accepted and the function is created (HTTP 201), and a fourth refusal uses emulator wording instead of AWS's. 4 causes, all in the CreateFunction request-validation stage.

Cause 1 of 4 - a code-signing config in Enforce mode is not applied, so an unsigned/mis-signed package is deployed.
AWS: Smithy: CreateFunction declares InvalidCodeSignatureException (400) and CodeVerificationFailedException (400). With a code-signing configuration whose UntrustedArtifactOnDeployment is Enforce, AWS refuses the deploy with HTTP 400, Type 'User' and the message 'Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <function-name-or-arn>.' - fixed text with the function name or ARN from the request substituted at the end.
Emulator: creates the function and answers HTTP 201 with the function configuration; no Type, no Message, no x-amzn-errortype header. The same enforcement gap is observable on UpdateFunctionCode in the same fixture.
Why: a client relying on code signing to block unsigned artifacts gets them deployed silently, so the control it configured never fires.
Fix: the CreateFunction handler's code-signing-config enforcement step (before state is written).
Traces: lam-err-code-signing-warn-then-enforce#task#7 (CreateFunction, expected 400, got 201). The open issue 20261029T233239Z-lambda-code-signing-is-never-enforced-and-code.md carries the UpdateFunctionCode and teardown replay keys for this fixture but not this CreateFunction step.

Cause 2 of 4 - EphemeralStorage.Size above the account maximum is accepted.
AWS: Smithy: EphemeralStorage.Size is required with range min 512; its documentation states the value 'can be any whole number between 512 and 10,240 MB'. A larger value is refused with HTTP 400 and the message "'EphemeralStorage.Size' value failed to satisfy constraint: Member must have value less than or equal to 10240" - fixed text, with the member path and the limit 10240 as its variable parts.
Emulator: creates the function and answers HTTP 201; no message, no x-amzn-errortype.
Why: a client that probes the ceiling to size /tmp gets a function AWS would never create, and the oversize setting is reported back as if it were in effect.
Fix: the CreateFunction handler's validation of EphemeralStorage.
Traces: lambda-create-function-ephemeral-storage-above-maximum-rejected#task#4.

Cause 3 of 4 - ImageConfig on a Zip deployment package is accepted.
AWS: Smithy: CreateFunction input carries both PackageType and ImageConfig and declares InvalidParameterValueException (400). ImageConfig is only meaningful for PackageType=Image; supplying it with a Zip package is refused with HTTP 400, Type 'User' and the message 'The ImageConfig parameter is not supported for functions created with deployment packages.' (fixed text, no variable parts).
Emulator: creates the function and answers HTTP 201.
Why: a client that mistakenly carries ImageConfig over from a container deployment gets a function whose entrypoint/command overrides are silently ignored, instead of the refusal that would tell it the package type is wrong.
Fix: the CreateFunction handler's package-type/ImageConfig cross-field validation.
Traces: lambda-create-function-imageconfig-on-zip-package-rejected#task#4.

Cause 4 of 4 - the Handler-on-Image refusal uses emulator wording.
AWS: same InvalidParameterValueException (400); for Handler supplied with PackageType=Image the message is 'The Handler parameter is not supported for functions created with container images.' (fixed text, no variable parts).
Emulator: refuses with the correct 400 but the message 'Handler cannot be set for a function whose PackageType is Image'.
Fix: the CreateFunction validation's message text for this refusal.
Traces: lambda-container-image-handler-and-runtime-rejected#task#12. A related open issue, 20261103T031416Z-lambda-10-net-new-root-causes-container-image.md, covers other container-image validation gaps but does not carry this cluster's replay key.

## Evidence
Cause 1: lam-err-code-signing-warn-then-enforce#task#7 - expected status 400 with $.Type='User' and $.Message='Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <name>.'; got status 201 with no Type/Message and no x-amzn-errortype header.
Cause 2: lambda-create-function-ephemeral-storage-above-maximum-rejected#task#4 - expected 400 with $.message="'EphemeralStorage.Size' value failed to satisfy constraint: Member must have value less than or equal to 10240"; got 201.
Cause 3: lambda-create-function-imageconfig-on-zip-package-rejected#task#4 - expected 400 with $.Type='User' and $.message='The ImageConfig parameter is not supported for functions created with deployment packages.'; got 201. 2 steps affected.
Cause 4: lambda-container-image-handler-and-runtime-rejected#task#12 - 400 vs 400, $.message expected 'The Handler parameter is not supported for functions created with container images.', got 'Handler cannot be set for a function whose PackageType is Image'. 5 steps affected.

## Steps to Reproduce
1. Replay lam-err-code-signing-warn-then-enforce and inspect task#7 (CreateFunction with a code-signing config in Enforce mode and an untrusted artifact).
2. Replay lambda-create-function-ephemeral-storage-above-maximum-rejected task#4 (CreateFunction with EphemeralStorage.Size above 10240).
3. Replay lambda-create-function-imageconfig-on-zip-package-rejected task#4 (CreateFunction with PackageType=Zip plus ImageConfig).
4. Replay lambda-container-image-handler-and-runtime-rejected task#12 (CreateFunction with PackageType=Image plus Handler) and compare the refusal message.
