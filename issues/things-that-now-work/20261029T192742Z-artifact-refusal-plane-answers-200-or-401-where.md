Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-compliance-inquiry-feedback-and-export
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-compliance-inquiry-feedback-and-export artifact-report-term-and-download artifact-tag-resource-error-paths
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|PutComplianceInquiryFeedback|4xx->2xx|$.message
Issue-Key: artifact|PutComplianceInquiryFeedback|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: artifact|PutComplianceInquiryFeedback|4xx->2xx|status
Issue-Key: artifact|ExportComplianceInquiry|4xx->2xx|$.message
Issue-Key: artifact|ExportComplianceInquiry|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: artifact|ExportComplianceInquiry|4xx->2xx|status
Issue-Key: artifact|GetReport|4xx->2xx|$.message
Issue-Key: artifact|GetReport|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: artifact|GetReport|4xx->2xx|status
Issue-Key: artifact|ListTagsForResource|4xx->4xx|$.message
Issue-Key: artifact|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: artifact|ListTagsForResource|4xx->4xx|status
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# artifact refusal plane answers 200 or 401 where AWS refuses with 400 ValidationException

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutComplianceInquiryFeedback

## Description
Four net-new root causes in the artifact REFUSAL plane: three requests AWS refuses with 400 ValidationException are accepted and answered 200, and a fourth is refused with the front door's 401 UnauthorizedException instead of the modeled 400 ValidationException. Each cause is independent and must be closed separately.

Cause 1 of 4 — PutComplianceInquiryFeedback accepts question-level feedback that names no response revision (fixture artifact-compliance-inquiry-feedback-and-export, step task#2).
AWS: PutComplianceInquiryFeedback (Smithy: restJson1 PUT /v1/compliance-inquiry/putFeedback; input complianceInquiryId @required, rating @required, queryIdentifier, responseRevisionId — documented "The response revision ID. Use this value to prevent submitting feedback on a stale response."; modeled error ValidationException httpError 400 with @required message and @required reason: ValidationExceptionReason one of unknownOperation|cannotParse|fieldValidationFailed|invalidToken|other). Feedback scoped to a single question must carry the response revision it is feedback ON; a question-level submission without it is refused 400 ValidationException with the fixed message 'expectedResponseRevisionId is required for question-level feedback submission.' — that whole sentence is fixed text, it interpolates nothing from the request.
Emulator: HTTP 200 with a PutComplianceInquiryFeedbackResponse (submittedAt) — no refusal at all: $.message missing (expected 'expectedResponseRevisionId is required for question-level feedback submission.') and no x-amzn-errortype header. The gate is absent, not mis-worded.
Why: a client omitting the revision id believes its feedback was recorded against the response it was reading; the emulator silently attaches it to whatever revision is current, which is exactly the stale-response attribution the member exists to prevent, and no client ever sees the validation error it must handle.
Fix: PutComplianceInquiryFeedback's input validation — require responseRevisionId whenever the submission is question-level (queryIdentifier present) and emit ValidationException (reason fieldValidationFailed) with that message.
Traces: artifact-compliance-inquiry-feedback-and-export#task#2 (representative).

Cause 2 of 4 — ExportComplianceInquiry exports a resource still in PROCESSING instead of refusing (fixture artifact-compliance-inquiry-feedback-and-export, step task#3).
AWS: ExportComplianceInquiry (Smithy: restJson1 POST /v1/compliance-inquiry/export; input complianceInquiryId @required, queryIdentifiers, includeCitations; modeled ValidationException httpError 400). An export is only servable once the thing being exported has finished processing — QuerySummary.status is an enum of PROCESSING|COMPLETED|FAILED — so an export naming a resource whose status is still PROCESSING is refused 400 ValidationException with the fixed message 'Unable to export resource in a `PROCESSING` state. Please wait for processing to complete.' (fixed text throughout, including the backquoted state name).
Emulator: HTTP 200 with an ExportComplianceInquiryResponse — $.message missing (expected that sentence) and no x-amzn-errortype header. The processing-state precondition is never evaluated.
Why: the caller receives a documentPresignedUrl for an export built from an unfinished inquiry, so it downloads a partial or empty document and treats it as the final compliance artifact; the retry-when-processing contract the error expresses is unavailable to it.
Fix: the ExportComplianceInquiry handler's precondition check against the inquiry/query status held in compliance-inquiry state, ahead of minting the presigned URL.
Traces: artifact-compliance-inquiry-feedback-and-export#task#3.

Cause 3 of 4 — GetReport accepts an invalid termToken (fixture artifact-report-term-and-download, step task#4).
AWS: GetReport (Smithy: restJson1 GET /v1/report/get; input reportId @required @pattern ^report-[a-zA-Z0-9]{16}$, termToken @required @length 1..256 @pattern ^[a-zA-Z0-9_\-\s]*$, reportVersion; modeled ValidationException httpError 400). A termToken that is not a receipt this service issued for the report being downloaded is refused 400 ValidationException, x-amzn-errortype: ValidationException, with the fixed message 'The provided token is invalid.' — fixed text, interpolating neither the token nor the report id (ValidationException also requires reason, which for a bad token is the modeled invalidToken value).
Emulator: HTTP 200 with a GetReportResponse (documentPresignedUrl) — $.message missing (expected 'The provided token is invalid.'), x-amzn-errortype missing (expected 'ValidationException'). Any token value is honoured as a valid term grant.
Why: the term grant is the service's record that the caller accepted the report's terms; honouring an unissued token lets a client download a term-gated report without ever accepting its terms, and a client that must handle an expired/invalid receipt by re-accepting the term never gets the signal to do so.
Fix: GetReport's termToken validation — verify the token against the term grants held in state and refuse with ValidationException (reason invalidToken) before serving a presigned URL.
Traces: artifact-report-term-and-download#task#4.

Cause 4 of 4 — a malformed resourceArn on ListTagsForResource is answered 401 UnauthorizedException instead of 400 ValidationException (fixture artifact-tag-resource-error-paths, step task#1).
AWS: ListTagsForResource (Smithy: restJson1 GET /tags/{resourceArn}; input resourceArn @httpLabel @required @length 1..2048 @pattern ^[^<>]*$; modeled errors AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400, ThrottlingException 429, InternalServerException 500 — there is no 401 error in this operation's closure or in the service's common errors). A resourceArn that is not a syntactically valid ARN is a request-shape failure: 400 ValidationException whose message is the fixed text 'Invalid input resource arn: ' followed by the reason the parse failed ('bad syntax' in the recording) — the fixed clause plus a parse-failure reason, not the caller's ARN echoed back.
Emulator: HTTP 401 with x-amzn-errortype: UnauthorizedException and message 'Unauthorized'. The tag plane runs its front-door authorization decision on an ARN it could not parse and reports the unparseable ARN as an authorization failure, so a syntax error is reported in an error class this operation does not model and at a status the restJson1 surface never uses here.
Why: the status class is wrong (401 vs 400) and the error code is not in the operation's closure, so a client's modeled-error dispatch has no branch for it; an SDK retry/credential-refresh path keyed on 401 will re-sign and retry a request that can never succeed, and the caller is told its credentials are bad rather than that its ARN is malformed.
Fix: ordering and classification in the tagging handlers' input validation — validate the resourceArn httpLabel's ARN syntax and refuse with ValidationException before the authorization gate resolves the target.
Traces: artifact-tag-resource-error-paths#task#1.

Note on neighbouring open issues: none of these four are covered by the open artifact issues. 20261029T055020Z-artifact-putcomplianceinquiryfeedback-error-response concerns a clientToken replay being answered 409 ConflictException (a different input and a different error class from Cause 1's missing-revision gate), and 20261028T182407Z-artifact-gettermforreport-wrong-behavior concerns how the download receipt is minted on GetTermForReport, not GetReport's failure to validate a receipt it never issued (Cause 3).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 70.0%, 9 of 30 steps mismatched).

Cause 1 — fixture artifact-compliance-inquiry-feedback-and-export step task#2, PutComplianceInquiryFeedback:
  status: expected 400, got 200
  $.message: missing (expected 'expectedResponseRevisionId is required for question-level feedback submission.', got None)
  $headers.x-amzn-errortype: missing (expected a shaped error type, got None)

Cause 2 — fixture artifact-compliance-inquiry-feedback-and-export step task#3, ExportComplianceInquiry:
  status: expected 400, got 200
  $.message: missing (expected 'Unable to export resource in a `PROCESSING` state. Please wait for processing to complete.', got None)
  $headers.x-amzn-errortype: missing (expected a shaped error type, got None)

Cause 3 — fixture artifact-report-term-and-download step task#4, GetReport:
  status: expected 400, got 200
  $.message: missing (expected 'The provided token is invalid.', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Cause 4 — fixture artifact-tag-resource-error-paths step task#1, ListTagsForResource:
  status: expected 400, got 401
  $.message: value_mismatch (expected 'Invalid input resource arn: bad syntax', got 'Unauthorized')
  $headers.x-amzn-errortype: shape_mismatch (expected a shaped error type, got 'UnauthorizedException')

Smithy (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json):
  PutComplianceInquiryFeedback PUT /v1/compliance-inquiry/putFeedback; responseRevisionId doc 'The response revision ID. Use this value to prevent submitting feedback on a stale response.'; error ValidationException httpError 400 {message @required, reason @required: ValidationExceptionReason [unknownOperation|cannotParse|fieldValidationFailed|invalidToken|other], fieldList}
  ExportComplianceInquiry POST /v1/compliance-inquiry/export; QueryStatus enum [PROCESSING|COMPLETED|FAILED]; error ValidationException 400
  GetReport GET /v1/report/get; termToken @required @length 1..256 @pattern ^[a-zA-Z0-9_\-\s]*$; error ValidationException 400
  ListTagsForResource GET /tags/{resourceArn}; resourceArn @httpLabel @required; errors AccessDenied 403, ResourceNotFound 404, Validation 400, Throttling 429, InternalServer 500 — no 401/UnauthorizedException anywhere in the closure or in common_errors [AccessDeniedException, InternalServerException, ThrottlingException, ValidationException].

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: run fixture artifact-compliance-inquiry-feedback-and-export to step task#2 — PUT /v1/compliance-inquiry/putFeedback with complianceInquiryId, a queryIdentifier and a rating but NO responseRevisionId. Expect 400 ValidationException with message 'expectedResponseRevisionId is required for question-level feedback submission.'; the emulator answers 200 with submittedAt.
3. Cause 2: same fixture, step task#3 — POST /v1/compliance-inquiry/export naming a resource whose status is still PROCESSING. Expect 400 ValidationException with message 'Unable to export resource in a `PROCESSING` state. Please wait for processing to complete.'; the emulator answers 200 with a documentPresignedUrl.
4. Cause 3: run fixture artifact-report-term-and-download to step task#4 — GET /v1/report/get?reportId=<reportId>&termToken=<a token this service never issued>. Expect 400, x-amzn-errortype: ValidationException, message 'The provided token is invalid.'; the emulator answers 200 with a documentPresignedUrl.
5. Cause 4: run fixture artifact-tag-resource-error-paths step task#1 — GET /tags/{resourceArn} with a resourceArn that is not a syntactically valid ARN. Expect 400 ValidationException 'Invalid input resource arn: bad syntax'; the emulator answers 401 UnauthorizedException 'Unauthorized'.
