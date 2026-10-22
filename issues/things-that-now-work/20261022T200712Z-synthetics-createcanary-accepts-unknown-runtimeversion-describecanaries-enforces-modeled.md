Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.create-canary-invalid-runtime-version-rejected synthetics.describe-canaries-max-results-out-of-range-rejected synthetics.tag-resource-invalid-arn-bad-request-exception
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.create-canary-invalid-runtime-version-rejected|_client_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: synthetics|DescribeCanaries|ValidationException
Issue-Key: synthetics.tag-resource-invalid-arn-bad-request-exception|run#5|AssertionError|assert error["Message"] == expected_message, f"expected message {expected_message!r}, got {error['Message']!r}"
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Synthetics: CreateCanary accepts unknown RuntimeVersion, DescribeCanaries enforces modeled MaxResults ceiling of 20 instead of AWS's 1000, TagResource echoes anchored regex in validation message (3 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCanary

## Description
Three distinct root causes in the bootstrapped synthetics emulator, all reached by operations that DO answer (no scaffold stub involved).

Cause 1 of 3: CreateCanary does not validate RuntimeVersion against the set of supported Synthetics runtimes.
AWS: CreateCanary takes RuntimeVersion as a required member; Smithy (services/service-com-amazonaws-synthetics-synthetics/smithy-model) models it only as String @length min 1 max 1024, with no enum, so the runtime allow-list is a server-side check: CreateCanary.errors = InternalServerException(500), RequestEntityTooLargeException(413), ValidationException(400). Recorded behavior: a RuntimeVersion that is not a published runtime is rejected with HTTP 400 / error code ValidationException, and NO canary is created (the subsequent DescribeCanaries(Names=[name]) must not list it). The message is built from the request and names the offending value, e.g. "Invalid runtime version specified: syn-nodejs-puppeteer-not-a-real-version" (example text; the rule is that it reports the rejected RuntimeVersion value).
Emulator: returns HTTP 200 with a full Canary body (Status.State=CREATING, StateReasonCode=CREATE_IN_PROGRESS) and echoes RuntimeVersion: 'syn-nodejs-puppeteer-not-a-real-version' back, persisting a canary plus a synthesized EngineArn/SourceLocationArn for a runtime that does not exist.
Why: a client that mistypes or pins a retired runtime gets a success and a canary id from the emulator, so code that handles the AWS ValidationException path (surfacing the bad runtime, falling back to DescribeRuntimeVersions) is never exercised, and the emulator's state contains canaries that real AWS would never have created. Negative-path tests pass against the emulator and fail against AWS.
Fix: CreateCanary's input validation / handler in the synthetics service crate — reject RuntimeVersion values outside the runtime catalogue the service reports from DescribeRuntimeVersions, with ValidationException (400), before any canary state is written.
Traces: synthetics.create-canary-invalid-runtime-version-rejected.

Cause 2 of 3: DescribeCanaries enforces the modeled MaxResults range (1-20) that real AWS does not enforce, so valid page sizes are rejected.
AWS: Smithy DescribeCanaries.MaxResults targets MaxCanaryResults with @range min 1 max 20, but the recorded behavior shows real AWS does NOT enforce that trait server-side: MaxResults=21 and MaxResults=1000 both return HTTP 200 with a Canaries member, and only values above the true ceiling of 1000 are rejected — MaxResults=1001 returns HTTP 400 / ValidationException with a message built from the request, naming the member and the real ceiling, e.g. "1 validation error detected: Value '1001' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000" (the rule being: member path 'maxResults', ceiling 1000).
Emulator: rejects MaxResults=1000 with HTTP 400 / ValidationException "1 validation error detected: Value '1000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20" — i.e. it applies the modeled @range ceiling of 20 literally, so every page size in 21..1000 that AWS accepts is refused.
Why: a client that pages DescribeCanaries with any size above 20 (a perfectly ordinary choice, and accepted by AWS) gets a hard ValidationException from the emulator and cannot enumerate canaries at all; conversely it never sees the real boundary at 1000. This is a false rejection, the worst class for a client under test.
Fix: DescribeCanaries' input validation in the synthetics service crate — do not apply the modeled MaxResults @range ceiling; accept 1..=1000 and raise ValidationException (400) only above 1000, reporting the member as 'maxResults' and the 1000 ceiling.
Traces: synthetics.describe-canaries-max-results-out-of-range-rejected.

Cause 3 of 3: TagResource's ValidationException message echoes the ResourceArn pattern WITH the regex anchors, where AWS echoes the unanchored pattern.
AWS: Smithy TagResource.ResourceArn is an httpLabel, required, @length 1..2048, @pattern "^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\\d{1}:\\d{12}:(canary|group):[0-9a-z_\\-]+$". A malformed ARN is rejected with HTTP 400 / error code ValidationException (note: ValidationException is not even in TagResource's modeled error set, which lists only BadRequestException/ConflictException/NotFoundException/TooManyRequestsException — recorded behavior governs here). The message is request-derived and therefore a requirement by rule: "1 validation error detected: Value '<value>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where the pattern is rendered WITHOUT the leading ^ and trailing $.
Emulator: returns the right status and code (ValidationException) and the right member path, but renders the pattern with the anchors included: "...regular expression pattern: ^arn:(aws[a-zA-Z-]*)?:synthetics:...[0-9a-z_\\-]+$" instead of "...regular expression pattern: arn:(aws[a-zA-Z-]*)?:synthetics:...[0-9a-z_\\-]+".
Why: lowest severity of the bundle — status and code are correct. It still diverges for clients and test suites that assert on the constraint text the service reports, and it indicates the validation-message formatter emits the raw modeled pattern rather than AWS's rendering, so every pattern-constrained member in this service will carry the same extra anchors.
Fix: the shared validation-error message formatter used by the synthetics handlers — strip the anchors when rendering a @pattern constraint into a ValidationException message.
Traces: synthetics.tag-resource-invalid-arn-bad-request-exception.

## Evidence
Cause 1 — synthetics.create-canary-invalid-runtime-version-rejected (mismatch):
  expected: ClientError, code ValidationException (HTTP 400), message e.g. "Invalid runtime version specified: syn-nodejs-puppeteer-not-a-real-version", and no canary left behind.
  actual:   HTTPStatusCode 200 with {'Canary': {'Id': ..., 'Status': {'State': 'CREATING', 'StateReasonCode': 'CREATE_IN_PROGRESS'}, 'RuntimeVersion': 'syn-nodejs-puppeteer-not-a-real-version', ...}}. Harness: "AssertionError: expected a ClientError, but the call SUCCEEDED and returned {...}".

Cause 2 — synthetics.describe-canaries-max-results-out-of-range-rejected (error):
  expected: DescribeCanaries(MaxResults=1000) -> 200 with Canaries; MaxResults=21 -> 200 with Canaries; MaxResults=1001 -> 400 ValidationException "... Value '1001' at 'maxResults' ... less than or equal to 1000".
  actual:   DescribeCanaries(MaxResults=1000) -> botocore.errorfactory.ValidationException: "An error occurred (ValidationException) when calling the DescribeCanaries operation: 1 validation error detected: Value '1000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20" (the positive control never got past its first call).

Cause 3 — synthetics.tag-resource-invalid-arn-bad-request-exception (mismatch):
  expected message: "1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\\d{1}:\\d{12}:(canary|group):[0-9a-z_\\-]+"
  actual message:   "1 validation error detected: Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\\d{1}:\\d{12}:(canary|group):[0-9a-z_\\-]+$"
  Error code matched (ValidationException); only the rendered pattern differs (anchors).

## Steps to Reproduce
Boot the emulator and point a botocore synthetics client at it (region us-west-2 in the replay).

Cause 1:
  1. Create an S3 artifact bucket and an IAM execution role.
  2. synthetics.create_canary(Name=<new-name>, Code={'ZipFile': <minimal zip>, 'Handler': 'canary.handler'}, ArtifactS3Location='s3://<bucket>', ExecutionRoleArn=<role arn>, Schedule={'Expression': 'rate(1 hour)'}, RuntimeVersion='syn-nodejs-puppeteer-not-a-real-version').
  3. Observe HTTP 200 and a created Canary; AWS returns 400 ValidationException and creates nothing. Confirm with synthetics.describe_canaries(Names=[<new-name>]) — the emulator lists the canary.

Cause 2:
  1. synthetics.describe_canaries(MaxResults=1000) -> emulator raises 400 ValidationException citing a ceiling of 20; AWS returns 200 with Canaries.
  2. synthetics.describe_canaries(MaxResults=21) -> same false rejection on AWS-accepted input.
  3. synthetics.describe_canaries(MaxResults=1001) -> should be the only rejection, with the message citing 1000.

Cause 3:
  1. synthetics.tag_resource(ResourceArn='not-an-arn', Tags={'k': 'v'}).
  2. Compare the ValidationException message's rendered pattern: emulator includes the leading ^ and trailing $, AWS does not.

Replay: synthetics.create-canary-invalid-runtime-version-rejected, synthetics.describe-canaries-max-results-out-of-range-rejected, synthetics.tag-resource-invalid-arn-bad-request-exception.
