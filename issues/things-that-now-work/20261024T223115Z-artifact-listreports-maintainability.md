Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 27d60cb41c89860474fb4eac178b022e94c750b0

# The paginated page-walk rule is written out five times, once per listing handler

_Filed: 2026-10-24T22:31:15Z_
_Operation: ListReports | Class: maintainability_

> _Note: this issue was identified against the code base at commit `27d60cb41c89860474fb4eac178b022e94c750b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate centralises the pagination CURSOR (encode_cursor/decode_cursor plus one mint/parse pair per listing in src/state.rs) but not the WALK that uses it. The identical rule — refuse a nextToken that does not decode to this operation's own cursor, skip entries while `position < resume_at`, cut the page by minting a token at `position` when `items.len() >= page_size`, otherwise push the projection and advance `position` — is spelled out five separate times, once in each paginated handler:

- src/ops/list_reports.rs:171-182 (token refusal) and 187-211 (walk)
- src/ops/list_report_versions.rs:178-191 and 204-221
- src/ops/list_customer_agreements.rs:173-184 and 189-213
- src/ops/list_compliance_inquiries.rs:181-192 and 197-219
- src/ops/list_compliance_inquiry_queries.rs:192-210 and 231-250

Five copies of one branch structure can drift: the cut condition, the order of the resume check against the page-full check, and whether a filtered-out entry advances `position` all have to stay byte-identical for the five tokens to keep meaning "the number of entries this operation has already reported" (the meaning src/state.rs:1836-1840 states for every cursor of this service). Any edit to one copy that is not mirrored in the other four silently changes what a token resumes at for that one listing only. The crate already shows the shape a shared helper would take: `crate::state::page_size` (src/state.rs:2240) is the one place the page size is decided for all five, and the cursor codec is the one place a token is sealed for all five; the walk is the remaining copy-pasted half.

The fix is a single helper beside `page_size` in src/state.rs that takes the already-filtered iterator, `resume_at`, `page_size` and a token minter and returns (items, next_token) — leaving every response byte and every existing test unchanged, because the branch order it implements is the one all five copies already share.

## Evidence

Copy 1 — services/service-com-amazonaws-artifact-artifact/src/ops/list_reports.rs:200-211:
```
            // Resume at the cursor's position in this operation's own total order.
            if position < resume_at {
                position += 1;
                continue;
            }
            if reports.len() >= page_size {
                next_token = Some(crate::state::reports_page_token(position));
                break;
            }
            reports.push(report.to_summary(latest, &region));
            position += 1;
```

Copy 2 — services/service-com-amazonaws-artifact-artifact/src/ops/list_customer_agreements.rs:201-212:
```
            // Resume at the cursor's position in this operation's own reported order, so
            // a page is still served when the agreement it was cut after left the set.
            if position < resume_at {
                position += 1;
                continue;
            }
            if customer_agreements.len() >= page_size {
                next_token = Some(crate::state::customer_agreements_page_token(position));
                break;
            }
            customer_agreements.push(agreement.to_summary(&region));
            position += 1;
```

Copies 3-5 are the same five statements with another collection and another mint function: src/ops/list_report_versions.rs:208-220, src/ops/list_compliance_inquiries.rs:203-218, src/ops/list_compliance_inquiry_queries.rs:235-249.

The refusal block is duplicated the same way; src/ops/list_reports.rs:171-182:
```
        let resume_at = match input.next_token.as_deref() {
            None => 0usize,
            Some(token) => match crate::state::parse_reports_page_token(token) {
                Some(position) => position,
                None => {
                    return Err(ListReportsErrorModeled::ValidationException(
                        crate::state::invalid_token(),
                    )
                    .into())
                }
            },
        };
```
and src/ops/list_compliance_inquiries.rs:181-192 is the same block with `parse_inquiries_page_token` and `ListComplianceInquiriesErrorModeled`.

That all five are meant to be ONE rule is stated in the crate itself — src/state.rs:1845-1849: "This holds for EVERY paginated operation of the service, not only the report listings: `ListReports`, `ListReportVersions`, `ListCustomerAgreements`, `ListComplianceInquiries` and `ListComplianceInquiryQueries` all cut their page with a cursor from this codec".

Test coverage that pins the behaviour a refactor must not change: tests/all/pagination_token_opacity.rs (rows for ListReports / ListReportVersions / ListCustomerAgreements at lines 55-74; `a_next_token_is_opaque_and_resumes_exactly_after_its_page` at :199 and `a_value_this_operation_did_not_cut_is_refused_as_a_next_token` at :280), tests/all/report_workflow.rs:257 and :833, tests/all/customer_agreements.rs:556 and :626, tests/all/compliance_inquiry_listing.rs:582 and :665, tests/all/compliance_inquiry_queries.rs:494.

## Steps to Reproduce

1. Open services/service-com-amazonaws-artifact-artifact/src/ops/list_reports.rs and read lines 171-211.
2. Open src/ops/list_customer_agreements.rs lines 173-213, src/ops/list_report_versions.rs lines 178-221, src/ops/list_compliance_inquiries.rs lines 181-219 and src/ops/list_compliance_inquiry_queries.rs lines 192-250: the same token-refusal block and the same five-statement page-walk appear in each, differing only in the collection, the projection and the mint/parse function.
3. Note that src/state.rs:1845-1849 declares the rule to be one rule shared by all five operations, and that src/state.rs:2240 (`page_size`) already centralises the other half of the same decision.
4. Change the cut condition in one copy (e.g. make ListReports mint its token before pushing) and observe that only that listing's pagination changes — nothing ties the five copies together.
