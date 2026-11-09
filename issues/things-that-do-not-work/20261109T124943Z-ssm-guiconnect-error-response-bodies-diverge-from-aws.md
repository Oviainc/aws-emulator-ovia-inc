Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-guiconnect-get-unconfigured-returns-resource-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-guiconnect-get-unconfigured-returns-resource-not-found ssm-guiconnect-update-bucket-name-and-owner-shape-validation ssm-guiconnect-update-client-token-length-validation-boundary ssm-guiconnect-update-recording-destinations-bucket-count-validation
Issue-Shape-Id: com.amazonaws.ssmguiconnect#SSMGuiConnect
Issue-Key: ssm-guiconnect|GetConnectionRecordingPreferences|4xx->4xx|$.requestId
Issue-Key: ssm-guiconnect|UpdateConnectionRecordingPreferences|4xx->4xx|$.requestId
Issue-Key: ssm-guiconnect|UpdateConnectionRecordingPreferences|4xx->4xx|$.message
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# ssm-guiconnect error response bodies diverge from AWS: no top-level requestId on any error, and @pattern refusals carry the framework envelope message

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnectionRecordingPreferences

## Description
Two independent root causes on the ssm-guiconnect (sdkId "SSM GuiConnect", restJson1, sigv4) error-response path, grounded in 4 wire divergence clusters covering 9 replayed steps. Both are in one service and are filed as one bundled work item; BOTH must be closed before submitting.

Related open issue (does NOT cover these steps — it carries no replay key for any fixture step in this corpus, and the replay reported owned_by=[] for all four clusters): 20261029T171307Z-ssm-guiconnect-3-net-new-behavioral-root-causes.md. That issue is grounded in the behavioral suite and describes the request id as a ResponseMetadata/header-envelope value and the @pattern wording from the botocore side; the divergences below are wire-level assertions on the JSON error BODY and must be closed against the recorded wire bodies.

=== Cause 1 of 2: every ssm-guiconnect error body omits the top-level `requestId` field, on every error class and every operation ===

AWS: recorded behavior only. The Smithy model declares no request-id member anywhere — ResourceNotFoundException, AccessDeniedException and ValidationException each have exactly one member, `message` (required, target ErrorMessage), and no output shape carries an id (Smithy: ResourceNotFoundException httpError 404, AccessDeniedException httpError 403, ValidationException httpError 400; all three are service-common errors bound to all three operations). The recordings nonetheless show a populated top-level `requestId` sibling of `message` in the JSON error body of EVERY error response, across all three error classes:
  * 404 ResourceNotFoundException from GetConnectionRecordingPreferences
  * 403 AccessDeniedException from UpdateConnectionRecordingPreferences
  * 400 ValidationException from UpdateConnectionRecordingPreferences
The id's VALUE is per-run and is not a requirement (the corpus asserts it as '<shaped-not-asserted>' — presence and shape only); its PRESENCE on every error body is.

Emulator: the error body contains `message` and nothing else — `$.requestId` is absent (replay detail on all four clusters: "$.requestId: missing (expected '<shaped-not-asserted>', got None)"). The HTTP status and the error code are correct on every one of these steps (404->404, 403->403, 400->400), so this is a single omission on the shared restJson1 error serializer, not a per-operation or per-error-class bug: it reproduces identically on GetConnectionRecordingPreferences and on UpdateConnectionRecordingPreferences, and on the 404, 403 and 400 paths alike.

Why: a client that correlates or logs the service request id out of the error body (support escalation, retry/diagnostic tooling, any caller echoing the id into its own logs) reads null where AWS always supplies an id. Because the field is not modeled, an SDK cannot recover it from a modeled member — the body field is the only place it appears, so dropping it loses the id outright for every failing call.

Fix: the shared restJson1 error-response serialization for this crate — the Err arm of the response path the generated ServiceDefinition renders through, so that it emits the envelope `requestId` alongside `message` for every modeled error. Not a per-handler change: no operation handler can add an envelope field.

Traces: ssm-guiconnect-get-unconfigured-returns-resource-not-found#task#2 (404 Get, cluster also covers task#3); ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#1 (403 Update, cluster also covers ssm-guiconnect-update-client-token-length-validation-boundary#task#2); ssm-guiconnect-update-client-token-length-validation-boundary#task#1 (400 Update, cluster also covers ssm-guiconnect-update-recording-destinations-bucket-count-validation#task#1 and #task#2); ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#2 (400 Update, cluster also covers #task#3 — this cluster diverges on BOTH causes).

=== Cause 2 of 2: a BucketName @pattern violation is refused with the framework's aggregated validation envelope (echoing the caller's value and the server-side regex) instead of this service's "isn't valid." sentence ===

AWS: recorded behavior for the message wording; the model states the code and the constraint. ssm-guiconnect refuses a malformed S3Bucket member of UpdateConnectionRecordingPreferences with 400 ValidationException (Smithy: ValidationException, errorKind client, httpError 400, `message` required) worded as ONE sentence naming the member by its dotted MODEL path and withholding the offending value:
  "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName isn't valid."
FIXED text: "The value provided for " and " isn't valid.". The ONLY variable part is the dotted model member path of the member that failed, in the model's own PascalCase spelling, with no list index and no value echo. The constraint itself is modeled: Smithy S3Bucket$BucketName @pattern "(?=^.{3,63}$)(?!^(\\d+\\.)+\\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])\\.)*([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])$)", required; S3Bucket$BucketOwner -> AccountId @pattern "^[0-9]{12}$", required. The same sentence form applies to whichever member failed (…S3Buckets.BucketOwner for the owner member).

Emulator: the status (400) and the code (ValidationException) are correct, but the message is the framework's aggregated constraint envelope, which additionally reflects the caller's value and the compiled regex back to the client: "1 validation error detected: Value '<the submitted value>' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.<index>.member.bucketName' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modeled regex>". Three things diverge in that one wording: (a) the "N validation error detected: " prefix AWS does not use, (b) a lowerCamel framework path with a ".<index>.member." list segment instead of the model's dotted PascalCase path with no index, and (c) the echo of the request value and of the server-side pattern, both of which AWS keeps out of this error body.

Why: the message is the ONLY place the offending member is named — the error shape has no member-name field — so a client that displays the refusal or parses which member it got wrong out of the message sees a sentence AWS never emits, with a different member-path spelling, and cannot match on AWS's wording. The value/regex echo also reflects caller input and an internal server pattern into a response AWS keeps free of both.

Fix: the UpdateConnectionRecordingPreferences validation path for this crate — the seam that words a constraint violation before it reaches the serializer, so a ViolationKind::Pattern on an S3Bucket member renders the "The value provided for <ModelMemberPath> isn't valid." sentence instead of the framework envelope, mapping the framework's lowerCamel violation path to the model's dotted PascalCase member path with the list-index segment dropped. The hand-rolled BucketName check must be reworded the same way rather than calling the framework's envelope renderer.

Traces: ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#2 (cluster also covers #task#3, the BucketOwner member).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout: emulator_up=true, bootstrap_signal=false, steps_total=10, steps_passed=1, steps_mismatched=9, score_pct=10.0, divergence_count=4. All 4 clusters are ssm-guiconnect; none carried stub_path and none carried owned_by.

Cause 1 — `$.requestId` missing from every error body (4 clusters, 9 steps):
  [mismatch] ssm-guiconnect-get-unconfigured-returns-resource-not-found # task#2  GetConnectionRecordingPreferences
    expected status 404, actual status 404 (correct)
    detail: $.requestId: missing (expected '<shaped-not-asserted>', got None)
    affected: ssm-guiconnect-get-unconfigured-returns-resource-not-found#task#2, ssm-guiconnect-get-unconfigured-returns-resource-not-found#task#3
    divergence_key: service=ssm-guiconnect|GetConnectionRecordingPreferences|status=404->404|keys=$.requestId
  [mismatch] ssm-guiconnect-update-bucket-name-and-owner-shape-validation # task#1  UpdateConnectionRecordingPreferences
    expected status 403, actual status 403 (correct)
    detail: $.requestId: missing (expected '<shaped-not-asserted>', got None)
    affected: ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#1, ssm-guiconnect-update-client-token-length-validation-boundary#task#2
    divergence_key: service=ssm-guiconnect|UpdateConnectionRecordingPreferences|status=403->403|keys=$.requestId
  [mismatch] ssm-guiconnect-update-client-token-length-validation-boundary # task#1  UpdateConnectionRecordingPreferences
    expected status 400, actual status 400 (correct)
    detail: $.requestId: missing (expected '<shaped-not-asserted>', got None)
    affected: ssm-guiconnect-update-client-token-length-validation-boundary#task#1, ssm-guiconnect-update-recording-destinations-bucket-count-validation#task#1, ssm-guiconnect-update-recording-destinations-bucket-count-validation#task#2
    divergence_key: service=ssm-guiconnect|UpdateConnectionRecordingPreferences|status=400->400|keys=$.requestId
  [mismatch] ssm-guiconnect-update-bucket-name-and-owner-shape-validation # task#2 (also diverges on Cause 2)
    detail includes: $.requestId: missing (expected '<shaped-not-asserted>', got None)
  => the omission spans 3 error classes (404/403/400) and 2 operations (Get/Update), i.e. one shared error serializer.

Cause 2 — @pattern ValidationException message wording:
  [mismatch] ssm-guiconnect-update-bucket-name-and-owner-shape-validation # task#2  UpdateConnectionRecordingPreferences
    expected status 400, actual status 400 (correct); code ValidationException (correct)
    detail: $.message: value_mismatch
      expected: "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName isn't valid."
      actual:   "1 validation error detected: Value '192.168.1.1' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketName' failed to satisfy constraint: Member must satisfy regular expression pattern: (?=^.{3,63}$)(?!^(\\d+\\.)+\\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])\\.)*([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])$)"
    affected: ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#2, ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#3
    divergence_key: service=ssm-guiconnect|UpdateConnectionRecordingPreferences|status=400->400|keys=$.message,$.requestId
    Message anatomy — FIXED: "The value provided for " + " isn't valid."; VARIABLE: the failing member's dotted model path (here ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName; …BucketOwner for the owner member in task#3). The value '192.168.1.1' and the regex text in the emulator's message are NOT part of AWS's template at all — AWS emits neither.

Model evidence (smithy_query on services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json):
  service SSMGuiConnect, sdkId "SSM GuiConnect", protocols [restJson1], authSchemes [sigv4], 3 operations.
  common_errors: AccessDeniedException, ConflictException, InternalServerException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException, ValidationException.
  ResourceNotFoundException: errorKind client, httpError 404, members {message: ErrorMessage required}.
  AccessDeniedException:     errorKind client, httpError 403, members {message: ErrorMessage required}.
  ValidationException:       errorKind client, httpError 400, members {message: ErrorMessage required}.
  => no modeled request-id member anywhere, so the recorded top-level `requestId` is a protocol envelope field of the error body (recorded behavior only).
  S3Bucket: BucketName required @pattern "(?=^.{3,63}$)(?!^(\\d+\\.)+\\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])\\.)*([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])$)"; BucketOwner -> AccountId required @pattern "^[0-9]{12}$".
  UpdateConnectionRecordingPreferencesRequest: ConnectionRecordingPreferences required; ClientToken @length 1..64, idempotencyToken.
  All three operations are POST /<OperationName>.

## Steps to Reproduce
Boot the emulator from this checkout and replay the ssm-guiconnect wire fixtures, or drive it directly with a sigv4-signing HTTP client / boto3 ssm-guiconnect client in the corpus Region.

Cause 1 (`requestId` absent from every error body):
  1. POST /GetConnectionRecordingPreferences on an account with no preferences configured. The emulator answers 404 with body {"message": "The following  resource cannot be found: 'ConnectionRecordingPreferences'"} — the recorded AWS body carries a top-level "requestId" sibling as well. Replay: ssm-guiconnect-get-unconfigured-returns-resource-not-found#task#2 (and #task#3).
  2. POST /UpdateConnectionRecordingPreferences with a WELL-SHAPED body (valid BucketName, 12-digit BucketOwner, ClientToken 1..64 chars) so the request reaches the account-level gate: the emulator answers 403 AccessDeniedException with a body carrying only "message" — no "requestId". Replay: ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#1 (and ssm-guiconnect-update-client-token-length-validation-boundary#task#2).
  3. POST /UpdateConnectionRecordingPreferences with a ClientToken past the modeled @length max of 64, and separately with a RecordingDestinations.S3Buckets list past its modeled bound: the emulator answers 400 ValidationException with a body carrying only "message" — no "requestId". Replay: ssm-guiconnect-update-client-token-length-validation-boundary#task#1, ssm-guiconnect-update-recording-destinations-bucket-count-validation#task#1 and #task#2.
  4. Assert on every one of the above: the JSON error body has a top-level "requestId" whose value is a well-formed id (the value itself is per-run and must NOT be asserted literally).

Cause 2 (@pattern refusal wording):
  1. POST /UpdateConnectionRecordingPreferences with an S3Buckets entry whose BucketName is IP-address-shaped (excluded by the modeled pattern) and an otherwise well-shaped body.
     Expected: 400 ValidationException, message exactly "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName isn't valid."
     Actual:   400 ValidationException, message "1 validation error detected: Value '<submitted value>' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.<index>.member.bucketName' failed to satisfy constraint: Member must satisfy regular expression pattern: <modeled regex>".
     Replay: ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#2.
  2. Repeat with a BucketOwner that is not 12 digits: expected "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketOwner isn't valid."; the emulator returns the analogous framework envelope naming '…s3Buckets.<index>.member.bucketOwner' and the ^[0-9]{12}$ regex. Replay: ssm-guiconnect-update-bucket-name-and-owner-shape-validation#task#3.
  3. Confirm the refusal still precedes the account-level gate and created no state: POST /GetConnectionRecordingPreferences still answers 404 ResourceNotFoundException.

Full replay to confirm both causes: replay the whole simparity-wire-aws-v1 ssm-guiconnect set — all 9 mismatched steps across ssm-guiconnect-get-unconfigured-returns-resource-not-found, ssm-guiconnect-update-bucket-name-and-owner-shape-validation, ssm-guiconnect-update-client-token-length-validation-boundary and ssm-guiconnect-update-recording-destinations-bucket-count-validation must pass.
