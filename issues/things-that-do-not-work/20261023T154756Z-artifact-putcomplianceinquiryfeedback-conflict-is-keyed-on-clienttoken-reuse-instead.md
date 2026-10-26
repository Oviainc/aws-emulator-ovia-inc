Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: artifact.put-compliance-inquiry-feedback-conflicting-payload-rejected
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact.put-compliance-inquiry-feedback-conflicting-payload-rejected|run#16|AssertionError|assert message == "Feedback already submitted for this response version.", (
Issue-Commit: 6e9732dd4597c448322b99e6bbcbf9baa588c73a

# PutComplianceInquiryFeedback conflict is keyed on clientToken reuse instead of AWS's one-feedback-per-response-revision rule

> _Note: this issue was identified against the code base at commit `6e9732dd4597c448322b99e6bbcbf9baa588c73a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutComplianceInquiryFeedback

## Description
ONE cause in the artifact emulator (sdkId Artifact, restJson1, sigv4), grounded in a behavioral check of suite simparity-behavioral-aws-v1.

Cause 1 of 1 - PutComplianceInquiryFeedback refuses a repeat submission for the wrong reason: the emulator's ConflictException is raised by an idempotency-token bookkeeping rule, while AWS's is raised by the state of the targeted response revision.

AWS: Smithy (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json): PutComplianceInquiryFeedback, PUT /v1/compliance-inquiry/putFeedback, http code 200, @idempotent; input members complianceInquiryId @required @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$, rating @required (FeedbackRating), queryIdentifier : Integer, responseRevisionId : Integer documented as "The response revision ID. Use this value to prevent submitting feedback on a stale response.", clientToken : IdempotentClientToken @idempotencyToken @length 1..128 @pattern ^[\w\-]+$ documented as "A unique, case-sensitive identifier to ensure that the operation completes no more than one time. If this token matches a previous request, the service ignores the request, but does not return an error"; modeled errors include ConflictException @httpError 409 (members message/resourceId/resourceType all @required, doc "Request to create/modify content would result in a conflict"). Recorded behavior for the refusal condition: a given (queryIdentifier, responseRevisionId) accepts exactly ONE submitted feedback ever; a second submission against that revision is refused with HTTP 409 / ConflictException whose message states the revision-scoped rule (recorded example: "Feedback already submitted for this response version."). The recording shows this holds for a second submission even with a FRESH clientToken, i.e. the conflict is a property of the response revision, not of the token. The first submission returns 200 with submittedAt, and a fresh clientToken submitting the SAME rating against a DIFFERENT inquiry's untouched revision succeeds, so neither the rating value nor the endpoint is at fault.

Emulator: the status class and error code are correct (ConflictException), but the refusal is attributed to idempotency-token bookkeeping: the returned message is "the clientToken was already used to submit different feedback on a compliance inquiry". This is the observable evidence that the conflict is keyed on (clientToken, payload) rather than on whether the targeted (queryIdentifier, responseRevisionId) already carries feedback. Hypothesis (not asserted by the check): because the emulator's conflict key is the token, a second feedback submitted on an already-fed-back revision with a FRESH clientToken would be ACCEPTED where AWS refuses it with 409 — an overly-permissive path. Also note ConflictException models resourceId and resourceType as @required; the check does not assert them, so whether the emulator populates them is unverified here.

Why: a client that relies on AWS's documented semantics gets two things wrong. First, it cannot rely on the revision guard the model advertises for responseRevisionId ("prevent submitting feedback on a stale response"): against the emulator the guard is only as strong as the caller's token hygiene, so duplicate/contradictory feedback can be recorded for one response version. Second, a retry with a matching token is documented to be ignored without an error, so a client whose only differentiator is the token sees conflict semantics attached to the wrong key and cannot tell "this revision already has feedback" (terminal, move on) from "my token collided" (retry with a new token).

Fix: the PutComplianceInquiryFeedback handler's conflict detection and the compliance-inquiry feedback state it consults — the uniqueness key must be the (complianceInquiryId, queryIdentifier, responseRevisionId) feedback record, with clientToken handled as idempotency replay on top of it, and the ConflictException constructed for that state condition (including its @required resourceId/resourceType).

Traces: artifact.put-compliance-inquiry-feedback-conflicting-payload-rejected.

## Evidence
Suite simparity-behavioral-aws-v1, service artifact: 19/22 checks passed (86%), 3 failed. Two of the three failures (artifact.get-report-download-roundtrip, artifact.get-report-term-token-cross-resource-rejected) are marked OWNED by the open issue 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and.md and are not re-filed here. The remaining one is this cause.

[mismatch] artifact.put-compliance-inquiry-feedback-conflicting-payload-rejected:
- CreateComplianceInquiry(name=<unique>, inquiryContent={'query': 'Does AWS provide a HIPAA compliant offering?'}, supportMode='AI_ONLY'), polled via GetComplianceInquiryMetadata to COMPLETED/FAILED, then ListComplianceInquiryQueries -> queries[0]; queryIdentifier=q, responseRevisionId=len(updatedResponseVersions or []).
- First call: PutComplianceInquiryFeedback(complianceInquiryId=..., queryIdentifier=q, rating='THUMBS_UP', responseRevisionId=r, clientToken=T) -> 200 with submittedAt (positive control passed).
- Second call: PutComplianceInquiryFeedback(same ids, rating='THUMBS_DOWN', responseRevisionId=r, clientToken=T) -> error code ConflictException (matched expectation).
- Divergence asserted: AssertionError: expected AWS's exact conflict message, got 'the clientToken was already used to submit different feedback on a compliance inquiry'. Recorded AWS message for this refusal: 'Feedback already submitted for this response version.' — the recorded text is cited as an example; the requirement being reported is the CONDITION it names (the one-feedback-per-(query, responseRevisionId) rule), not the literal string.
- Final positive control in the same check (fresh clientToken, rating='THUMBS_DOWN', different inquiry's untouched revision) was not reached because the assertion fired first.

Model citations used: PutComplianceInquiryFeedback (PUT /v1/compliance-inquiry/putFeedback, 200, @idempotent; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400); PutComplianceInquiryFeedbackRequest.responseRevisionId and .clientToken documentation quoted above; ConflictException @httpError 409 with @required message/resourceId/resourceType.

## Steps to Reproduce
1. Clone this checkout and run the behavioral suite simparity-behavioral-aws-v1 against the booted emulator (service artifact).
2. Replay artifact.put-compliance-inquiry-feedback-conflicting-payload-rejected: CreateComplianceInquiry with supportMode='AI_ONLY', poll GetComplianceInquiryMetadata until status is COMPLETED or FAILED, then take queries[0] from ListComplianceInquiryQueries and r = len(updatedResponseVersions or []).
3. Call PutComplianceInquiryFeedback(queryIdentifier=q, rating='THUMBS_UP', responseRevisionId=r, clientToken=T) and confirm 200 with submittedAt.
4. Call PutComplianceInquiryFeedback(queryIdentifier=q, rating='THUMBS_DOWN', responseRevisionId=r, clientToken=T). Observe HTTP 409 / ConflictException whose message attributes the refusal to clientToken reuse; AWS attributes it to the response version already having feedback.
5. Isolate the key: repeat step 4 with a FRESH clientToken against the same (queryIdentifier, responseRevisionId). AWS refuses this with 409 / ConflictException as well; a 200 here confirms the emulator's conflict is keyed on the token rather than on the revision's feedback state.
6. Confirm the non-regression: with a fresh clientToken, submit rating='THUMBS_DOWN' against a DIFFERENT inquiry's revision that has received no feedback — this must still return 200 with submittedAt.
7. Check that the emitted ConflictException body carries the modeled @required resourceId and resourceType members.
