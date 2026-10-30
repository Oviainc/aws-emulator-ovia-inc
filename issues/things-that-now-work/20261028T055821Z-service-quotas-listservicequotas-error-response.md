Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# ListServiceQuotas hand-rolls its own "no such quota" message instead of the single wording helper its siblings use

_Filed: 2026-10-28T05:58:21Z_
_Operation: ListServiceQuotas | Class: error-response_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::no_such_quota_message` is declared as the ONE wording this service uses whenever a request names a `(ServiceCode, QuotaCode)` pair the catalog does not hold, and every other read or write of a single quota calls it. `ListServiceQuotas`' `QuotaCode` filter is such a read — the handler's own module doc lists it as "a `QuotaCode` the service has no quota for, is `NoSuchResourceException`" — but the handler formats its own sentence inline instead of calling the helper. Two different `Message` bodies therefore describe one fact, and the copies can drift from the helper the rest of the crate changes.

## Evidence

The helper and the rule it states — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:1771-1779:

    /// How this service states that no quota answers to a `(ServiceCode, QuotaCode)` pair,
    /// for the modelled `NoSuchResourceException`.
    ///
    /// ONE wording for the fact, wherever a request names a pair the catalog does not
    /// hold: the reads and the writes of one quota, and an Automatic Management exclusion
    /// naming one.
    pub fn no_such_quota_message(service_code: &str, quota_code: &str) -> String {
        format!("no quota with ServiceCode '{service_code}' and QuotaCode '{quota_code}' exists")
    }

The sibling handlers that call it: get_service_quota.rs:119-122, get_aws_default_service_quota.rs:121-124, list_requested_service_quota_change_history_by_quota.rs:134-137, request_service_quota_increase.rs:165-168 — e.g.

                    message: Some(crate::state::no_such_quota_message(
                        &input.service_code,
                        &input.quota_code,
                    )),

The path that needs it and skips it — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/list_service_quotas.rs:138-151:

            if let Some(requested) = input.quota_code.as_deref() {
                codes.retain(|code| code == requested);
                if codes.is_empty() {
                    return Err(ListServiceQuotasErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(format!(
                                "ServiceCode '{}' has no quota with QuotaCode '{requested}'",
                                input.service_code
                            )),
                        },
                    )
                    .into());

The handler's own doc naming this as the same fact — list_service_quotas.rs:9-10: "//! * an unknown `ServiceCode`, or a `QuotaCode` the service has no quota for, is //!   `NoSuchResourceException`;". No test pins either sentence: grep for "has no quota with QuotaCode" and "no quota with ServiceCode" over services/service-com-amazonaws-servicequotas-servicequotasv20190624/tests returns no matches.

## Steps to Reproduce

1. POST / with X-Amz-Target: ServiceQuotasV20190624.GetServiceQuota and body {"ServiceCode":"ec2","QuotaCode":"L-00000000"}; note Message = "no quota with ServiceCode 'ec2' and QuotaCode 'L-00000000' exists"
2. POST / with X-Amz-Target: ServiceQuotasV20190624.ListServiceQuotas and body {"ServiceCode":"ec2","QuotaCode":"L-00000000"}
3. Observe NoSuchResourceException whose Message is "ServiceCode 'ec2' has no quota with QuotaCode 'L-00000000'" — a second wording for the fact state.rs:1774-1776 says has exactly one
