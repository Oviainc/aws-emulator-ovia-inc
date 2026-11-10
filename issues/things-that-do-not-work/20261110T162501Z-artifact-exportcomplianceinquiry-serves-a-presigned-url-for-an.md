Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: artifact.export-compliance-inquiry-refused-while-processing
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact.export-compliance-inquiry-refused-while-processing|run#5|AssertionError|assert still != "PROCESSING", "ExportComplianceInquiry on an inquiry that stayed PROCESSING was accepted"
Issue-Commit: e88affa0c81a58fb8423ccb328f6804dccd04335

# artifact ExportComplianceInquiry serves a presigned URL for an inquiry still in PROCESSING instead of refusing 400 ValidationException (1 cause)

> _Note: this issue was identified against the code base at commit `e88affa0c81a58fb8423ccb328f6804dccd04335`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportComplianceInquiry

## Description
Behavioral replay of the artifact state-machine group found one net-new root cause.

Cause 1 of 1: ExportComplianceInquiry does not evaluate the processing-state precondition of the inquiry it is asked to export.

AWS: ExportComplianceInquiry (Smithy, services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json: restJson1 POST /v1/compliance-inquiry/export, success code 200; input complianceInquiryId @required @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$, queryIdentifiers, includeCitations; output documentPresignedUrl (@sensitive) + tags; modeled errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). A compliance inquiry is a resource with a lifecycle: InquiryDetail.status is @required and targets the enum InquiryStatus = PROCESSING | HUMAN_REVIEW | COMPLETED | FAILED, and CreateComplianceInquiry is modeled with HTTP 202 (accepted, not complete), so a freshly created inquiry is observable in PROCESSING before it settles. An export can only be served from a finished inquiry, so an export naming an inquiry whose status is still PROCESSING is refused with HTTP 400 ValidationException, header x-amzn-errortype: ValidationException, body members message and reason (both @required; reason drawn from ValidationExceptionReason, here the modeled 'other' class for a state precondition rather than a field failure) and the message 'Unable to export resource in a `PROCESSING` state. Please wait for processing to complete.' — that sentence is entirely FIXED text for this error class, including the backquoted state name; it interpolates nothing from the request or from state (no inquiry id, no name, no timestamp). Once the same inquiry reaches COMPLETED the identical call succeeds 200 with a documentPresignedUrl (an https URL).

Emulator: the check creates an inquiry, confirms via GetComplianceInquiryMetadata that complianceInquiryDetail.status is still 'PROCESSING', and calls ExportComplianceInquiry on it. The call is ACCEPTED — no ClientError is raised, the emulator answers a success response with documentPresignedUrl — and a re-read of the inquiry shows it is STILL 'PROCESSING', i.e. the acceptance is not explained by the inquiry having settled between the two reads. The assertion that fires is `assert still != "PROCESSING"` -> AssertionError: 'ExportComplianceInquiry on an inquiry that stayed PROCESSING was accepted'. No 400, no ValidationException, no message, no x-amzn-errortype header. The precondition is absent, not mis-worded.

Why: a client receives a documentPresignedUrl minted from an inquiry that has not finished processing, downloads a partial or empty document, and stores it as the final compliance artifact for an audit — a silent correctness failure with no signal at all. The retry-when-processing contract that this ValidationException expresses is unavailable: a client written against AWS polls on the refusal (or on status) and only exports once COMPLETED, while against the emulator it exports immediately and never learns the export was premature. Error-handling branches keyed on ValidationException for this operation are dead code and never exercised.

Fix: the ExportComplianceInquiry handler's precondition check — read the target inquiry's status from compliance-inquiry state and, when it is PROCESSING, return the modeled ValidationException (400, x-amzn-errortype: ValidationException, required message + reason) BEFORE minting the presigned URL; only serve the URL for a settled (COMPLETED) inquiry.

Traces: behavioral check artifact.export-compliance-inquiry-refused-while-processing (group state-machine). Note on a neighbouring open issue: the open wire issue 20261029T192742Z-artifact-refusal-plane-answers-200-or-401-where.md describes the same emulator gap as its 'Cause 2 of 4', grounded in the WIRE fixture artifact-compliance-inquiry-feedback-and-export step task#3 and keyed on artifact|ExportComplianceInquiry|4xx->2xx|status. It does not carry this behavioral check's replay key, so this issue records the behavioral grounding; a fixer closing one should re-replay both.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the emulator: 22/23 passed, 1 mismatched — all in service 'artifact'.

[mismatch] artifact.export-compliance-inquiry-refused-while-processing
  check_artifact_export_compliance_inquiry_refused_while_processing.py line 51:
    assert still != "PROCESSING", "ExportComplianceInquiry on an inquiry that stayed PROCESSING was accepted"
  AssertionError: ExportComplianceInquiry on an inquiry that stayed PROCESSING was accepted

Sequence the check performs, and what each step returned:
  1. CreateComplianceInquiry(name=<unique>, inquiryContent={query: ...}, supportMode='AI_ONLY') -> complianceInquirySummary.id
  2. GetComplianceInquiryMetadata(complianceInquiryId=<id>) -> complianceInquiryDetail.status == 'PROCESSING'
  3. ExportComplianceInquiry(complianceInquiryId=<id>)
       expected: HTTP 400, x-amzn-errortype: ValidationException,
                 Error.Code 'ValidationException',
                 Error.Message 'Unable to export resource in a `PROCESSING` state. Please wait for processing to complete.'
                 (fixed text in full; no request- or state-derived parts)
       actual:   accepted — a success response carrying documentPresignedUrl, no ClientError raised
  4. GetComplianceInquiryMetadata(complianceInquiryId=<id>) -> status STILL 'PROCESSING'
     (so the acceptance is not a race in which the inquiry settled between steps 2 and 3)

Model facts used (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json):
  ExportComplianceInquiry: POST /v1/compliance-inquiry/export, http code 200; errors include ValidationException httpError 400
  ValidationException: {message @required, reason @required -> ValidationExceptionReason [unknownOperation|cannotParse|fieldValidationFailed|invalidToken|other], fieldList}
  InquiryDetail.status @required -> InquiryStatus enum [PROCESSING, HUMAN_REVIEW, COMPLETED, FAILED]
  CreateComplianceInquiry: POST /v1/compliance-inquiry/create, http code 202 (asynchronous settle)

Positive control (the part of the check never reached): after the inquiry settles to COMPLETED, the identical ExportComplianceInquiry call must return a documentPresignedUrl beginning 'https://'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1, check artifact.export-compliance-inquiry-refused-while-processing.
2. Or by hand, against the emulator's artifact endpoint:
   a. CreateComplianceInquiry with a unique name, inquiryContent={"query": "Does AWS provide a HIPAA compliant environment?"} and supportMode=AI_ONLY; keep complianceInquirySummary.id.
   b. GetComplianceInquiryMetadata(complianceInquiryId=<id>) and confirm complianceInquiryDetail.status == "PROCESSING".
   c. While it is still PROCESSING, call ExportComplianceInquiry(complianceInquiryId=<id>).
      Expected: HTTP 400 with x-amzn-errortype: ValidationException and message 'Unable to export resource in a `PROCESSING` state. Please wait for processing to complete.'
      Observed: a 200 success response carrying documentPresignedUrl.
   d. Re-read GetComplianceInquiryMetadata: the status is still "PROCESSING", confirming the export was served from an unfinished inquiry.
3. Positive control that must keep working after the fix: poll GetComplianceInquiryMetadata until status == "COMPLETED", then call the identical ExportComplianceInquiry — it must return documentPresignedUrl starting with "https://".
