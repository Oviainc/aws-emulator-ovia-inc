Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a7f4b46fd13e3042ad499c1371bfc738191ffa75

# The one-open-request-per-plane guard is region-scoped, so two open increases on one GlobalQuota collide and an APPROVED increase is never in force

_Filed: 2026-10-28T01:58:11Z_
_Operation: RequestServiceQuotaIncrease | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a7f4b46fd13e3042ad499c1371bfc738191ffa75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RequestServiceQuotaIncrease` refuses a second request that is still awaiting a decision for the same quota plane — but it looks for the open request only in the CALLER's region. For a quota the catalog publishes with `GlobalQuota` true, the crate itself defines the plane as the whole ACCOUNT: the approved value is stored in `global_applied_quotas` under a key that carries no region, and `settle_due_requests` deliberately settles every region because "an approved increase on it is in force at every regional endpoint, not just the one it was requested at".

So for a global quota the guard does not guard anything across endpoints: a caller can hold two (or N) simultaneously open requests for the same account-wide plane, one per regional endpoint, and both are approved. Both approvals then write the SAME key of `global_applied_quotas`, so one of the two approved values silently overwrites the other — and because `settle_due_requests` walks `self.regions.keys()` (a `BTreeMap`, i.e. alphabetical region order) rather than decision order, the surviving value is decided by the region code's spelling, not by which increase was decided last or which is larger. The result a client observes is a request reported as `APPROVED` with `DesiredValue` 3000 while `GetServiceQuota` keeps reporting 2000 — an approved increase that never comes into force, which is exactly the failure the global-quota work in this crate was built to prevent.

Within one region the same two requests are correctly refused with `ResourceAlreadyExistsException`, which is what makes this an internal inconsistency rather than a missing feature: the rule exists, it is just applied at the wrong scope for the quotas this crate otherwise treats as account-wide.

## Evidence

The rule the handler states for itself, `src/ops/request_service_quota_increase.rs:45-47`:
```
//! * a second open request for the same quota AND the same context ->
//!   `ResourceAlreadyExistsException` (400): one quota plane has at most one request
//!   awaiting a decision, whether it is `PENDING` or has a support case processing it;
```
The check, which only ever sees the caller's region, `src/ops/request_service_quota_increase.rs:249-257`:
```
        let open = quotas_state
            .requests_for_quota(&region, &input.service_code, &input.quota_code)
            .into_iter()
            .find(|request| {
                crate::state::is_open_request_status(&request.status)
                    // One PLANE of a quota has at most one open request: an open
                    // increase for one resource does not block another resource's.
                    && request.context_id.as_deref() == input.context_id.as_deref()
            });
```
`requests_for_quota` is region-local, `src/state.rs:1222-1231`:
```
    /// Every stored request for one quota, NEWEST FIRST.
    pub fn requests_for_quota(
        &self,
        region: &str,
        service_code: &str,
        quota_code: &str,
    ) -> Vec<QuotaRequest> {
        let Some(state) = self.regions.get(region) else {
            return Vec::new();
        };
```
The other side of the inconsistency — the same crate states that a global quota's plane is the ACCOUNT, `src/state.rs:714-718`:
```
    /// The applied overlay of the quotas AWS counts ACCOUNT-WIDE (`GlobalQuota`).
    /// A global quota has one value for the whole account — which is why its ARN
    /// carries no region — so an approved increase on it is in force at every
    /// regional endpoint, not just the one it was requested at.
    global_applied_quotas: BTreeMap<AppliedKey, AppliedQuota>,
```
and both regions' approvals write that one key, `src/state.rs:1152-1174` (the key is `(service_code, quota_code, context_id)` — no region):
```
                    decided.push((
                        (
                            request.service_code.clone(),
                            request.quota_code.clone(),
                            request.context_id.clone(),
                        ),
                        AppliedQuota {
                            value: request.desired_value,
                        },
                    ));
...
            for (key, applied) in decided {
                if self
                    .default_quota(&key.0, &key.1)
                    .is_some_and(|quota| quota.global_quota)
                {
                    self.global_applied_quotas.insert(key, applied);
```
The write order is alphabetical, not chronological, `src/state.rs:1101-1104`:
```
        let regions: Vec<String> = self.regions.keys().cloned().collect();
        for region in regions {
            self.settle_region_due_requests(&region, now);
        }
```
Test coverage: `tests/all/quota_workflow.rs:802-828` (`the_history_is_newest_first_and_one_request_is_open_at_a_time`) pins the refusal with ONE client at `us-east-1` on the regional quota `vpc`/`L-F678F1CE`, so nothing exercises the guard across endpoints; `tests/all/cross_region_settlement.rs:70-92` raises global quotas (`iam`/`Roles per account`, `route53`/`Hosted zones`, `cloudfront`) in one region and reads them in another, but never opens a second request for the same global quota at a second endpoint.

## Steps to Reproduce

1. Boot the emulator with the Service Quotas service mounted.
2. At the us-east-1 endpoint, call ListAWSDefaultServiceQuotas for ServiceCode `iam` and take the QuotaCode of `Roles per account` (the catalog publishes it with GlobalQuota true).
3. At the us-east-1 endpoint, call RequestServiceQuotaIncrease with that (ServiceCode, QuotaCode) and DesiredValue 2000 -> 200, Status PENDING.
4. Immediately (inside SUPPORT_CASE_OPEN_DELAY) call RequestServiceQuotaIncrease for the SAME (ServiceCode, QuotaCode) with DesiredValue 3000 using credentials whose SigV4 scope names eu-west-1 -> 200 and a second open request, where the same pair at the same endpoint is refused with ResourceAlreadyExistsException.
5. Wait past SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY, then call GetRequestedServiceQuotaChange for the eu-west-1 request -> Status APPROVED, DesiredValue 3000.
6. Call GetServiceQuota for that quota at either endpoint -> Value 2000 (the us-east-1 request's value, because `us-east-1` sorts after `eu-west-1` in the settle walk), so the approved 3000 increase is never in force.
