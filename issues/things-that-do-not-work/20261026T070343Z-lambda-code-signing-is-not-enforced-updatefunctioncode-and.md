Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.update-function-code-validation-errors lambda.update-function-configuration-signature-errors lambda.update-function-code-invalid-signature-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.update-function-code-validation-errors|_error#13|AssertionError|raise AssertionError(
Issue-Key: lambda.update-function-configuration-signature-errors|_error#14|AssertionError|raise AssertionError(
Issue-Key: lambda|GetObject|NoSuchKey
Issue-Commit: 6de2108b200ed90d33fb4f24d0237ce550e05e72

# Lambda code-signing is not enforced: UpdateFunctionCode and UpdateFunctionConfiguration accept unsigned artifacts under an Enforce CodeSigningConfig, and Signer reports Succeeded without writing the signed object

> _Note: this issue was identified against the code base at commit `6de2108b200ed90d33fb4f24d0237ce550e05e72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionCode

## Description
Three net-new root causes, all in the code-signing path. A fixer must close all three before submitting.

=== Cause 1 of 3: UpdateFunctionCode ignores the function's Enforce CodeSigningConfig ===
Failing check: lambda.update-function-code-validation-errors
AWS: UpdateFunctionCode models CodeVerificationFailedException as a client error with httpError 400 (Smithy: UpdateFunctionCode errors include CodeVerificationFailedException 400, InvalidCodeSignatureException 400, CodeSigningConfigNotFoundException 404). When a function has a CodeSigningConfig attached whose CodeSigningPolicies.UntrustedArtifactOnDeployment is `Enforce`, deploying a package that is not signed by one of the config's AllowedPublishers signing-profile versions is REFUSED with CodeVerificationFailedException and the function's code is left untouched. AWS's message for this error class is the fixed text "Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for " followed by the ARN of the artifact that failed verification (here the function ARN, taken from the request's target) and a trailing ".".
Emulator: the same UpdateFunctionCode SUCCEEDS. The replay's refusal helper reports `AssertionError: the call was expected to be refused but it SUCCEEDED — unsigned code was accepted under an Enforce code-signing policy`. The preconditions all held in the same run: CreateCodeSigningConfig with UntrustedArtifactOnDeployment=Enforce was accepted, PutFunctionCodeSigningConfig attached it, and GetFunctionCodeSigningConfig read the same CodeSigningConfigArn back — so the config exists and is attached, and UpdateFunctionCode simply never consults it. No 400, no error code, no message; the unsigned bytes are stored and become the live code.
Why: a client that relies on code signing to stop unreviewed code reaching production gets the opposite of the guarantee it configured — the emulator reports the control as attached and then ignores it, so a deployment pipeline tested against the emulator believes an Enforce policy is protecting it when nothing is.
Fix: the UpdateFunctionCode handler's validation step in the lambda service — before storing the package it must resolve the function's attached CodeSigningConfig, and when UntrustedArtifactOnDeployment is Enforce and the package is not signed by an allowed publisher, return CodeVerificationFailedException (HTTP 400) with the message template above and leave CodeSha256 unchanged.

=== Cause 2 of 3: UpdateFunctionConfiguration ignores the Enforce CodeSigningConfig when attaching Layers ===
Failing check: lambda.update-function-configuration-signature-errors
AWS: UpdateFunctionConfiguration also models CodeVerificationFailedException as a client error with httpError 400 (Smithy: UpdateFunctionConfiguration errors include CodeVerificationFailedException 400 and InvalidCodeSignatureException 400), because its `Layers` member attaches code. Attaching an unsigned layer version to a function whose CodeSigningConfig enforces verification is refused with CodeVerificationFailedException, the layer is NOT attached, and the message is the same fixed text as Cause 1 — "Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for " — but with the LAYER VERSION ARN as the variable part (the artifact that failed verification), taken from the request's Layers entry. The identical attach against a function whose policy is `Warn` is accepted; that is the control that isolates the policy as the only difference.
Emulator: the Enforce-policy UpdateFunctionConfiguration SUCCEEDS and attaches the unsigned layer. Replay: `AssertionError: the call was expected to be refused but it SUCCEEDED — an unsigned layer was accepted under an Enforce code-signing policy`.
Why: layers are a code-delivery path, so an unenforced one is a hole straight through the same control — a client can introduce arbitrary unsigned code into an Enforce-protected function via UpdateFunctionConfiguration even if Cause 1 is fixed. The two must be fixed together or the guarantee is still absent.
Fix: the UpdateFunctionConfiguration handler's validation step in the lambda service — apply the attached CodeSigningConfig to every layer version named in Layers, refuse with CodeVerificationFailedException (HTTP 400, message naming the layer version ARN) under Enforce, and leave the function's Layers list unmodified on refusal; keep accepting under Warn.

=== Cause 3 of 3: Signer reports a signing job Succeeded but never writes the signed object to the destination ===
Failing check: lambda.update-function-code-invalid-signature-rejected
AWS: StartSigningJob takes a `source` (S3 bucket/key/version) and a `destination` (S3 bucket/prefix) and returns a jobId (Smithy: StartSigningJob, input members source/destination/profileName/clientRequestToken all required, output jobId). DescribeSigningJob then reports `status` and, on success, a `signedObject` whose s3.key names an object that EXISTS in the destination bucket — that object is the signed deployment package, and fetching it with s3:GetObject is how a caller obtains signed bytes to deploy. A job whose artifact was not produced reports status Failed with a statusReason, never Succeeded.
Emulator: DescribeSigningJob reports status Succeeded and hands back signedObject.s3.key, but s3:GetObject on that exact bucket/key is refused: `botocore.errorfactory.NoSuchKey: An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist.` So the emulator mints a job result that points at an object it never wrote. Hypothesis (neither modelled nor recorded): StartSigningJob is implemented as a bookkeeping-only stub that records the job and marks it Succeeded without performing the copy/sign into the destination prefix.
Why: every flow that needs genuinely signed bytes is unreachable, and it fails in the most misleading way possible — the caller is told the job succeeded and is then given a dangling key, so the error surfaces as an S3 NoSuchKey far from its cause. In particular no client can exercise the signed-but-wrong-publisher path (Causes 1 and 2's positive and negative legs both need real signed packages), so code-signing cannot be validated end to end even once Causes 1 and 2 are fixed.
Fix: the StartSigningJob handler and the signing-job state in the signer service — the job must write the signed artifact under `destination.s3.prefix` in `destination.s3.bucketName` before it is allowed to reach status Succeeded, and DescribeSigningJob's signedObject.s3 must name that written object; if the artifact cannot be produced the job must report status Failed with statusReason rather than Succeeded.

Traces: lambda.update-function-code-validation-errors (Cause 1), lambda.update-function-configuration-signature-errors (Cause 2), lambda.update-function-code-invalid-signature-rejected (Cause 3).

## Evidence
Cause 1 of 3 — lambda.update-function-code-validation-errors
Expected (real AWS): with a CodeSigningConfig (UntrustedArtifactOnDeployment=Enforce) attached and confirmed by GetFunctionCodeSigningConfig, `UpdateFunctionCode(FunctionName=<fn>, ZipFile=<unsigned zip>)` is refused with HTTP 400, error code `CodeVerificationFailedException`, message = "Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <function ARN>." (fixed text as quoted; the variable part is the function ARN from the request). GetFunctionConfiguration afterwards still reports the previous CodeSha256.
Actual (emulator): the call returns success — no error code and no message at all. Replay detail: `check_lambda_update_function_code_validation_errors.py, line 214 ... AssertionError: the call was expected to be refused but it SUCCEEDED — unsigned code was accepted under an Enforce code-signing policy`. The positive control in the same run (an ordinary UpdateFunctionCode with no config attached) succeeded and CodeSha256 matched base64(sha256(zip)), so the only thing not working is the enforcement.

Cause 2 of 3 — lambda.update-function-configuration-signature-errors
Expected (real AWS): with an Enforce CodeSigningConfig attached, `UpdateFunctionConfiguration(FunctionName=<fn>, Layers=[<unsigned layer version ARN>])` is refused with HTTP 400, error code `CodeVerificationFailedException`, message = "Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <layer version ARN>." (fixed text as quoted; the variable part is the layer version ARN from the request's Layers entry), and GetFunctionConfiguration shows no Layers attached. The same attach against a Warn-policy function succeeds and the layer appears in Layers.
Actual (emulator): the Enforce-policy call returns success and attaches the layer. Replay detail: `check_lambda_update_function_configuration_signature_errors.py, line 214 ... AssertionError: the call was expected to be refused but it SUCCEEDED — an unsigned layer was accepted under an Enforce code-signing policy`.

Cause 3 of 3 — lambda.update-function-code-invalid-signature-rejected
Expected (real AWS): after `StartSigningJob(source={s3:{bucketName,key,version}}, destination={s3:{bucketName,prefix}}, profileName=<profile>)`, polling DescribeSigningJob until status == 'Succeeded' yields signedObject.s3.key, and `s3:GetObject(Bucket=<destination bucket>, Key=<that key>)` returns the signed package bytes (HTTP 200).
Actual (emulator): DescribeSigningJob reports status 'Succeeded' and returns a signedObject.s3.key, but GetObject on it is refused. Replay detail: `botocore.errorfactory.NoSuchKey: An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist.` — a Succeeded job pointing at an object that was never written.

## Steps to Reproduce
Cause 1 — UpdateFunctionCode does not enforce the policy:
1. Create an execution role and a python3.12 function; wait for State=Active.
2. UpdateFunctionCode with a fresh unsigned zip and confirm CodeSha256 == base64(sha256(zip)) (positive control, no config attached).
3. CreateCodeSigningConfig(AllowedPublishers={SigningProfileVersionArns:[<a signer profile version ARN>]}, CodeSigningPolicies={UntrustedArtifactOnDeployment:'Enforce'}).
4. PutFunctionCodeSigningConfig(CodeSigningConfigArn=<csc>, FunctionName=<fn>) and verify with GetFunctionCodeSigningConfig.
5. UpdateFunctionCode(FunctionName=<fn>, ZipFile=<another unsigned zip>) -> observe HTTP 200/success instead of HTTP 400 CodeVerificationFailedException, and GetFunctionConfiguration showing the new CodeSha256.

Cause 2 — UpdateFunctionConfiguration does not enforce the policy for Layers:
1. signer:PutSigningProfile(profileName=<name>, platformId='AWSLambda-SHA384-ECDSA') and keep profileVersionArn.
2. CreateCodeSigningConfig twice with the same AllowedPublishers — one with UntrustedArtifactOnDeployment='Enforce', one with 'Warn'.
3. Create two identical python3.12 functions; attach the Enforce config to one and the Warn config to the other with PutFunctionCodeSigningConfig.
4. PublishLayerVersion with an unsigned zip containing python/<module>.py.
5. UpdateFunctionConfiguration(FunctionName=<enforce fn>, Layers=[<layer version ARN>]) -> observe success instead of HTTP 400 CodeVerificationFailedException, and GetFunctionConfiguration reporting the layer attached. (The same call against the Warn function is expected to succeed.)

Cause 3 — Signer reports Succeeded without writing the signed object:
1. Create a versioned S3 bucket.
2. signer:PutSigningProfile(profileName=<name>, platformId='AWSLambda-SHA384-ECDSA').
3. PutObject an unsigned zip at unsigned/<label>.zip and keep its VersionId.
4. signer:StartSigningJob(source={'s3':{'bucketName':<bucket>,'key':'unsigned/<label>.zip','version':<VersionId>}}, destination={'s3':{'bucketName':<bucket>,'prefix':'signed/<label>/'}}, profileName=<name>).
5. Poll signer:DescribeSigningJob(jobId=...) until status == 'Succeeded' and read signedObject.s3.key.
6. s3:GetObject(Bucket=<bucket>, Key=<that key>) -> observe NoSuchKey; also ListObjectsV2 under prefix 'signed/<label>/' and observe the prefix is empty.
