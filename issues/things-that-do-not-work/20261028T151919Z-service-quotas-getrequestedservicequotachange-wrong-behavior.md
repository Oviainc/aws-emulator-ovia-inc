Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# A global quota's open request is reported by id at every endpoint but is only readable at the endpoint it was made at

_Filed: 2026-10-28T15:19:19Z_
_Operation: GetRequestedServiceQuotaChange | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a quota the catalog publishes as counted account-wide (`GlobalQuota`), this crate deliberately makes the "one request awaiting a decision" plane ACCOUNT-wide: `open_request_on_plane` scans every region's request store and `RequestServiceQuotaIncrease` refuses a second request at ANY regional endpoint, naming the open request's `Id` in the refusal `Message`.

But the three operations that resolve a request by that `Id` are scoped to the caller's region only (`state::request`, `state::open_support_case`, `state::requests_for_quota`). So the same endpoint that just told the client "request <id> ... is still awaiting a decision" answers `NoSuchResourceException` — "no quota increase request with RequestId '<id>' exists" — when the client reads that very id back, and `CreateSupportCase` on it is refused the same way. The service names an id and then denies it exists, at one and the same endpoint, in two consecutive calls.

The two halves of the crate cannot both be right: either the request plane for a `GlobalQuota` is account-wide (as `open_request_on_plane` implements and the handler doc states) and the id it hands out must resolve wherever it is handed out, or the plane is per region and the refusal must not name a request from another region. Today the write path uses the account-wide rule and the read paths use the per-region one.

This affects every account-wide quota in the seeded catalog (iam, route53, cloudfront, organizations, shield, ...).

## Evidence

The write path says the plane is the whole ACCOUNT, and the refusal names the other region's request id.

services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/request_service_quota_increase.rs:45-51 (module doc):

    //! * a second open request for the same quota AND the same context ->
    //!   `ResourceAlreadyExistsException` (400): one quota plane has at most one request
    //!   awaiting a decision, whether it is `PENDING` or has a support case processing it.
    //!   The plane is as wide as the value an approved increase puts in force: the whole
    //!   ACCOUNT for a quota the catalog counts account-wide (`GlobalQuota`), so a request
    //!   made at ANY regional endpoint blocks it, and the caller's Region for any other
    //!   quota;

request_service_quota_increase.rs:264-276:

        if let Some(open) = open {
            return Err(
                RequestServiceQuotaIncreaseErrorModeled::ResourceAlreadyExistsException(
                    ResourceAlreadyExistsException {
                        message: Some(format!(
                            "request {} for QuotaCode '{}' is still awaiting a decision",
                            open.id, input.quota_code
                        )),

and the helper it calls scans EVERY region for an account-wide quota — src/state.rs:1226-1233:

        let account_wide = self
            .default_quota(service_code, quota_code)
            .is_some_and(|quota| quota.global_quota);
        let stores: Vec<&RegionState> = if account_wide {
            self.regions.values().collect()
        } else {
            self.regions.get(region).into_iter().collect()
        };

The read paths are region-only — src/state.rs:1161-1164:

    /// One stored request by `Id`, within the caller's region.
    pub fn request(&self, region: &str, id: &str) -> Option<&QuotaRequest> {
        self.regions.get(region)?.quota_requests.get(id)
    }

src/ops/get_requested_service_quota_change.rs:109-121:

        let Some(request) = quotas_state.request(&region, &input.request_id) else {
            return Err(
                GetRequestedServiceQuotaChangeErrorModeled::NoSuchResourceException(
                    NoSuchResourceException {
                        message: Some(format!(
                            "no quota increase request with RequestId '{}' exists",
                            input.request_id
                        )),

and the same region-only lookup backs `CreateSupportCase` — src/state.rs:1184-1190 (`open_support_case`): `self.regions.get_mut(region).and_then(|state| state.quota_requests.get_mut(id))` -> `SupportCaseRefusal::NoSuchRequest`, mapped at src/ops/create_support_case.rs:154-162; and the by-quota history — src/state.rs:1259-1261: `let Some(state) = self.regions.get(region) else { return Vec::new(); };`

Test coverage: tests/all/open_request_plane_scope.rs:312-315 pins that the SECOND endpoint's refusal names the first request's id —

            assert!(
                message.contains(&first_id),
                "case {}: the refusal must name the open request {first_id}, got {message:?}",
                row.why
            );

but the suite only ever reads that id back through the FIRST endpoint (open_request_plane_scope.rs:347 `stored_request(&first_caller, &first_id)`), so nothing pins the second endpoint's 404 on the id it just advertised.

## Steps to Reproduce

1. Start the emulator and sign a request with a us-east-1 SigV4 credential scope.
2. POST / with X-Amz-Target: ServiceQuotasV20190624.ListAWSDefaultServiceQuotas and body {"ServiceCode":"iam"}; find the quota named "Users per account" and note its QuotaCode and Value (GlobalQuota is true).
3. At the us-east-1 endpoint POST / with X-Amz-Target: ServiceQuotasV20190624.RequestServiceQuotaIncrease and body {"ServiceCode":"iam","QuotaCode":"<code>","DesiredValue":<2x Value>}; it is accepted PENDING.
4. Immediately (within SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY) repeat the same RequestServiceQuotaIncrease against an eu-west-1-scoped endpoint: it is refused ResourceAlreadyExistsException with Message "request <id> for QuotaCode '<code>' is still awaiting a decision".
5. At that same eu-west-1 endpoint POST / with X-Amz-Target: ServiceQuotasV20190624.GetRequestedServiceQuotaChange and body {"RequestId":"<id>"} — observe NoSuchResourceException "no quota increase request with RequestId '<id>' exists" for the id the previous response just named.
6. Same endpoint, CreateSupportCase with {"RequestId":"<id>"} — also NoSuchResourceException; and ListRequestedServiceQuotaChangeHistoryByQuota for the pair returns an empty RequestedQuotas list.
