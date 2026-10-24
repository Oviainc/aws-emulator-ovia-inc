Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# A file-upload compliance inquiry always ends FAILED with an internal-error message and owns zero queries, so FULL_SUPPORT / HUMAN_REVIEW are unreachable

_Filed: 2026-10-24T03:08:58Z_
_Operation: CreateComplianceInquiry | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateComplianceInquiry` models two content variants (`InquiryContent` = `query` | `fileContent`), and the file variant is a first-class, documented submission mode: `InquiryFileContent.content` is `@required` and `fileSections` is documented as the "List of file sections/sheets to process", while `CreateComplianceInquiryRequest$supportMode`'s `FULL_SUPPORT` ("includes human expert review") is documented as "Only supported for file upload mode".

Expected (AWS): a well-formed upload is accepted with 202/`PROCESSING` and then reaches a NON-failure terminal state — `COMPLETED` with the modeled clean-completion `statusMessage` ("Compliance inquiry processing is complete."), or `HUMAN_REVIEW` with "Human review is in progress." while the `FULL_SUPPORT` expert review is pending — and the inquiry owns one query per question/section extracted from the document, so `ListComplianceInquiryQueries` reports them (with `reviewType` `HUMAN` for a `FULL_SUPPORT` submission) and `PutComplianceInquiryFeedback` can name one via `queryIdentifier`. AWS does not report "An internal error occurred while processing the inquiry" for a valid upload.

Actual (emulator): `enumerate_queries` returns an EMPTY vector for `InquiryContentRecord::File`, and `ComplianceInquiry::materialise` turns an inquiry with no queries into `status = FAILED` / `statusMessage = "An internal error occurred while processing the inquiry. Try again at a later time."`. Consequences observable over HTTP:
- `GetComplianceInquiryMetadata` / `ListComplianceInquiries` report every file inquiry as FAILED with an internal-error message ~500ms after creation;
- `ListComplianceInquiryQueries` always returns an empty `queries` list for a file inquiry;
- `PutComplianceInquiryFeedback` with any `queryIdentifier` on a file inquiry is always refused with `ValidationException` (no query is ever owned);
- `ExportComplianceInquiry` renders a document covering no queries;
- `supportMode = FULL_SUPPORT` is persisted and fingerprinted but has NO observable effect anywhere: `InquiryStatus$HUMAN_REVIEW`, `InquiryStatusMessage$"Human review is in progress."` and `ReviewType$HUMAN` are unreachable in the whole emulator, and `fileSections` never shapes any query.

The fix does not require fabricating model-generated answer prose (the accepted precedent for the TEXT path in the already-fixed issue 20261022T085754Z was "let the inquiry complete cleanly without inventing answer prose"); it requires the file path to reach a non-FAILED terminal status and to own at least one query (e.g. one per submitted `fileSection`), with `reviewType`/status reflecting `supportMode`.

## Evidence

Spec: Smithy `InquiryContent` union member `fileContent` -> `InquiryFileContent{content @required (Blob), fileSections (FileSectionList, 0..30, "List of file sections/sheets to process")}`; `InquiryStatus` = PROCESSING|HUMAN_REVIEW|COMPLETED|FAILED; `InquiryStatusMessage` includes "Human review is in progress." and "Compliance inquiry processing is complete."; `ReviewType` = HUMAN|AI; `CreateComplianceInquiryRequest$supportMode` doc: "Support mode for inquiry processing. Only supported for file upload mode. Defaults to AI_ONLY if not specified."

Handler: services/service-com-amazonaws-artifact-artifact/src/ops/create_compliance_inquiry.rs — `enumerate_queries` returns `Vec::new()` for `InquiryContentRecord::File { .. }` (the arm is explicit: "it owns none"). src/state.rs `ComplianceInquiry::materialise` — `if self.queries.is_empty() { self.status = INQUIRY_STATUS_FAILED; self.status_message = INQUIRY_MESSAGE_INTERNAL_ERROR; }`. No code path ever sets `INQUIRY_STATUS` to HUMAN_REVIEW or a query `review_type` to HUMAN (grep for HUMAN in src/ finds only the constants).

Tests: services/service-com-amazonaws-artifact-artifact/tests/all/compliance_inquiry_lifecycle.rs lines ~301-339 ASSERT the divergent behavior (three FILE cases, including one with `FULL_SUPPORT` and two `fileSections`, all expecting `terminal_status: "FAILED"` / `INQUIRY_INTERNAL_ERROR`); compliance_inquiry_listing.rs lines ~264-286 assert the same. No test asserts the AWS behavior, so these fixtures must be updated with the fix. Precedent that the terminal-status/statusMessage enum selection is contract (not cosmetic): fixed issue 20261022T085754Z-artifact-compliance-inquiry-projection-returns-epoch-timestamps-and (cause 2, TEXT path).

## Steps to Reproduce

1. POST /v1/compliance-inquiry/create with body {"name":"Annual audit workbook","inquiryContent":{"fileContent":{"content":"<base64 bytes>","fileSections":["Section A","Section B"]}},"supportMode":"FULL_SUPPORT"} and note the 202 response's complianceInquirySummary.id (status PROCESSING, inputSource FILE).
2. Wait ~1s, then GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<id>: observe status FAILED and statusMessage "An internal error occurred while processing the inquiry. Try again at a later time."; expect a non-failure terminal state instead (HUMAN_REVIEW with "Human review is in progress." while expert review is pending, or COMPLETED with "Compliance inquiry processing is complete.").
3. GET /v1/compliance-inquiry/listQueries?complianceInquiryId=<id>: observe an empty `queries` list; expect at least one query extracted from the upload (one per submitted fileSection), with reviewType HUMAN for the FULL_SUPPORT submission.
4. PUT /v1/compliance-inquiry/putFeedback with {"complianceInquiryId":"<id>","queryIdentifier":1,"rating":"THUMBS_UP"}: observe a 400 ValidationException naming queryIdentifier, because the inquiry owns no query at all.
5. Repeat step 1 with a TEXT inquiry (inquiryContent.query) for contrast: it reaches COMPLETED and owns one query, showing the divergence is specific to the file-upload content mode.
