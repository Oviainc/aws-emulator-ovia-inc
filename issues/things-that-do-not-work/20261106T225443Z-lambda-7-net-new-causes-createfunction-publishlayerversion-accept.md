Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted lambda.create-function-code-validation-errors lambda.blueprint-tag-tag-resource-oversize-value-rejected lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create lambda.put-resource-policy-length-exceeded lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.create-function-code-validation-errors|_error_code#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: lambda.blueprint-tag-tag-resource-oversize-value-rejected|run#7|AssertionError|assert msg == expected, f"expected the exact ValidationException message, got {msg[-200:]!r}"
Issue-Key: lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create|run#10|AssertionError|assert msg == (
Issue-Key: lambda.put-resource-policy-length-exceeded|run#15|AssertionError|assert error_msg == (
Issue-Key: lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected|run#10|AssertionError|assert code_msg == expected_msg, (
Issue-Commit: 2d5907cd7e831b17c8c4fd60b3b325267ed3fe4d

# lambda: 7 net-new causes — CreateFunction/PublishLayerVersion accept invalid code packages, and four refusal messages are re-worded

> _Note: this issue was identified against the code base at commit `2d5907cd7e831b17c8c4fd60b3b325267ed3fe4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Seven net-new root causes on the Lambda control plane, all grounded in failing behavioral checks that are not covered by any open issue. Each cause is numbered and carries its own failing check ids.

Cause 1 of 7: CreateFunction accepts a code package that is not a usable zip.
AWS: CreateFunction rejects a Code.ZipFile that is not a zip archive with HTTP 400 InvalidParameterValueException and the fixed message "Could not unzip uploaded file. Please check your file, then try to upload again.", and rejects a syntactically valid zip with no entries with HTTP 400 InvalidParameterValueException "Uploaded file must be a non-empty zip" (both messages are fixed text, no variable parts). The refusal creates nothing: a follow-up GetFunction answers 404 ResourceNotFoundException with "Function not found: <function ARN>" (variable part: the ARN built from region/account/name in the request). Smithy: CreateFunction declares InvalidParameterValueException (httpError 400) and ResourceNotFoundException (404).
Emulator: both requests are accepted with HTTP 201 and a full FunctionConfiguration body (State "Pending", StateReasonCode "Creating", a CodeSha256 computed over the junk bytes), and the function then exists.
Why: a client that relies on Lambda to validate its artifact gets a function that can never run instead of a 400 at deploy time; negative-path code that expects InvalidParameterValueException sees success, and rollback/cleanup logic keyed on "nothing was created" leaks a function.
Fix: the zip-package validation in the CreateFunction handler of the lambda service crate (the code-package decode step shared by Code.ZipFile ingestion), plus the guarantee that a refused create commits no state.
Traces: lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing.

Cause 2 of 7: PublishLayerVersion accepts non-zip bytes and mints a layer version.
AWS: PublishLayerVersion with Content.ZipFile that is not a zip is refused with HTTP 400 InvalidParameterValueException "Could not unzip uploaded file. Please check your file, then try to upload again." (fixed text) and no version is minted — ListLayerVersions stays empty and the next valid publish is version 1.
Emulator: answers HTTP 201 with a Content envelope (Location/CodeSha256/CodeSize) and mints version 1 from the junk bytes.
Why: a layer that cannot be unpacked is published, so the failure surfaces later as a broken function instead of at publish time, and version numbering is consumed by an artifact AWS would have rejected.
Fix: the zip-package validation in the PublishLayerVersion handler of the lambda service crate, and no version minting on refusal.
Traces: lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted.

Cause 3 of 7: CreateFunction ignores a code-signing configuration whose UntrustedArtifactOnDeployment is Enforce.
AWS: creating a function with CodeSigningConfigArn pointing at an Enforce configuration and an unsigned zip is refused with CodeVerificationFailedException, and the function is not created.
Emulator: the create succeeds (HTTP 201) and returns a normal FunctionConfiguration, so the check's `_error_code` helper reports "expected the call to be rejected, but it SUCCEEDED".
Why: code-signing enforcement is a security control; a client that deploys through an Enforce configuration believes unsigned artifacts cannot reach the account, while the emulator admits them.
Note: the open issue 20261026T070343Z-lambda-code-signing-is-not-enforced-updatefunctioncode-and.md covers the UpdateFunctionCode / UpdateFunctionConfiguration signature paths; it does not carry this CreateFunction check, which is why this cause is filed here.
Fix: code-signing verification in the CreateFunction handler of the lambda service crate (the same verification the update paths need).
Traces: lambda.create-function-code-validation-errors.

Cause 4 of 7: TagResource's over-long tag value is refused with the wrong ValidationException message shape.
AWS: a tag value longer than 256 characters is refused with ValidationException whose message renders the whole Tags map once and lists the full constraint set: "1 validation error detected: Value '{<key>=<value>}' at 'tags' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*), Member must not be null]". Fixed text: everything outside the quoted value; variable parts: the `<key>=<value>` rendering taken from the request's Tags map. Smithy: TagResource.Tags is a required map, Resource is length 1..10000.
Emulator: returns "1 validation error detected: Value '<value>' at 'tags.<key>' failed to satisfy constraint: Member must have length less than or equal to 256" — the member path is per-key (`tags.<key>`) instead of `tags`, the value is quoted bare instead of as a `{key=value}` map, and the three other constraints are dropped.
Why: clients and test suites that match on the documented validation text (or parse the member path to attribute the error) misclassify the failure.
Fix: the tag validation/serialization in the TagResource handler of the lambda service crate.
Traces: lambda.blueprint-tag-tag-resource-oversize-value-rejected.

Cause 5 of 7: the environment-variable 4KB refusal does not report the measured size.
AWS: when the serialized environment exceeds 4096 bytes, CreateFunction is refused with InvalidParameterValueException and the message "Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit. Measured size: <n> bytes" — fixed text plus one variable part, the measured byte count (4097 in the one-byte-over case).
Emulator: returns the same first sentence but then "String measured: {<the entire environment JSON>}", echoing the whole payload instead of its measured size.
Why: a client that reads the measured size to decide how much to trim (or logs the refusal) gets a multi-kilobyte echo of its own variables, including values that may be sensitive, and no number.
Fix: the environment-size validation message in the CreateFunction/UpdateFunctionConfiguration validation path of the lambda service crate.
Traces: lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create.

Cause 6 of 7: PutResourcePolicy's length refusal truncates the offending value.
AWS: a Policy longer than the modeled maximum is refused with ValidationException "1 validation error detected: Value '<policy>' at 'policy' failed to satisfy constraint: Member must have length less than or equal to 20480", where `<policy>` is the request's policy document reproduced in full and 20480 is the modeled bound. Smithy: PutResourcePolicy.Policy is length 1..20480 and the operation declares ValidationException-class refusals alongside PolicyLengthExceededException (400).
Emulator: returns the same template but replaces the value with a prefix plus a synthesized suffix "... [91147 bytes]", so the message does not reproduce the value AWS quotes.
Why: callers that diff the refusal text against AWS, or extract the rejected document from it, see a message AWS never emits.
Fix: the validation-message rendering for the policy member in the PutResourcePolicy handler of the lambda service crate (value truncation must not be introduced).
Traces: lambda.put-resource-policy-length-exceeded.

Cause 7 of 7: ListVersionsByFunction rejects a malformed Marker with the wrong message.
AWS: an unparseable Marker is refused with InvalidParameterValueException "The marker value isn't valid: <marker>. Specify a valid marker value and try again." — fixed text with the request's marker as the only variable part. Smithy: ListVersionsByFunction is paginated on Marker/NextMarker and declares InvalidParameterValueException (httpError 400).
Emulator: returns a generic constraint-violation message instead: "1 validation error detected: Value '<marker>' at 'marker' failed to satisfy constraint: Member must be the function version ARN of a previous page's last entry", asserting a marker format (a version ARN) that is not AWS's contract.
Why: pagination clients that retry or reset on AWS's documented marker refusal do not recognize the emulator's text, and the invented "must be the function version ARN" rule teaches callers a wrong token format.
Fix: the Marker validation and its message in the ListVersionsByFunction handler of the lambda service crate.
Traces: lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected.

## Evidence
All seven causes are replayed failures of the behavioral suite against the emulator (service heading `### lambda`), none marked ALREADY FILED or OWNED.

1) create-function-non-zip-and-empty-zip-rejected-creating-nothing — expected ('InvalidParameterValueException', 'Could not unzip uploaded file. Please check your file, then try to upload again.') / ('InvalidParameterValueException', 'Uploaded file must be a non-empty zip'); actual: AssertionError "expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201 ...}}" with a complete FunctionConfiguration.
2) publish-layer-version-corrupt-zip-rejected-no-version-minted — expected ('InvalidParameterValueException', 'Could not unzip uploaded file. Please check your file, then try to upload again.'); actual HTTP 201 with Content.Location/CodeSha256 for the non-zip bytes.
3) create-function-code-validation-errors — expected code CodeVerificationFailedException for unsigned code under an Enforce code-signing config; actual HTTP 201 and a created function (AssertionError "expected the call to be rejected, but it SUCCEEDED").
4) blueprint-tag-tag-resource-oversize-value-rejected — expected "1 validation error detected: Value '{<key>=<value>}' at 'tags' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ([\p{L}\p{Z}\p{N}_.:/=+\-@]*), Member must not be null]"; actual "...' at 'tags.<key>' failed to satisfy constraint: Member must have length less than or equal to 256".
5) env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create — expected "...exceeded the 4KB limit. Measured size: 4097 bytes"; actual "...exceeded the 4KB limit. String measured: {\"SIM2REAL_A\":\"aaaa...\"" (the whole environment echoed).
6) put-resource-policy-length-exceeded — expected the full policy quoted in "1 validation error detected: Value '<policy>' at 'policy' failed to satisfy constraint: Member must have length less than or equal to 20480"; actual the same template with the value replaced by a prefix plus "... [91147 bytes]".
7) list-function-versions-by-capacity-provider-invalid-parameter-rejected — expected "The marker value isn't valid: <marker>. Specify a valid marker value and try again."; actual "1 validation error detected: Value '<marker>' at 'marker' failed to satisfy constraint: Member must be the function version ARN of a previous page's last entry".

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, then re-run only these checks under the `lambda` service heading:
- lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing
- lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted
- lambda.create-function-code-validation-errors
- lambda.blueprint-tag-tag-resource-oversize-value-rejected
- lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create
- lambda.put-resource-policy-length-exceeded
- lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected

By hand, against the emulator endpoint: (a) CreateFunction with Code.ZipFile set to arbitrary non-zip bytes, then with a zip containing no entries — both answer 201 instead of 400 InvalidParameterValueException, and GetFunction then finds the function; (b) PublishLayerVersion with the same non-zip bytes — answers 201 and ListLayerVersions reports version 1; (c) CreateFunction with CodeSigningConfigArn of an Enforce config and an unsigned zip — answers 201; (d) TagResource on a function ARN with a 257-character tag value; (e) CreateFunction with an environment serializing to 4097 bytes; (f) PutResourcePolicy with a >20480-byte policy; (g) ListVersionsByFunction with Marker="not-a-real-pagination-token!!" — compare each refusal's code and message with the AWS text quoted above.
