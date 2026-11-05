Issue-Source: trace-sweep-divergence
Issue-Trace: cfetag-function-etag-chain
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfetag-function-etag-chain cfetag-key-group-etag-chain cloudfront-distribution-trusted-key-groups-active cloudfront-get-public-key-wrong-resource-type-id-rejected cloudfront-key-group-error-paths cloudfront-key-group-lifecycle cloudfront-key-group-lifecycle-with-members cloudfront-key-group-precondition-and-inuse-errors cloudfront-list-distributions-by-key-group-invalid-argument-rejected cloudfront-list-public-keys-and-key-groups-pagination cloudfront-public-key-and-key-group-lifecycle cloudfront-field-level-encryption-config-workflow cloudfront-fle-config-lifecycle-with-profile-reference cloudfront-fle-config-not-found-and-invalid-ifmatch-errors cloudfront-anycast-ip-list-create-limit-exceeded cloudfront-anycast-ip-list-error-paths cloudfront-anycast-ip-list-lifecycle cloudfront-icc-per-object-batch-too-large cloudfront-cfode-attach-rewrite-request-shape-mistakes cloudfront-cfode-rewrite-fix-function-state-and-etag-chain cloudfront-icc-sdk-batch-construction-mistakes
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|DeleteFunction|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteFunction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteFunction|4xx->4xx|status
Issue-Key: cloudfront|DeletePublicKey|2xx->4xx|status
Issue-Key: cloudfront|DeletePublicKey|4xx->4xx|status
Issue-Key: cloudfront|CreateAnycastIpList|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateAnycastIpList|4xx->2xx|status
Issue-Key: cloudfront|CreateInvalidation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|CreateInvalidation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateInvalidation|4xx->4xx|status
Issue-Key: cloudfront|UpdateDistribution|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|UpdateDistribution|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|UpdateDistribution|4xx->4xx|status
Issue-Key: cloudfront|UpdateDistribution|2xx->4xx|$.Distribution
Issue-Key: cloudfront|UpdateDistribution|2xx->4xx|$headers.etag
Issue-Key: cloudfront|UpdateDistribution|2xx->4xx|status
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFront refusals carry the wrong HTTP status and error code

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunction

## Description
Ten net-new wire divergences in which a CloudFront operation answers with an HTTP status and/or error code from the wrong class. Clients branch on status class and on `Error/Code`, so each of these sends a correct client down the wrong recovery path. This is distinct from the open issue 20261029T212223Z-cloudfront-if-match-etag-handling-wrong-precondition-message.md (which owns its own replay keys); the causes below are not covered by it, nor by 20261102T072100Z-cloudfront-three-refusal-divergences-oversized-tenant-invalidation-batch.md.

Cause 1 of 10: DeleteFunction answers 400 InvalidIfMatchVersion where AWS answers 412 PreconditionFailed.
AWS: Smithy: DeleteFunction binds errors PreconditionFailed (httpError 412) and InvalidIfMatchVersion (httpError 400); shape PreconditionFailed is documented "The precondition in one or more of the request fields evaluated to false." The recorded response is 412 with Code `PreconditionFailed` and Message "The request failed because it didn't meet the preconditions in one or more request-header fields." That message is entirely fixed text — it has no variable parts. AWS reserves InvalidIfMatchVersion for an If-Match header that is syntactically absent/unusable, and PreconditionFailed for a well-formed If-Match that does not match the resource's current ETag.
Emulator: answers 400 with Code `InvalidIfMatchVersion` and Message "The If-Match version is missing or not valid for the resource."
Why: a client that retries after re-reading the ETag (the correct response to 412) instead sees a 400 and treats its own request as malformed, so it never recovers.
Fix: the DeleteFunction handler's If-Match precondition check (and the shared ETag-precondition helper it calls) must distinguish a missing/unparseable If-Match from a stale one and raise PreconditionFailed/412 for the latter.
Traces: cfetag-function-etag-chain#task#4.

Cause 2 of 10: DeletePublicKey answers 409 where AWS deletes the key and answers 204.
AWS: Smithy: DeletePublicKey http {method DELETE, uri /2020-05-31/public-key/{Id}, code 204}; errors include PublicKeyInUse (httpError 409, "The specified public key is in use"). AWS returns 409 PublicKeyInUse only while a key group still references the key; once the referencing key groups are deleted the key deletes successfully with an empty 204.
Emulator: answers 409 for the delete of a public key whose referencing key groups have already been deleted in the same fixture.
Why: teardown of a public key becomes impossible — the reference count is never released, so the resource leaks and every subsequent delete fails. This cluster covers 10 affected steps across 10 fixtures.
Fix: the public-key in-use bookkeeping in the CloudFront state store — deleting a key group must drop its references to its member public keys so DeletePublicKey's in-use check clears.
Traces: cfetag-key-group-etag-chain#teardown#9 (plus 9 further affected steps in the same cluster).

Cause 3 of 10: DeletePublicKey answers 412 PreconditionFailed where AWS answers 204.
AWS: as Cause 2 — DeletePublicKey success is 204 with an empty body; IfMatch is an optional httpHeader on the input, and PreconditionFailed (412) is only correct when a supplied If-Match does not match the key's current ETag.
Emulator: answers 412 for a delete whose If-Match carries the ETag the emulator itself most recently returned for that public key.
Why: the documented delete sequence (get ETag, delete with If-Match) can never complete, so public keys cannot be removed.
Fix: the DeletePublicKey handler's precondition comparison — it must compare against the ETag currently held in state for that public key, including after an intervening update.
Traces: cloudfront-field-level-encryption-config-workflow#teardown#9 (3 affected steps).

Cause 4 of 10: DeletePublicKey answers 409 where AWS answers 404 for a key that no longer exists.
AWS: Smithy: DeletePublicKey errors include NoSuchPublicKey (httpError 404) and PublicKeyInUse (httpError 409). A second delete of an already-deleted key is a not-found condition, so AWS answers 404 NoSuchPublicKey.
Emulator: answers 409.
Why: an idempotent cleanup loop that treats 404 as "already gone" never terminates, because it is told the resource is still in use.
Fix: the DeletePublicKey handler must resolve existence before the in-use check, and the state store must actually remove the key on a successful delete.
Traces: cloudfront-public-key-and-key-group-lifecycle#teardown#19.

Cause 5 of 10: CreateAnycastIpList answers 202 success where AWS refuses with 400 EntityLimitExceeded.
AWS: Smithy: CreateAnycastIpList http code 202, errors include EntityLimitExceeded (httpError 400). The recorded refusal is 400 with Type `Sender`, Code `EntityLimitExceeded` and a Message whose fixed text is "To request a limit increase for your Anycast static IP list, contact AWS Support: " followed by a Service Quotas console URL for the cloudfront quota — the URL and quota code are fixed service-quota text, not per-run values.
Emulator: answers 202 and creates the list, emitting no ErrorResponse at all.
Why: a client exercising its quota-exhaustion path never sees the quota error, and the emulator accumulates resources AWS would have refused. 3 affected steps.
Fix: CreateAnycastIpList's validation — enforce the per-account Anycast IP list quota before the handler mutates state.
Traces: cloudfront-anycast-ip-list-create-limit-exceeded#task#1 (3 affected steps).

Cause 6 of 10: CreateInvalidation answers 413 BatchTooLarge where AWS answers 400 TooManyInvalidationsInProgress.
AWS: Smithy: CreateInvalidation binds BatchTooLarge (httpError 413) and TooManyInvalidationsInProgress (httpError 400). The recorded refusal is 400, Code `TooManyInvalidationsInProgress`, Message "Processing your request will cause you to exceed the maximum number of in-progress invalidations." (fixed text, no variable parts). The two limits are different: BatchTooLarge is the per-batch path count, TooManyInvalidationsInProgress is the count of concurrently in-progress invalidation batches on the distribution.
Emulator: answers 413 with Code `BatchTooLarge` and Message "Invalidation batch specified is too large."
Why: a client that would back off and retry when in-progress invalidations drain instead sees a permanent 413 and splits its batch, which does not help.
Fix: CreateInvalidation's validation must evaluate the in-progress-batch limit separately from the per-batch path limit and pick the matching error.
Traces: cloudfront-icc-per-object-batch-too-large#task#2.

Cause 7 of 10: UpdateDistribution answers 412 PreconditionFailed where AWS answers 400 MalformedXML for an invalid body.
AWS: Smithy: UpdateDistribution binds PreconditionFailed (412) and InvalidFunctionAssociation (400) among many 400-class validation errors. The recorded refusal is 400 with Code `MalformedXML` and a message whose fixed text is "N validation errors detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy value set: [viewer-request, viewer-response]" — N, the offending value and the member path come from the request; the value set is fixed (see FunctionAssociation.EventType doc: "You cannot use origin-facing event types (origin-request and origin-response) with a CloudFront function"). AWS validates the request body before evaluating If-Match.
Emulator: answers 412 with Code `PreconditionFailed`, never reporting the body defect.
Why: the client is told its ETag is stale, re-reads the config, resubmits the same invalid body and loops forever, never learning which member is wrong. 3 affected steps.
Fix: ordering in the UpdateDistribution handler — body/shape validation must run before the If-Match precondition check.
Traces: cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#8 (3 affected steps).

Cause 8 of 10: UpdateDistribution answers 412 PreconditionFailed for a valid, current If-Match where AWS answers 200.
AWS: Smithy: UpdateDistribution http {method PUT, uri /2020-05-31/distribution/{Id}/config, code 200}; output carries the Distribution payload and an ETag httpHeader. With an If-Match equal to the resource's current ETag the update succeeds with 200 and a fresh ETag.
Emulator: answers 412, and returns no Distribution body.
Why: the distribution can never be updated through the documented read-modify-write sequence. Because the emulator's ETag bookkeeping is wrong, the only way forward for a client is to omit If-Match entirely, which AWS would reject.
Fix: UpdateDistribution's precondition check and the ETag the serializer publishes must agree — the ETag returned by the preceding Get must be the one the handler accepts.
Traces: cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#7.

Cause 9 of 10: UpdateDistribution reports a constraint violation as ValidationException instead of MalformedXML.
AWS: Smithy: UpdateDistribution's bound errors contain no ValidationException; XML-body constraint failures are reported with Code `MalformedXML`. The recorded refusal is 400 with Code `MalformedXML`; its message's fixed text is "1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", with the value and member path taken from the request. The pattern AWS reports is partition-generalised and unanchored (`arn:aws(-[a-z]+)*:cloudfront::[0-9]{12}:function\/[a-zA-Z0-9-_]{1,64}`), which is broader than the model's FunctionAssociation.FunctionARN @pattern `^arn:aws:cloudfront::[0-9]{12}:function\/[a-zA-Z0-9-_]{1,64}$` — AWS's runtime wording is the requirement here.
Emulator: answers 400 with Code `ValidationException` and reports the model's anchored, aws-partition-only pattern.
Why: `ValidationException` is not in CloudFront's REST-XML error vocabulary, so SDK error mapping produces an unmodelled/unknown error instead of MalformedXML, and the reported pattern wrongly tells the caller that non-`aws` partition ARNs are invalid.
Fix: the UpdateDistribution validation layer's error classification and the pattern text it renders.
Traces: cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#10.

Cause 10 of 10: CreateInvalidation reports batch-construction defects as ValidationException instead of MalformedXML.
AWS: Smithy: CreateInvalidation's bound errors are AccessDenied, BatchTooLarge, InconsistentQuantities, InvalidArgument, MissingBody, NoSuchDistribution and TooManyInvalidationsInProgress — ValidationException is not among them. The recorded refusals are 400 with Code `MalformedXML`.
Emulator: answers 400 with Code `ValidationException` (status and message otherwise match). 3 affected steps.
Why: the SDK cannot map `ValidationException` to any modelled CloudFront error, so a client's `catch MalformedXML` branch never fires and the failure surfaces as an unknown service error.
Fix: CreateInvalidation's InvalidationBatch validation must emit the REST-XML MalformedXML error code rather than a generic ValidationException.
Traces: cloudfront-icc-sdk-batch-construction-mistakes#task#3 (3 affected steps).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 78.2%, 102 divergence clusters, all cloudfront).

Cause 1 — cfetag-function-etag-chain#task#4 DeleteFunction: expected 412 / Code 'PreconditionFailed' / "The request failed because it didn't meet the preconditions in one or more request-header fields."; got 400 / Code 'InvalidIfMatchVersion' / 'The If-Match version is missing or not valid for the resource.'
Cause 2 — cfetag-key-group-etag-chain#teardown#9 DeletePublicKey: expected status 204, got 409 (10 affected steps: cloudfront-distribution-trusted-key-groups-active#teardown#12, cloudfront-get-public-key-wrong-resource-type-id-rejected#teardown#5, cloudfront-key-group-error-paths#teardown#10, cloudfront-key-group-lifecycle#teardown#9, cloudfront-key-group-lifecycle-with-members#teardown#10, cloudfront-key-group-precondition-and-inuse-errors#teardown#7, cloudfront-list-distributions-by-key-group-invalid-argument-rejected#teardown#9, cloudfront-list-public-keys-and-key-groups-pagination#teardown#8, cloudfront-public-key-and-key-group-lifecycle#task#16).
Cause 3 — cloudfront-field-level-encryption-config-workflow#teardown#9 DeletePublicKey: expected 204, got 412 (also cloudfront-fle-config-lifecycle-with-profile-reference#teardown#8, cloudfront-fle-config-not-found-and-invalid-ifmatch-errors#teardown#8).
Cause 4 — cloudfront-public-key-and-key-group-lifecycle#teardown#19 DeletePublicKey: expected 404, got 409.
Cause 5 — cloudfront-anycast-ip-list-create-limit-exceeded#task#1 CreateAnycastIpList: expected 400 with ErrorResponse {Error:{Type:Sender, Code:EntityLimitExceeded, Message:'To request a limit increase for your Anycast static IP list, contact AWS Support: <service-quotas URL>'}}, got 202 and no ErrorResponse (also cloudfront-anycast-ip-list-error-paths#task#1, cloudfront-anycast-ip-list-lifecycle#task#1).
Cause 6 — cloudfront-icc-per-object-batch-too-large#task#2 CreateInvalidation: expected 400 / 'TooManyInvalidationsInProgress' / 'Processing your request will cause you to exceed the maximum number of in-progress invalidations.'; got 413 / 'BatchTooLarge' / 'Invalidation batch specified is too large.'
Cause 7 — cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#8 UpdateDistribution: expected 400 / 'MalformedXML' / "1 validation errors detected: Value 'origin-request' at 'distributionConfig.defaultCacheBehavior.functionAssociations.items.1.member.eventType' failed to satisfy constraint: Member must satisfy value set: [viewer-request, viewer-response]"; got 412 / 'PreconditionFailed' (also #task#11 and cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#6).
Cause 8 — cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#7 UpdateDistribution: expected 200 with Distribution payload and ETag header, got 412 and no payload.
Cause 9 — cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#10 UpdateDistribution: expected Code 'MalformedXML' with pattern 'arn:aws(-[a-z]+)*:cloudfront::[0-9]{12}:function\\/[a-zA-Z0-9-_]{1,64}'; got Code 'ValidationException' with pattern '^arn:aws:cloudfront::[0-9]{12}:function\\/[a-zA-Z0-9-_]{1,64}$'.
Cause 10 — cloudfront-icc-sdk-batch-construction-mistakes#task#3 CreateInvalidation: expected Code 'MalformedXML', got 'ValidationException' (also #task#4, #task#5).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cfetag-function-etag-chain and inspect step task#4 (the representative). Then replay, in order: cfetag-key-group-etag-chain#teardown#9, cloudfront-field-level-encryption-config-workflow#teardown#9, cloudfront-public-key-and-key-group-lifecycle#teardown#19, cloudfront-anycast-ip-list-create-limit-exceeded#task#1, cloudfront-icc-per-object-batch-too-large#task#2, cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#8, cloudfront-cfode-rewrite-fix-function-state-and-etag-chain#task#7, cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#10, cloudfront-icc-sdk-batch-construction-mistakes#task#3.
4. Compare each step's HTTP status and ErrorResponse/Error/Code+Message against the recorded response. All ten must match before the issue is closed.
