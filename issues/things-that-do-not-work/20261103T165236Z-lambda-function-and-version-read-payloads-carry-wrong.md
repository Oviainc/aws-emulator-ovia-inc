Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-batch-diag-submit-wrapper-function-handle-mistakes
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-batch-diag-submit-wrapper-function-handle-mistakes lambda-poller-handlers-invoked-directly lambda-cfnuf-alias-repair-attempts-in-lambda lambda-lsrno-traj-esm-role-gap-then-lookup-by-function-and-queue lambda-sstr-traj-handler-module-mismatch-then-fix lambda-poller-function-reference-forms lambda-wsbe-traj-integration-uri-as-function-name lambda-container-image-handler-and-runtime-rejected lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|GetFunction|2xx->2xx|$.Configuration.RuntimeVersionConfig.RuntimeVersionArn
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->2xx|$.RuntimeVersionConfig.RuntimeVersionArn
Issue-Key: lambda|GetFunction|2xx->2xx|$.Code.Location
Issue-Key: lambda|GetFunction|2xx->2xx|$.Configuration.FunctionArn
Issue-Key: lambda|ListVersionsByFunction|2xx->2xx|$.Versions[*].Description
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.CodeSha256
Issue-Key: lambda|CreateFunction|2xx->2xx|$.Layers[*].UncompressedCodeSize
Issue-Related: 20261029T233239Z-lambda-function-configuration-and-version-listings-carry-the.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda function and version read payloads carry wrong identity and code metadata

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
The emulator's FunctionConfiguration / FunctionCodeLocation projection is built from internal defaults rather than from AWS's modelled members, so every read path that returns a function or a version returns wrong identity and wrong code metadata. 8 causes, all in the shared serializer/projection for function reads.

Cause 1 of 8 - GetFunction RuntimeVersionConfig.RuntimeVersionArn is a short non-AWS identifier.
AWS: Smithy: GetFunction output FunctionConfiguration.RuntimeVersionConfig -> RuntimeVersionConfig.RuntimeVersionArn. AWS's runtime-version ARN ends in a 64-hex-character SHA-256 runtime digest, i.e. arn:aws:lambda:<region>::runtime:<64 hex chars>; the recording shows the 64-hex form.
Emulator: returns arn:aws:lambda:us-west-2::runtime:<16 hex chars> - a 16-character identifier in the ARN resource position.
Why: a client that parses or pins the runtime-version digest (runtime-management configuration, drift detection) cannot match the value against any real runtime version, and the ARN fails any 64-hex validation.
Fix: the function-configuration serializer that materialises RuntimeVersionConfig.
Traces: lambda-batch-diag-submit-wrapper-function-handle-mistakes#task#7 (GetFunction, 200 vs 200).
Note: the open issue 20261102T111455Z-lambda-runtimeversionarn-carries-a-short-non-aws-identifier.md carries the GetFunctionConfiguration replay key only; the GetFunction and UpdateFunctionConfiguration clusters here are not covered by it.

Cause 2 of 8 - UpdateFunctionConfiguration returns the same short RuntimeVersionArn.
AWS: same modelled member on the UpdateFunctionConfiguration output (FunctionConfiguration); the 64-hex runtime digest form.
Emulator: the 16-hex short identifier again on the mutation response.
Why: a client that reads the configuration back from the mutation response, instead of re-reading it, sees the same unusable digest.
Fix: the same function-configuration serializer, reached from the UpdateFunctionConfiguration handler.
Traces: lambda-cfnuf-alias-repair-attempts-in-lambda#task#11.

Cause 3 of 8 - GetFunction Code.Location is not a presigned, time-limited S3 URL.
AWS: Smithy: GetFunction output FunctionCodeLocation.Location - AWS returns a presigned S3 GET URL for the deployment package that is valid for a short window, so the URL carries the SigV4 query parameters (X-Amz-Algorithm, X-Amz-Credential, X-Amz-Date, X-Amz-Expires, X-Amz-SignedHeaders, X-Amz-Signature, and a security token when the caller is role-based).
Emulator: returns a bare, unsigned object path of the form https://awslambda-<region>-tasks.s3.<region>.amazonaws.com/snapshots/<account>/<function>-<uuid> with no query string at all, which fails the recorded shape assertion on that member.
Why: a client that downloads the deployment package with a plain unauthenticated GET - the documented use of this field - gets an access-denied response rather than the archive.
Fix: the GetFunction handler's code-location builder (presign the object instead of formatting a path).
Traces: lambda-poller-function-reference-forms#task#6.

Cause 4 of 8 - the same unsigned Code.Location on a second function shape.
AWS: as cause 3.
Emulator: as cause 3, confirming the defect is in the shared builder and not in one fixture's function.
Fix: as cause 3.
Traces: lambda-wsbe-traj-integration-uri-as-function-name#task#6.

Cause 5 of 8 - GetFunction drops the version qualifier from Configuration.FunctionArn when the request is qualified.
AWS: Smithy: GetFunction output FunctionConfiguration.FunctionArn targets NameSpacedFunctionArn, whose pattern explicitly admits a trailing qualifier: ...:function:<name>(:($LATEST(.PUBLISHED)?|<alias-or-version>))?. When the request names a qualifier, AWS echoes the qualified ARN, so a $LATEST-qualified read returns ...:function:<name>:$LATEST.
Emulator: returns the unqualified ...:function:<name>, discarding the qualifier.
Why: a client that round-trips the returned ARN into a qualified call (AddPermission, an alias target, an event-source FunctionArn) silently loses the pin and operates on $LATEST-unqualified identity instead of the version it asked about.
Fix: the GetFunction handler's ARN construction, which must preserve the request qualifier.
Traces: lambda-poller-function-reference-forms#task#8.

Cause 6 of 8 - ListVersionsByFunction's $LATEST entry is unqualified and the entries carry fields AWS leaves unset.
AWS: Smithy: ListVersionsByFunction output Versions -> FunctionConfiguration. The $LATEST entry's FunctionArn is qualified with :$LATEST, and in the recording the per-version entries carry no State, no LastUpdateStatus and no RuntimeVersionConfig.
Emulator: emits the $LATEST entry with the unqualified ARN and adds State=Active, LastUpdateStatus=Successful and RuntimeVersionConfig to every entry; it also lists a version the fixture never published (see the separate DeleteFunction/alias issue) and returns an empty Description where the published version's description was set.
Why: a client that keys versions off FunctionArn cannot distinguish $LATEST from the unqualified function, and one that treats the presence of State/LastUpdateStatus as 'this entry is a live, separately-tracked version' misreads every historical version.
Fix: the ListVersionsByFunction handler's per-version projection.
Traces: lambda-cfnuf-alias-repair-attempts-in-lambda#task#16.

Cause 7 of 8 - GetFunctionConfiguration omits CodeSha256 for an Image-package function.
AWS: Smithy: FunctionConfiguration.CodeSha256 (String) - AWS always reports the code hash, and for PackageType=Image it is the resolved image digest-derived hash; the recording asserts the member is present and shaped.
Emulator: omits CodeSha256 entirely from the response for a container-image function.
Why: a client that compares CodeSha256 to detect whether the deployed code changed (the documented deployment-drift check) has no value to compare and concludes nothing is deployed.
Fix: the GetFunctionConfiguration handler's projection for the Image package type.
Traces: lambda-container-image-handler-and-runtime-rejected#task#16.

Cause 8 of 8 - CreateFunction's Layers entries omit the uncompressed layer size.
AWS: recorded behavior only - the Smithy Layer shape in this model carries Arn, CodeSize, SigningProfileVersionArn and SigningJobArn, but the recorded AWS response additionally reports the attached layer's unzipped size on each Layers entry (the recording's value is the probe layer's own unzipped byte count; the requirement is that the member is present and reports the attached layer archive's uncompressed size in bytes).
Emulator: omits that member from every Layers entry.
Why: a client that sums layer uncompressed sizes to check the 250 MB unzipped deployment-package ceiling before deploying has nothing to sum and cannot predict the limit refusal.
Fix: the CreateFunction handler's Layers projection (and the layer-state lookup that must carry the uncompressed size).
Traces: lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling#task#8.

## Evidence
Cause 1: GetFunction/GetFunctionConfiguration expected $.RuntimeVersionConfig.RuntimeVersionArn to end in a 64-hex runtime digest; emulator returned a 16-hex one (arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd). 22+ steps affected across fixtures.
Cause 2: same member, same divergence, on UpdateFunctionConfiguration 200 responses (4 steps).
Causes 3-4: $.Code.Location expected a presigned URL shape; emulator returned an unsigned https://awslambda-us-west-2-tasks.s3.us-west-2.amazonaws.com/snapshots/<account>/<name>-<uuid> path with no SigV4 query parameters.
Cause 5: $.Configuration.FunctionArn expected ...:function:<name>:$LATEST, got ...:function:<name>.
Cause 6: $.Versions[0].FunctionArn expected ...:$LATEST, got unqualified; $.Versions[*].State / .LastUpdateStatus / .RuntimeVersionConfig expected absent, got 'Active' / 'Successful' / {RuntimeVersionArn: ...}; $.Versions[1].Description expected 'cfnuf fix', got ''.
Cause 7: $.CodeSha256 expected present and shaped, got absent (status 200 vs 200).
Cause 8: $.Layers[0].UncompressedCodeSize expected present (recording reports the attached layer's unzipped byte count), got absent (status 201 vs 201).

## Steps to Reproduce
1. Replay fixture lambda-batch-diag-submit-wrapper-function-handle-mistakes and compare task#7's GetFunction body.
2. Replay lambda-cfnuf-alias-repair-attempts-in-lambda task#11 (UpdateFunctionConfiguration) and task#16 (ListVersionsByFunction).
3. Replay lambda-poller-function-reference-forms task#6 and task#8 (GetFunction by name and by $LATEST-qualified reference) and lambda-wsbe-traj-integration-uri-as-function-name task#6.
4. Replay lambda-container-image-handler-and-runtime-rejected task#16 (GetFunctionConfiguration on an Image-package function).
5. Replay lambda-x2x-xlsx-layer-attach-mistakes-and-size-ceiling task#8 (CreateFunction with a layer attached).
