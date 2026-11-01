Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# The inquiry mutators and the term-grant mint leave no attributable record, breaking the attribution discipline this crate's other state changes state

_Filed: 2026-10-29T09:33:17Z_
_Operation: PutComplianceInquiryFeedback | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for a state-changing call: it leaves exactly one structured record on the `emulator::audit` target naming the caller (the access-key id the request presented and its credential-scope region) and the outcome — "a state-changing call that changed nothing is still attributable". `PutAccountSettings`, `TagResource` and `UntagResource` state that rule and apply it, reading the akid and the region specifically in order to attribute the change.

Four other handlers in the same crate are mutators too and skip it entirely. `PutComplianceInquiryFeedback` persists a feedback record on the inquiry (`inquiry.record_feedback(..)`) and remembers the request's `clientToken`, then logs on tracing's DEFAULT target with no `akid` and no `region`. The identical omission is in `CreateComplianceInquiry` (which mints and persists the resource), `ExportComplianceInquiry` (which mints a persisted `InquiryExport` plus the download token that authorises its document) and `GetTermForReport` (which mints a persisted term grant — the receipt `GetReport` requires).

Consequence: of the seven calls in this service that write state a later caller can observe, only the settings write and the two tag writes are attributable. Who created an inquiry, who rated one, who exported its document and was handed the download token, and who accepted a report's term are recorded on no audit record at all — the records those handlers do emit carry neither the caller nor the region, so they cannot be attributed to a caller, which is exactly what the crate's own discipline demands of a state change.

The fix is the one the crate already applies in its other mutators: read `::emulator_server::extract_akid(&ctx.parts().headers)` and `crate::state::request_region(ctx.parts())` and emit the existing record on `target: "emulator::audit"` with those two fields added. No response changes.

## Evidence

The discipline, stated in the crate — services/service-com-amazonaws-artifact-artifact/src/ops/tag_resource.rs:24-29:
```
//! The write changes persisted state, so it is ATTRIBUTABLE: this handler leaves exactly
//! one structured record naming the caller (the access-key id and the credential-scope
//! region the request presented), the resource whose tag set it changed, which keys it
//! wrote and the outcome — on the refusal paths as much as on the success one, since a
//! state-changing call that changed nothing is still attributable.
```

The sibling mutator that applies it — src/ops/put_account_settings.rs:160-163 and 202-214:
```
        // The settings record is account-wide, so nothing in the request metadata shapes
        // it; the request's credential scope is read only to ATTRIBUTE the change below.
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx.parts());
...
        ::tracing::info!(
            target: "emulator::audit",
            operation = "PutAccountSettings",
            akid = %akid,
            region = %region,
```
and src/ops/untag_resource.rs:248-261 (`target: "emulator::audit", ... akid = %akid, region = %region, ... outcome = "removed"`).

The write that skips it — src/ops/put_compliance_inquiry_feedback.rs:339-346 persists the record:
```
        inquiry.record_feedback(crate::state::InquiryFeedback {
            query_identifier: input.query_identifier,
            rating: input.rating.clone(),
            reason_codes,
            comment: input.comment.clone(),
            response_revision_id: input.response_revision_id,
            submitted_at: now,
        });
```
and src/ops/put_compliance_inquiry_feedback.rs:357-368 records it with neither the audit target nor the caller:
```
        // Attributable: the inquiry rated, which query, and how. NEVER the `comment` —
        // `@sensitive` in the model — nor the inquiry's name, content or queries.
        ::tracing::info!(
            operation = "PutComplianceInquiryFeedback",
            compliance_inquiry_id = %input.compliance_inquiry_id,
            query_identifier = input.query_identifier,
            response_revision_id = input.response_revision_id,
            rating = %input.rating,
            reason_code_count,
            outcome = "recorded",
            "recorded feedback on a compliance inquiry"
        );
```

The same omission in the three sibling mutators:
- src/ops/create_compliance_inquiry.rs:325-333 — `::tracing::info!(operation = "CreateComplianceInquiry", compliance_inquiry_id = %id, input_source, outcome = "created", ...)` after `store.inquiries.insert(id.clone(), inquiry);` (line 320);
- src/ops/export_compliance_inquiry.rs:254-261 — `::tracing::info!(operation = "ExportComplianceInquiry", ... export_id = %minted.id, include_citations, outcome = "exported", ...)` after `store.record_inquiry_export(..)` (lines 242-248);
- src/ops/get_term_for_report.rs:210-216 — `::tracing::info!(operation = "GetTermForReport", report_id = %report.id, report_version = resolved_version, outcome = "issued", "issued a term grant")` after `store.mint_term_grant(..)` (line 204).

The rule as the crate's own test suites state it — tests/all/resource_tag_write_audit.rs:3-12:
```
//! Every `TagResource` and `UntagResource` call leaves one attributable record.
//!
//! The rule under test ...: a call that changes
//! — or is refused the chance to change — a resource's tag set leaves EXACTLY ONE
//! structured record on the audit target, naming the operation, the caller (the access key
//! id and the credential-scope region the request presented), ...
```
and tests/all/account_settings.rs:15-17:
```
//! * every WRITE of the settings leaves one structured, attributable record on the
//!   `emulator::audit` target: the operation, the access key id the request presented,
//!   the credential scope's region, the status now stored and the outcome;
```

Test coverage of the gap: none. A grep for `audit_capture` / `emulator::audit` over tests/all matches only tests/all/audit_capture.rs (the harness), tests/all/resource_tag_write_audit.rs and tests/all/account_settings.rs; no suite asserts anything about audit records for `CreateComplianceInquiry`, `PutComplianceInquiryFeedback`, `ExportComplianceInquiry` or `GetTermForReport`, so the current behaviour is unpinned and adding the fields changes no existing assertion.

## Steps to Reproduce

1. Install the crate's own capture as the global tracing subscriber (tests/all/audit_capture.rs:101 `audit_capture()`), which keeps every event rendered as `<target> field=value …`.
2. Sign a `POST /v1/compliance-inquiry/create` with Credential=AKIDPROBE/.../eu-west-1/artifact/aws4_request and body {"name":"soc2","inquiryContent":{"query":"Is S3 in scope?"}} — 202.
3. Wait past the 500 ms processing span, then sign `PUT /v1/compliance-inquiry/putFeedback` with the same AKID and {"complianceInquiryId":"<id>","rating":"THUMBS_UP"} — 200.
4. Call `CapturedRecords::drain_for("AKIDPROBE")` (tests/all/audit_capture.rs:41): it returns NOTHING, because `is_for` requires a record on the `emulator::audit` target carrying `akid=…` and neither the create nor the feedback write emits one.
5. Now sign `POST /tags/<percent-encoded inquiry arn>` with the same AKID and {"tags":{"team":"legal"}} — 204 — and drain again: exactly one `emulator::audit … akid=AKIDPROBE region=eu-west-1 … outcome=recorded` record appears, which is the discipline tag_resource.rs:24-29 states and the inquiry mutators skip.
6. Repeat with `POST /v1/compliance-inquiry/export` and `GET /v1/report/getTermForReport?reportId=report-H8wRnP4cYk2JsL6m` (both of which persist a new record plus a download/acceptance token): neither leaves an attributable record either.
