Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected|run#16|AssertionError|assert _name_in_list(art, query_text), (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Artifact: a TEXT/query compliance inquiry reports the caller-supplied title as InquirySummary.name instead of the query content, so it is not findable by query text in ListComplianceInquiries

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComplianceInquiry

## Description
ONE cause in the artifact emulator (sdkId Artifact, restJson1, sigv4), grounded in a behavioral check of suite simparity-behavioral-aws-v1.

Cause 1 of 1 — for a TEXT (query) compliance inquiry, the emulator projects the caller-supplied `name` (title) into `InquirySummary.name`, where AWS projects the inquiry's QUERY CONTENT, so an inquiry created from a query is never visible under that query text in ListComplianceInquiries / GetComplianceInquiryMetadata.

AWS: recorded behavior only for the projection rule. The model says only that the member exists and is a title — Smithy: CreateComplianceInquiryRequest.name : InquiryName @required @sensitive (doc 'Title of the inquiry.'); CreateComplianceInquiryRequest.inquiryContent : InquiryContent @required @sensitive, a union whose `query` member is LongStringAttribute @length 1..2048 @pattern ^[^<>]*$ (doc 'Single text query for AI-generated answer.'); InquirySummary.name : String @required (doc 'Title of the inquiry.'); InquirySummary.inputSource : InputSource @required; operation POST /v1/compliance-inquiry/create, success 202; ListComplianceInquiries GET /v1/compliance-inquiry/list, 200, paginated items=complianceInquiries. The recording shows that when the inquiry is created from `inquiryContent.query` (inputSource TEXT), the service echoes the QUERY CONTENT back as `name` in list and get responses rather than persisting the submitted title verbatim — i.e. after a successful create of query 'What is AWS Artifact? [<marker>]', an entry whose `name` equals that exact query string appears in ListComplianceInquiries. Hypothesis: AWS derives the display name of a single-query inquiry from the query itself and only honours a caller-supplied title for file-upload inquiries.

Emulator: CreateComplianceInquiry succeeds (202) and returns a complianceInquirySummary with a well-formed id (compliance-inquiry-…) and arn, and ListComplianceInquiries does return the record — but with `name` set to the request's `name` member (the title), never to the query text. Consequently a listing scan for the query content finds nothing and the check fails with `AssertionError: inquiry with query content '…' not visible in list after a successful create`. The same record is created and listed under a different name, so this is a projection divergence, not a lost write.

Why: a client that creates a single-query inquiry and then locates it in the listing by the text it submitted (the only content-derived handle the summary exposes — the summary carries no query text and no content field) can never match its own inquiry, so poll/correlate flows after a 202 create silently find nothing and either re-create duplicates or report the create as lost. Any UI or report keyed on the AWS-visible display name shows a different string from AWS for the same resource.

Fix: the CreateComplianceInquiry handler's record construction and the state's summary projection — the `name` written for a TEXT-content inquiry, and `ComplianceInquiry::to_summary()`, which both ListComplianceInquiries and GetComplianceInquiryMetadata read, must agree with AWS's derived-name rule for inputSource TEXT while leaving the file-upload case on the submitted title.

Traces: artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected (the check's positive control, after its clientToken-rejection assertions now pass). Code sites: services/service-com-amazonaws-artifact-artifact/src/ops/create_compliance_inquiry.rs:238 (`name: input.name`) and services/service-com-amazonaws-artifact-artifact/src/ops/list_compliance_inquiries.rs:214 (`inquiry.to_summary()`).

Related already-open issue (does NOT cover this): 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and.md lists this same check id, but for its cause 3 only (CreateComplianceInquiry answering ValidationException instead of BadRequestException for a malformed clientToken). That assertion now PASSES — the replay reaches the positive control and fails on the listing projection instead — so the cause filed here is net-new and its replay key is not carried by that issue.

## Evidence
Suite simparity-behavioral-aws-v1, service artifact: 17/22 checks passed (77%), 5 failed. Of those, artifact.get-report-download-roundtrip and artifact.get-report-term-token-cross-resource-rejected are marked OWNED by 20261022T012757Z-artifact-report-and-compliance-inquiry-error-paths-and.md, and artifact.put-compliance-inquiry-feedback-conflicting-payload-rejected (plus the other check blocked by the same stub) is marked ALREADY FILED against the unimplemented PutComplianceInquiryFeedback scaffold. The failure below is the only one not already filed or owned.

[mismatch] artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected
  check_artifact_create_compliance_inquiry_invalid_client_token_pattern_rejected.py, line 66, in run
    assert _name_in_list(art, query_text)
  AssertionError: inquiry with query content 'What is AWS Artifact? [sim2real-artifact-badtoken-e0c70ec3fdeb]' not visible in list after a successful create

Sequence actually observed:
1. CreateComplianceInquiry(name='sim2real-artifact-badtoken-<id>', inquiryContent={'query': 'What is AWS Artifact? [sim2real-artifact-badtoken-<id>]'}, supportMode='AI_ONLY', clientToken='not a valid token!') was refused with HTTP 400 / BadRequestException and message 'Invalid request body' — these assertions PASSED (so the previously filed error-code cause is no longer reproducing on this path).
2. The listing scan confirming nothing was created under that query content PASSED.
3. The positive control CreateComplianceInquiry with the same name/query and a VALID clientToken SUCCEEDED: complianceInquirySummary.id started with 'compliance-inquiry-' and arn with 'arn:'.
4. The final assertion failed: paging ListComplianceInquiries(maxResults=300) over up to 900 entries found no item whose `name` equals the submitted query string. Expected AWS behavior: for a TEXT/query inquiry the summary's `name` IS the query content, so that entry is present.

Emulator code read for the diagnosis: create_compliance_inquiry.rs stores `name: input.name` on the ComplianceInquiry record (line 238) and answers from `inquiry.to_summary()`; list_compliance_inquiries.rs serves `inquiry.to_summary()` for every stored inquiry (line 214) — the record is present in the store, only its `name` projection differs.

Model citations used: CreateComplianceInquiryRequest.{name : InquiryName @required @sensitive, inquiryContent : InquiryContent @required @sensitive (union: query LongStringAttribute @length 1..2048 @pattern ^[^<>]*$ | fileContent), clientToken @idempotencyToken @length 1..128 @pattern ^[\w\-]+$, supportMode InquirySupportMode}; CreateComplianceInquiry http 202 POST /v1/compliance-inquiry/create; InquirySummary.{arn, name, id @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$, status, statusMessage, inputSource, createdAt} all @required; ListComplianceInquiries http 200 GET /v1/compliance-inquiry/list, paginated items=complianceInquiries, maxResults @range 1..300.

## Steps to Reproduce
1. Clone this checkout and run the behavioral suite simparity-behavioral-aws-v1 against the booted emulator (service artifact).
2. Replay artifact.create-compliance-inquiry-invalid-client-token-pattern-rejected.
3. Directly: call CreateComplianceInquiry(name='<title>', inquiryContent={'query': 'What is AWS Artifact? [<unique-marker>]'}, supportMode='AI_ONLY', clientToken='<valid-token>') and confirm it returns 202 with complianceInquirySummary.id matching ^compliance-inquiry-[a-zA-Z0-9]{16}$.
4. Page ListComplianceInquiries(maxResults=300), following nextToken, and look for an entry whose `name` equals the exact query string submitted in step 3. AWS has one; the emulator instead has an entry whose `name` equals '<title>'.
5. Confirm the same projection on GetComplianceInquiryMetadata for that inquiry id: the detail's name must follow the same rule as the listing.
6. Confirm the file-upload path is unaffected: create an inquiry with inquiryContent.fileContent and check its `name` projection separately.
