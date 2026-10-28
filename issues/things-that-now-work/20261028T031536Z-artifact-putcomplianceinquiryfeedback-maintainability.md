Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# Two test suites still exclude now-implemented inquiry operations because their comments claim those operations answer a scaffolded 501

_Filed: 2026-10-28T03:15:36Z_
_Operation: PutComplianceInquiryFeedback | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this crate is implemented (no `//! SCAFFOLDED, NOT IMPLEMENTED:` header and no `Scaffolded::not_implemented(..)` call exists anywhere under `src/ops/`), but two test suites still state as their reason for NOT exercising an operation that it "answer[s] its own scaffolded 501". Each suite declares a rule it means to generalise over every implemented operation, and then skips rows for a reason that is no longer true:

1. `tests/all/timestamp_wire_form.rs` declares that rule 1 "is checked end to end over HTTP across every implemented operation that reports a timestamp", and excuses `PutComplianceInquiryFeedbackResponse$submittedAt` from that end-to-end check by calling it one of "the shapes behind a scaffolded 501". `PutComplianceInquiryFeedback` is implemented and answers `submittedAt` on every success, so the RFC 3339 wire form of the one timestamp that operation reports is asserted only against the member schema, never over HTTP, purely because of the stale claim.

2. `tests/all/front_door_errors.rs` asserts that a well-formed `complianceInquiryId` the account holds no inquiry for is refused with the front door's 401, and then restricts the row loop to `GetComplianceInquiryMetadata` because "the other three inquiry operations still answer their scaffolded 501 (their own stub issues)". All three — `ExportComplianceInquiry`, `ListComplianceInquiryQueries`, `PutComplianceInquiryFeedback` — are implemented and each makes exactly the state-dependent judgement that suite is about (`return Err(crate::scaffolded::Scaffolded::unauthorized())`), so the rule the suite states is left unverified for three of the four operations it covers.

The defect is the stale reason, not the behaviour: the handlers do the right thing. Nothing in a response changes; the remedy is to delete the two "scaffolded 501" claims and add the rows the false reason excluded (the `submittedAt` wire form over HTTP, and the unheld-id 401 for the three inquiry operations).

## Evidence

Stale claim 1 — services/service-com-amazonaws-artifact-artifact/tests/all/timestamp_wire_form.rs:28-33:
```
//! Rule 1 is checked end to end over HTTP across every implemented operation that reports
//! a timestamp (so the rows differ along operation, shape, member, whether the member is
//! `@required`, ... ) and directly on
//! the member schemas, which is the only reach the shapes behind a scaffolded 501
//! (`ResponseVersion`, `PutComplianceInquiryFeedbackResponse$submittedAt`) have.
```
and the same file at :374-377: "This is the only reach the shapes behind a scaffolded 501 have — `ResponseVersion` and `PutComplianceInquiryFeedbackResponse$submittedAt`, which only `PutComplianceInquiryFeedback` and an edited response would report".

Stale claim 2 — services/service-com-amazonaws-artifact-artifact/tests/all/front_door_errors.rs:311-317:
```
    // The other class of unauthorizable target: a well-formed id the account holds no
    // inquiry for. That judgement needs this service's state, so the HANDLER makes it and
    // answers the same refusal — exactly as an unpublished `reportId` is answered above.
    // Only `GetComplianceInquiryMetadata` is implemented today; the other three inquiry
    // operations still answer their scaffolded 501 (their own stub issues), which is why
    // they are not rows here.
    for (case, id) in UNHELD_INQUIRY_IDS {
```

The code both claims are about — implemented, not 501:
- services/service-com-amazonaws-artifact-artifact/src/ops/put_compliance_inquiry_feedback.rs:370-374 returns the timestamp over HTTP: `Ok(PutComplianceInquiryFeedbackOutput { submitted_at: ::aws_smithy_types::DateTime::from(now) })`, and :233-239 makes the state-dependent refusal: `let Some(inquiry) = store.inquiries.get_mut(&input.compliance_inquiry_id) else { ... return Err(crate::scaffolded::Scaffolded::unauthorized()); };`
- services/service-com-amazonaws-artifact-artifact/src/ops/export_compliance_inquiry.rs:189-197 — same refusal: `return Err(crate::scaffolded::Scaffolded::unauthorized());`
- services/service-com-amazonaws-artifact-artifact/src/ops/list_compliance_inquiry_queries.rs:201-208 — same refusal.
- No scaffold marker remains in the crate: grep for `SCAFFOLDED, NOT IMPLEMENTED` / `not_implemented(` over `src/ops/` returns nothing, and `src/ops/SKIPPED_STUBS.md` does not exist.

Coverage today: `tests/all/compliance_inquiry_feedback.rs` exercises `PutComplianceInquiryFeedback`, and `tests/all/timestamp_wire_form.rs:379` (`every_modelled_timestamp_member_carries_the_date_time_format`) pins `PutComplianceInquiryFeedbackOutput::M_SUBMITTED_AT` at the schema level only — no test asserts `submittedAt` is an RFC 3339 string in a real 200 body, and no test asserts the 401 for an unheld id on the three inquiry operations.

## Steps to Reproduce

1. Grep the crate for a scaffold marker: `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED\|not_implemented(' services/service-com-amazonaws-artifact-artifact/src/ops/` — no match, so no operation answers 501.
2. Read services/service-com-amazonaws-artifact-artifact/tests/all/front_door_errors.rs:311-317 and note the loop is limited to `GetComplianceInquiryMetadata` because "the other three inquiry operations still answer their scaffolded 501".
3. Read services/service-com-amazonaws-artifact-artifact/src/ops/export_compliance_inquiry.rs:196, list_compliance_inquiry_queries.rs:207 and put_compliance_inquiry_feedback.rs:238 — each already returns `crate::scaffolded::Scaffolded::unauthorized()` for a well-formed id the store holds no inquiry for, which is exactly the row the suite skipped.
4. Read services/service-com-amazonaws-artifact-artifact/tests/all/timestamp_wire_form.rs:28-33 and :374-377 and note `PutComplianceInquiryFeedbackResponse$submittedAt` is excused from the end-to-end HTTP rows as a shape "behind a scaffolded 501".
5. PUT /v1/compliance-inquiry/putFeedback with {"complianceInquiryId":"<id of a created inquiry>","rating":"THUMBS_UP"} and observe a 200 carrying `submittedAt` — an implemented operation reporting a timestamp that the suite's own stated rule should have covered end to end.
