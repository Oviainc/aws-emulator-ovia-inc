Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# Pagination tokens are scoped only to the operation name, so a token from one ServiceCode silently resumes another's listing

_Filed: 2026-10-28T05:58:21Z_
_Operation: ListAWSDefaultServiceQuotas | Class: pagination_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::paginate` labels a page token with the operation name alone (`Self::NAME`). The token therefore carries no trace of the arguments that *selected* the listing it was cut from — `ServiceCode` for `ListAWSDefaultServiceQuotas`, `ServiceCode`/`QuotaCode`/`QuotaAppliedAtLevel` for `ListServiceQuotas`, `ServiceCode`/`Status`/`QuotaRequestedAtLevel` for the two change-history listings. `decode_page_token` only has to see the operation-name prefix and a parseable offset, and `paginate_bounded` only rejects the offset when it is strictly past the end of the *new* listing (`if start > total { return Err(InvalidPageToken) }`).

Consequence: a `NextToken` issued while listing one service's quotas is accepted when replayed against a different service's listing, and the handler answers HTTP 200 with records read from the middle of that other listing — silently skipping the first N records (or returning an empty terminal page when the second listing is shorter) instead of the `InvalidPaginationTokenException` the crate says a token from another listing gets. An error is turned into a wrong success, and a caller walking a listing with a reused/stale token loses records with no signal.

The crate already knows the right granularity and applies it in exactly one place: `GetQuotaUtilizationReport` labels its token with the operation name *and* the report id so one report's token cannot resume another's. Every other paginated listing in this crate skips that step.

## Evidence

The rule the crate states for its own tokens — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:2003-2005 (doc of `paginate`):

    /// `listing` names the listing the token belongs to (the operation's own name), so a
    /// token issued by one listing is refused by another instead of resuming at an
    /// unrelated position.

The label actually used — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/list_aws_default_service_quotas.rs:146-151:

        let page = crate::state::paginate(
            Self::NAME,
            quotas,
            input.max_results,
            input.next_token.as_deref(),
        )

(identically in list_service_quotas.rs:182-187, list_requested_service_quota_change_history.rs:220-225, list_requested_service_quota_change_history_by_quota.rs:163-168, list_services.rs:121-126.)

The sibling path that DOES scope the token to the selected result set — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/get_quota_utilization_report.rs:229-239:

        // The token names a position in THIS report, so one report's token cannot
        // resume another's listing.
        let listing = format!("{}:{}", Self::NAME, report.id);
        let page = crate::state::paginate_bounded(
            &listing,
            ...

Why a mismatched token is not refused — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:2056-2058 and 2086-2088:

        if start > total {
            return Err(InvalidPageToken);
        }
...
        text.strip_prefix(&format!("{listing}:"))
            .and_then(|offset| offset.parse::<usize>().ok())
            .ok_or(InvalidPageToken)

Test coverage: services/service-com-amazonaws-servicequotas-servicequotasv20190624/tests/all/quota_workflow.rs:1163-1223 (`a_next_token_this_service_did_not_issue_is_refused`) exercises only cross-OPERATION reuse — "// And a REAL token, issued by ONE listing: it must not resume another listing." with the token taken from `ListServices` — so same-operation/different-argument reuse is unpinned. services/service-com-amazonaws-servicequotas-servicequotasv20190624/tests/all/utilization_report.rs:694-698 pins the finer rule for the one listing that implements it:

        (
            "a token another report issued",
            json!({"ReportId": mine, "NextToken": token}),
            "InvalidPaginationTokenException",
        ),

## Steps to Reproduce

1. POST / with X-Amz-Target: ServiceQuotasV20190624.ListAWSDefaultServiceQuotas and body {"ServiceCode":"ec2","MaxResults":2}; keep the returned NextToken T (it decodes to base64 of "ListAWSDefaultServiceQuotas:2")
2. POST / with X-Amz-Target: ServiceQuotasV20190624.ListAWSDefaultServiceQuotas and body {"ServiceCode":"iam","NextToken":T}
3. Observe HTTP 200 whose Quotas start at offset 2 of the `iam` listing — the first two `iam` default quotas are silently skipped — instead of the InvalidPaginationTokenException the state.rs:2003-2005 rule describes for a token issued by another listing
4. Repeat with ListServiceQuotas: take a token from {"ServiceCode":"ec2","MaxResults":2} and replay it against {"ServiceCode":"ec2","QuotaCode":"L-F678F1CE"} or against {"ServiceCode":"ec2","QuotaAppliedAtLevel":"RESOURCE"} — a token cut from a different result set is accepted the same way
5. Repeat with ListRequestedServiceQuotaChangeHistory: take a token from {"Status":"PENDING","MaxResults":1} and replay it with no Status filter
