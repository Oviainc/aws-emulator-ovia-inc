Issue-Source: trace-sweep-divergence
Issue-Trace: lambdamicrovms-image-request-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdamicrovms-image-request-validation lambdamicrovms-image-tagging-and-run-refusals lambda-microvms-image-error-paths lambdamicrovms-customer-image-lifecycle lambdamicrovms-image-version-status-and-update
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|CreateMicrovmImage|4xx->2xx|$.message
Issue-Key: lambda-microvms|CreateMicrovmImage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda-microvms|CreateMicrovmImage|4xx->2xx|status
Issue-Key: lambda-microvms|CreateMicrovmImage|4xx->4xx|$.message
Issue-Key: lambda-microvms|CreateMicrovmImage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda-microvms|CreateMicrovmImage|4xx->4xx|status
Issue-Key: lambda-microvms|GetMicrovmImage|4xx->4xx|$.message
Issue-Key: lambda-microvms|GetMicrovmImage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda-microvms|GetMicrovmImage|4xx->4xx|status
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.baseImageVersion
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.cpuConfigurations
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.egressNetworkConnectors
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.imageVersion
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.logging
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.resources
Issue-Key: lambda-microvms|CreateMicrovmImage|2xx->2xx|$.updatedAt
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.id
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.latestActiveImageVersion
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.latestFailedImageVersion
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.state
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.tags
Issue-Key: lambda-microvms|GetMicrovmImage|2xx->2xx|$.updatedAt
Issue-Key: lambda-microvms|GetMicrovmImage|4xx->2xx|$
Issue-Key: lambda-microvms|GetMicrovmImage|4xx->2xx|status
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# lambda-microvms image path: create validation bypassed, response projection incomplete, delete not durable

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrovmImage

## Description
Seven distinct causes on the lambda-microvms MicroVM-image surface (CreateMicrovmImage / GetMicrovmImage / DeleteMicrovmImage). They share one shape of fault: the image handler neither validates its inputs nor projects or persists the image aggregate that the model defines, so it accepts requests AWS refuses, answers with a stripped-down body, and never makes a delete take effect. All seven must be closed before this issue is submitted.

Cause 1 of 7: CreateMicrovmImage accepts a codeArtifact URI that is not an S3 URI.
AWS: CreateMicrovmImage (Smithy: restJson1, POST /2025-09-09/microvm-images, success code 201; input member codeArtifact targets CodeArtifact and is @required; modeled error ValidationException @httpError 400) resolves the code artifact location before creating anything and refuses a non-S3 location with HTTP 400 ValidationException. The refusal message is built from the offending input and names the required form, recorded as the example 'CodeArtifact URI must be a valid S3 URI in the format: s3://<bucket>/<key>'.
Emulator: answers HTTP 201 and creates the image; no message and no x-amzn-errortype are emitted.
Why: a client that mistypes or templates the artifact location wrongly gets a successful create and an image that can never build. The error AWS uses to surface the typo at submit time never arrives, so the failure only shows up later (or never), and negative-path tests that assert a 400 pass against a success.
Fix: CreateMicrovmImage input validation — add the codeArtifact URI scheme/format check ahead of resource creation and raise ValidationException (400).
Traces: lambdamicrovms-image-request-validation#task#2.

Cause 2 of 7: CreateMicrovmImage never resolves baseImageArn, and the name-conflict check runs ahead of input validation.
AWS: baseImageArn is @required (NonBlankString, @length 1..2048, @pattern ^[^\\s]+$) and is resolved against the managed-image catalog; a base image that does not exist is refused with HTTP 400, recorded message example 'Managed VMI not found: arn:aws:lambda:us-west-2:aws:microvm-image:sim2real-probe-absent-0'. Input and reference validation precede the account-level name-uniqueness check (CreateMicrovmImage also models ConflictException @httpError 409 and ResourceNotFoundException @httpError 404, so the status alone distinguishes the two refusals).
Emulator: answers HTTP 409 with x-amzn-errortype ConflictException and a name-collision message (example: 'a MicroVM image named sim2real-probe-mvmval-87fd6f86 already exists in this account'); the unresolvable baseImageArn is never examined.
Why: the client is told the wrong thing about its own request — a bad base-image reference is reported as a naming conflict, so retry logic that reacts to 409 by renaming loops forever while the real defect (the base image) is untouched. Status and error code both diverge, so SDK error-type dispatch takes the wrong branch.
Fix: CreateMicrovmImage handler — resolve baseImageArn against the managed-image catalog and validate inputs before the name-uniqueness check; refuse an unresolvable base image with 400.
Traces: lambdamicrovms-image-request-validation#task#3 (cluster also covers lambdamicrovms-image-tagging-and-run-refusals#task#6).

Cause 3 of 7: GetMicrovmImage treats a syntactically invalid image identifier as a missing resource.
AWS: GetMicrovmImage (Smithy: GET /2025-09-09/microvm-images/{imageIdentifier}, imageIdentifier @httpLabel @required targeting MicrovmImageIdentifier @length 1..256; modeled errors ValidationException @httpError 400 and ResourceNotFoundException @httpError 404) distinguishes a malformed identifier from an absent one: an identifier that is neither a bare name resolvable in the account nor a well-formed ARN is refused with HTTP 400, recorded message example 'Invalid ARN format: sim2real-probe-bare-id'.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and a lookup-miss message.
Why: 400 and 404 mean opposite things to a caller — 404 is a legitimate, often-retried 'not yet created' condition, while 400 says the request itself can never succeed. A client polling for an image it believes is being built will retry a permanently malformed identifier forever instead of failing fast.
Fix: GetMicrovmImage input validation — validate the identifier's form before the state lookup and raise ValidationException (400) for a malformed one.
Traces: lambdamicrovms-image-request-validation#task#4.

Cause 4 of 7: the constraint-violation message echoes the Smithy @pattern with its regex anchors instead of stripping them.
AWS: for a @pattern violation the service returns HTTP 400 with the standard text '1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>', where <pattern> is the member's constraint with the leading ^ and trailing $ removed. Smithy: CreateMicrovmImage.name targets ImageName with @pattern ^[a-zA-Z0-9-_]+$, and the recording shows the pattern rendered as [a-zA-Z0-9-_]+. The message is built from the request member and its modeled constraint, so the rendering is a known rule, not incidental text.
Emulator: emits the same message but renders the pattern as ^[a-zA-Z0-9-_]+$, keeping both anchors.
Why: callers and test suites that match the documented constraint text (or re-compile the echoed pattern) see a string AWS never produces, so assertions on the refusal reason fail even though the refusal itself was correct.
Fix: the shared constraint-violation message formatter — strip the anchoring ^ and $ when rendering a @pattern into the validation message.
Traces: lambda-microvms-image-error-paths#task#1 (cluster also covers lambdamicrovms-image-request-validation#task#1).

Cause 5 of 7: the CreateMicrovmImage 201 body drops most of the modeled response and mints the wrong first version number.
AWS: CreateMicrovmImageResponse echoes the whole created aggregate — Smithy members baseImageVersion, cpuConfigurations, egressNetworkConnectors, logging, resources and updatedAt are all modeled on the response, and imageVersion is @required. The recording shows the first version minted as '1.0' and an additional 'id' member in the body (recorded behavior only — 'id' is not in the on-disk model, so the recording is the reference for it).
Emulator: returns 201 but omits baseImageVersion, cpuConfigurations, egressNetworkConnectors, logging, resources, updatedAt and id entirely, and reports imageVersion as '1' rather than '1.0'.
Why: the create response is the only place a client sees the configuration the service actually accepted (the normalized CPU configurations, resource floor, logging target and egress connectors). With them absent the client cannot confirm what it got, and the version string it must pass back to GetMicrovmImageVersion / RunMicrovm / DeleteMicrovmImageVersion is in the wrong format, so every subsequent version-addressed call built from this response targets a version the service does not name.
Fix: CreateMicrovmImage serializer and the image state record — persist and project the full modeled aggregate, and mint version identifiers in the service's major.minor form.
Traces: lambdamicrovms-customer-image-lifecycle#setup#2 (cluster also covers lambdamicrovms-image-version-status-and-update#setup#2).

Cause 6 of 7: a failed image version is never reflected, so GetMicrovmImage reports a healthy image.
AWS: GetMicrovmImageOutput models state (MicrovmImageState enum), latestActiveImageVersion and latestFailedImageVersion as separate members plus tags and updatedAt. After the only version of an image fails its build the image reports state CREATE_FAILED, carries latestFailedImageVersion for that version, and carries no latestActiveImageVersion. The recording also shows an 'id' member and an empty tags map (recorded behavior only for 'id').
Emulator: reports state CREATED, sets latestActiveImageVersion to the failed version, leaves latestFailedImageVersion unset, and omits id, tags and updatedAt.
Why: the client cannot see that the build failed. Worse, the failed version is advertised as the active one, so a caller that reads latestActiveImageVersion and launches from it is pointed at a version that cannot run — a build failure is silently converted into a launch-time failure, and any wait-for-failure or retry-on-CREATE_FAILED loop never fires.
Fix: the MicroVM image state machine and the GetMicrovmImage projection — record version build outcome, drive image state from it, and populate latestFailedImageVersion / tags / updatedAt.
Traces: lambdamicrovms-customer-image-lifecycle#task#3.

Cause 7 of 7: DeleteMicrovmImage does not delete — the image stays readable indefinitely.
AWS: DeleteMicrovmImage (Smithy: DELETE /2025-09-09/microvm-images/{imageIdentifier}, documented as idempotent) removes the image, after which GetMicrovmImage answers HTTP 404 ResourceNotFoundException.
Emulator: after the delete, GetMicrovmImage keeps answering HTTP 200 with the full image body (state CREATED). The fixture polled for the 404 for the full 60s cap across 13 attempts and never observed it.
Why: deletion is not durable, so the resource leaks. Any client or test that cleans up and then waits for the resource to disappear blocks until its own timeout, and a later create that reuses the name collides with a tombstone that should not exist — which is exactly the spurious 409 seen in cause 2.
Fix: DeleteMicrovmImage handler and the image state store — actually remove the image (and its versions) so subsequent reads miss.
Traces: lambdamicrovms-customer-image-lifecycle#teardown#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Expected values are the recorded AWS responses.

Cause 1 — lambdamicrovms-image-request-validation#task#2, CreateMicrovmImage: expected status 400, got 201. $.message missing (expected 'CodeArtifact URI must be a valid S3 URI in the format: s3://<bucket>/<key>'); $headers.x-amzn-errortype missing.

Cause 2 — lambdamicrovms-image-request-validation#task#3, CreateMicrovmImage: expected status 400, got 409. $.message expected 'Managed VMI not found: arn:aws:lambda:us-west-2:aws:microvm-image:sim2real-probe-absent-0', got 'a MicroVM image named sim2real-probe-mvmval-87fd6f86 already exists in this account'; $headers.x-amzn-errortype got 'ConflictException'. Cluster size 2 (also lambdamicrovms-image-tagging-and-run-refusals#task#6).

Cause 3 — lambdamicrovms-image-request-validation#task#4, GetMicrovmImage: expected status 400, got 404. $.message expected 'Invalid ARN format: sim2real-probe-bare-id', got 'no MicroVM image matches the identifier sim2real-probe-bare-id'; $headers.x-amzn-errortype got 'ResourceNotFoundException'.

Cause 4 — lambda-microvms-image-error-paths#task#1, CreateMicrovmImage: status 400 both sides. $.message expected "...Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+", got "...Member must satisfy regular expression pattern: ^[a-zA-Z0-9-_]+$". Cluster size 2 (also lambdamicrovms-image-request-validation#task#1).

Cause 5 — lambdamicrovms-customer-image-lifecycle#setup#2, CreateMicrovmImage: status 201 both sides. $.baseImageVersion missing (expected '1.0'); $.cpuConfigurations missing (expected [{'architecture': 'ARM_64'}]); $.egressNetworkConnectors missing (expected ['arn:aws:lambda:us-west-2:aws:network-connector:aws-network-connector:INTERNET_EGRESS']); $.id missing; $.logging missing (expected {'cloudWatch': {'logGroup': '/aws/lambda-microvms/...'}}); $.resources missing (expected [{'minimumMemoryInMiB': 2048}]); $.updatedAt missing; $.imageVersion expected '1.0', got '1'. Cluster size 2 (also lambdamicrovms-image-version-status-and-update#setup#2).

Cause 6 — lambdamicrovms-customer-image-lifecycle#task#3, GetMicrovmImage: status 200 both sides. $.state expected 'CREATE_FAILED', got 'CREATED'; $.latestFailedImageVersion missing (expected '1.0'); $.latestActiveImageVersion expected absent, got '1'; $.id, $.tags (expected {}) and $.updatedAt missing.

Cause 7 — lambdamicrovms-customer-image-lifecycle#teardown#12, GetMicrovmImage: expected status 404, got 200; poll capped at 60s, 'status 200 not in [404] after 13 attempt(s)'. Body still {'imageArn': 'arn:aws:lambda:us-west-2:000000000000:microvm-image:sim2real-probe-mvmimg-8661b5c1', 'name': 'sim2real-probe-mvmimg-8661b5c1', 'state': 'CREATED', 'latestActiveImageVersion': '1', ...}.

Model grounding: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json — CreateMicrovmImage (201, ValidationException 400, ConflictException 409, ResourceNotFoundException 404), CreateMicrovmImageResponse, GetMicrovmImageOutput, ValidationException (@httpError 400), ImageName @pattern ^[a-zA-Z0-9-_]+$.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the lambda-microvms clusters.
3. Per cause, against the emulator's lambda-microvms endpoint:
   - Cause 1: POST /2025-09-09/microvm-images with a codeArtifact URI that is not an s3:// URI -> observe 201 instead of 400 ValidationException. Fixture lambdamicrovms-image-request-validation#task#2.
   - Cause 2: POST /2025-09-09/microvm-images with a baseImageArn naming a managed image that does not exist -> observe 409 ConflictException instead of 400. Fixture lambdamicrovms-image-request-validation#task#3.
   - Cause 3: GET /2025-09-09/microvm-images/<bare-non-ARN-identifier> -> observe 404 ResourceNotFoundException instead of 400 ValidationException. Fixture lambdamicrovms-image-request-validation#task#4.
   - Cause 4: POST /2025-09-09/microvm-images with name='not a valid name!' -> observe the echoed pattern carrying ^ and $. Fixture lambda-microvms-image-error-paths#task#1.
   - Cause 5: POST /2025-09-09/microvm-images with cpuConfigurations, resources, logging and egressNetworkConnectors set -> compare the 201 body against CreateMicrovmImageResponse and check imageVersion. Fixture lambdamicrovms-customer-image-lifecycle#setup#2.
   - Cause 6: drive the image's only version to a failed build, then GET /2025-09-09/microvm-images/<id> -> observe state CREATED and the failed version reported as latestActiveImageVersion. Fixture lambdamicrovms-customer-image-lifecycle#task#3.
   - Cause 7: DELETE /2025-09-09/microvm-images/<id>, then poll GET /2025-09-09/microvm-images/<id> -> observe 200 forever instead of 404. Fixture lambdamicrovms-customer-image-lifecycle#teardown#12.
4. Re-replay all seven fixture#step bindings listed in trace_ids plus the representative to confirm a fix.
