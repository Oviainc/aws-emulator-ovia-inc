Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# supportMode=FULL_SUPPORT is accepted and echoed for a TEXT (single-query) inquiry, which the model says only file upload supports

_Filed: 2026-10-24T18:42:40Z_
_Operation: CreateComplianceInquiry | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateComplianceInquiryRequest$supportMode` is documented as "Support mode for inquiry processing. Only supported for file upload mode. Defaults to AI_ONLY if not specified." The emulator applies only the second sentence (the AI_ONLY default) and ignores the first: a submission whose `inquiryContent` is the `query` variant (TEXT) is accepted with `supportMode = FULL_SUPPORT`, the value is persisted verbatim on the inquiry record, and every later read reports it.

Expected (AWS): a TEXT submission carrying `supportMode = FULL_SUPPORT` is refused with the modelled 400 `ValidationException` (reason `fieldValidationFailed`, fieldList naming `supportMode`), because the member is only supported for file upload mode — or, if the service tolerates it, the inquiry is normalised so no read advertises a support mode the inquiry was not processed under.

Actual (emulator): the create answers 202, `GetComplianceInquiryMetadata` reports `complianceInquiryDetail.supportMode = "FULL_SUPPORT"`, while the one enumerated query is given `reviewType = "AI"` and the inquiry completes without any human-review stage. The response therefore claims a human-expert support mode for an inquiry that was AI-reviewed, and the member has no effect other than being echoed back.

This is distinct from the open issue 20261024T030858Z (CreateComplianceInquiry / wrong-behavior), which concerns the FILE path never producing queries and `HUMAN_REVIEW`/`ReviewType$HUMAN` being unreachable. This issue is about the TEXT path accepting and echoing a mode the model restricts to file upload.

## Evidence

Spec: Smithy `CreateComplianceInquiryRequest$supportMode` (target `InquirySupportMode` = AI_ONLY | FULL_SUPPORT) doc — "Support mode for inquiry processing. Only supported for file upload mode. Defaults to AI_ONLY if not specified."; `InquiryDetail$supportMode` — "Support mode for this inquiry. AI_ONLY provides AI-generated responses. FULL_SUPPORT includes human expert review."; `InquiryContent` union variants `query` (TEXT) vs `fileContent` (FILE).

Handler: services/service-com-amazonaws-artifact-artifact/src/ops/create_compliance_inquiry.rs — `let support_mode = input.support_mode.unwrap_or_else(|| crate::state::SUPPORT_MODE_AI_ONLY.to_string());` applies only the default and is stored unconditionally on `crate::state::ComplianceInquiry.support_mode`, with no check against `content`'s variant; `enumerate_queries` hard-codes `review_type: Some(crate::state::REVIEW_TYPE_AI)` for text content with the comment "`supportMode`'s `FULL_SUPPORT` is 'only supported for file upload mode', so a query read off submitted text is AI-reviewed whatever the request asked for" — i.e. the handler knows the rule but neither refuses the request nor normalises the reported value. src/state.rs `ComplianceInquiry::to_detail` returns `support_mode` verbatim.

Tests: tests/all/compliance_inquiry_name_projection.rs passes `supportMode` on TEXT rows (`AI_ONLY` only) and never asserts anything about a FULL_SUPPORT text submission; tests/all/compliance_inquiry_lifecycle.rs uses FULL_SUPPORT only on FILE rows. No test asserts a refusal or a normalisation for TEXT + FULL_SUPPORT, and no existing issue (open or fixed) covers it.

## Steps to Reproduce

1. POST /v1/compliance-inquiry/create signed for the artifact service with body {"name":"soc2","inquiryContent":{"query":"Does the SOC 2 report cover eu-west-1?"},"supportMode":"FULL_SUPPORT"}.
2. Observe 202 with a complianceInquirySummary; expect instead a 400 ValidationException stating that supportMode is only supported for file upload mode (or, at minimum, an inquiry that does not report FULL_SUPPORT).
3. GET /v1/compliance-inquiry/getMetadata?complianceInquiryId=<id> and observe complianceInquiryDetail.supportMode == "FULL_SUPPORT".
4. GET /v1/compliance-inquiry/listQueries?complianceInquiryId=<id> and observe the single query's reviewType == "AI" — the reported support mode and the actual review type disagree.
5. For contrast, repeat step 1 with "supportMode":"AI_ONLY": the response is indistinguishable apart from the echoed member, showing FULL_SUPPORT has no effect on the TEXT path.
