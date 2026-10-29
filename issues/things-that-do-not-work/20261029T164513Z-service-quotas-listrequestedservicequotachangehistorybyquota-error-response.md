Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 94313aadf7366efaf8ae51cba58459f4f1ec87e1

# ListRequestedServiceQuotaChangeHistoryByQuota is the one ServiceCode-taking listing that does not state the "no such service" fact in the crate's single wording

_Filed: 2026-10-29T16:45:13Z_
_Operation: ListRequestedServiceQuotaChangeHistoryByQuota | Class: error-response_

> _Note: this issue was identified against the code base at commit `94313aadf7366efaf8ae51cba58459f4f1ec87e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::no_such_service_message` is declared as the ONE wording this service uses whenever a request names a `ServiceCode` the catalog does not hold, and its doc enumerates "the listings that take a `ServiceCode`" as the callers of it. Three of the four listings that take a `ServiceCode` check `knows_service` and call that helper (`ListServiceQuotas`, `ListAWSDefaultServiceQuotas`, `ListRequestedServiceQuotaChangeHistory`). `ListRequestedServiceQuotaChangeHistoryByQuota` makes no service check at all: it only looks the `(ServiceCode, QuotaCode)` PAIR up and refuses with `no_such_quota_message`. So the same input fact — a `ServiceCode` the catalog does not publish — is answered by the two sibling history listings with two different `NoSuchResourceException` `Message` bodies, which is precisely what the helper's doc says cannot happen, and the by-quota listing attributes the failure to the quota code the caller supplied rather than to the service that does not exist.

## Evidence

The rule, services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:2055-2063:

    /// How this service states that `service_code` names no service it publishes quotas
    /// for, for the modelled `NoSuchResourceException`.
    ///
    /// ONE wording for the fact, wherever a request names a service the catalog does not
    /// hold: the listings that take a `ServiceCode`, and an Automatic Management exclusion
    /// keyed by one.
    pub fn no_such_service_message(service_code: &str) -> String {
        format!("no service quotas are available for ServiceCode '{service_code}'")
    }

The sibling listing that obeys it — src/ops/list_requested_service_quota_change_history.rs:185-201:

            // A `ServiceCode` the catalog does not publish names no service, so the
            // listing is a lookup failure rather than an empty page. A caller that
            // names none asks for every service's requests.
            if let Some(service_code) = input.service_code.as_deref() {
                if !quotas_state.knows_service(service_code) {
                    return Err(
                        ListRequestedServiceQuotaChangeHistoryErrorModeled::NoSuchResourceException(
                            NoSuchResourceException {
                                message: Some(crate::state::no_such_service_message(
                                    service_code,
                                )),

(the same `knows_service` + `no_such_service_message` pair is in src/ops/list_service_quotas.rs:129-136 and src/ops/list_aws_default_service_quotas.rs:126-137).

The path that skips it — src/ops/list_requested_service_quota_change_history_by_quota.rs:132-147:

            if quotas_state
                .default_quota(&input.service_code, &input.quota_code)
                .is_none()
            {
                return Err(
                    ListRequestedServiceQuotaChangeHistoryByQuotaErrorModeled::NoSuchResourceException(
                        NoSuchResourceException {
                            message: Some(crate::state::no_such_quota_message(
                                &input.service_code,
                                &input.quota_code,
                            )),

There is no `knows_service` call anywhere in that handler, so an unknown `ServiceCode` falls through the pair lookup and is reported as a missing quota.

Test coverage: tests/all/requested_change_history.rs:445-470 exercises exactly this input against the service-wide listing (`json!({"ServiceCode": "not-a-real-service-xyz"})`, `json!({"ServiceCode": "Vpc"})`) but asserts only `code.contains("NoSuchResourceException")` and a non-empty `Message`; tests/all/history_filter_chain.rs has no unknown-service case at all, and a grep for "no service quotas are available" / "no quota with ServiceCode" over the crate's tests returns no matches — so neither sentence is pinned and the fix changes no asserted body.

## Steps to Reproduce

1. POST / with X-Amz-Target: ServiceQuotasV20190624.ListRequestedServiceQuotaChangeHistory and body {"ServiceCode":"not-a-real-service-xyz"} — note NoSuchResourceException with Message "no service quotas are available for ServiceCode 'not-a-real-service-xyz'"
2. POST / with X-Amz-Target: ServiceQuotasV20190624.ListRequestedServiceQuotaChangeHistoryByQuota and body {"ServiceCode":"not-a-real-service-xyz","QuotaCode":"L-F678F1CE"}
3. Observe NoSuchResourceException with Message "no quota with ServiceCode 'not-a-real-service-xyz' and QuotaCode 'L-F678F1CE' exists" — a second wording for the fact state.rs:2058-2060 says has exactly one, naming the quota code instead of the absent service
4. For contrast, call ListServiceQuotas and ListAWSDefaultServiceQuotas with the same unknown ServiceCode: both answer the single service wording
