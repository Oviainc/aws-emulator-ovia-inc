Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# A default policy stating CreateInterval=7 with no RetainInterval is accepted and read back with RetainInterval equal to the creation frequency

_Filed: 2026-10-26T23:21:29Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Cadence::violation` enforces the documented ordering rule ("it must be greater than the creation frequency") only when BOTH members have a value some request stated — the request body, the `PolicyDetails` structure, or (for an update that keeps the stored configuration) the stored value. The documented per-member DEFAULT is deliberately left out of that comparison, and the helper's own doc justifies that by asserting the defaults "satisfy the ordering". That assertion only holds when BOTH members are defaulted (1 and 7). When a caller states `CreateInterval: 7` — the top of its own documented range, so the magnitude check passes — and states no `RetainInterval`, nothing is compared, the policy is stored, and `effective_policy_details` then materialises the documented retention default 7, so `GetLifecyclePolicy` / `GetLifecyclePolicies` report `CreateInterval: 7` beside `RetainInterval: 7` — a retention period that is NOT greater than the creation frequency, i.e. exactly the configuration the same handler refuses when the caller spells both members out (the test table's "a retention period equal to the creation frequency" row, 3/3, is a 400). One effective configuration is therefore both refused and produced, depending only on whether the caller wrote the value the service would have filled in anyway.

The same hole is on the update path through the same helper: a PATCH stating `CreateInterval: 7` (body) against a policy that never stored a `RetainInterval`, or a PATCH whose `PolicyDetails` states `CreateInterval: 7` and no `RetainInterval` (which replaces the configuration wholesale, so the stored retention period is dropped), both leave the stored policy reporting 7/7.

## Evidence

Model — `smithy_query(query="doc", shape="CreateLifecyclePolicyRequest", member="RetainInterval")`: "The retention period can range from 2 to 14 days, but it must be greater than the creation frequency to ensure that the policy retains at least 1 snapshot or AMI at any given time. If you do not specify a value, the default is 7." (the same sentence on `UpdateLifecyclePolicyRequest$RetainInterval` and `PolicyDetails$RetainInterval`).

The comparison, which only runs when both sides were stated — `src/state.rs:1288-1304`:
```
        let create_interval = self
            .body_create_interval
            .or(self.details_create_interval)
            .or(self.stored_create_interval);
        let retain_interval = self
            .body_retain_interval
            .or(self.details_retain_interval)
            .or(self.stored_retain_interval);
        if let (Some(create_interval), Some(retain_interval)) = (create_interval, retain_interval) {
            if retain_interval <= create_interval {
                return Some(invalid_request(format!(
                    "The parameter RetainInterval ({retain_interval} days) must be greater than \
                     the creation frequency CreateInterval ({create_interval} days) to ensure \
                     that the policy retains at least 1 snapshot or AMI at any given time."
```

The crate's own justification for excluding the default, whose premise fails at CreateInterval=7 — `src/state.rs:1254-1258`:
```
    /// already stored for the other. A member NOTHING has stated takes the
    /// documented default ([`DEFAULT_CREATE_INTERVAL`] /
    /// [`DEFAULT_RETAIN_INTERVAL`], which satisfy the ordering), and a default
    /// the service chose is not a value the caller stated: it is left out of the
    /// comparison rather than turned into a refusal of the one member stated.
```

The read then reports that very default, so the pair a client sees violates the rule — `src/state.rs:544-545`:
```
        details.create_interval = Some(self.create_interval.unwrap_or(DEFAULT_CREATE_INTERVAL));
        details.retain_interval = Some(self.retain_interval.unwrap_or(DEFAULT_RETAIN_INTERVAL));
```
with `DEFAULT_RETAIN_INTERVAL` = 7 (`src/state.rs:137`) and the accepted ceiling `CREATE_INTERVAL_MAX_DAYS` = 7 (`src/state.rs:160`).

The create path that builds the `Cadence` with no stored side — `src/ops/create_lifecycle_policy.rs:258-277` (`// A create has nothing stored to compare against.` / `..crate::state::Cadence::default()` / `.violation()`), and the update path — `src/ops/update_lifecycle_policy.rs:337-356` (`let keeps_stored_configuration = replaced_details.is_none();` … `stored_retain_interval: keeps_stored_configuration.then_some(policy.retain_interval).flatten()`).

Test coverage: NOT pinned. `tests/all/interval_validation.rs:247-252` covers the one-member case only at a value where the default happens to satisfy the ordering:
```
        CadenceCase {
            case: "only the creation frequency stated, inside its range",
            create_interval: Some(3),
            retain_interval: None,
            refused: None,
        },
```
while `tests/all/interval_validation.rs:296-301` pins the refusal of the equal pair when both are stated:
```
        CadenceCase {
            case: "a retention period equal to the creation frequency",
            create_interval: Some(3),
            retain_interval: Some(3),
            refused: Some("RetainInterval"),
        },
```
No row states `CreateInterval: 7` with `retain_interval: None`, and `tests/all/interval_validation.rs:378-379` asserts the omitted member reads back as its documented default, so the 7/7 read is unguarded.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "weekly default policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "CreateInterval": 7} — observe HTTP 200 and note the PolicyId
2. GET /policies/{PolicyId} — observe Policy.PolicyDetails.CreateInterval = 7 and Policy.PolicyDetails.RetainInterval = 7, a retention period that is not greater than the creation frequency
3. POST /policies again with the same body plus "RetainInterval": 7 — observe HTTP 400 InvalidRequestException (Code InvalidParameter) "The parameter RetainInterval (7 days) must be greater than the creation frequency CreateInterval (7 days)…" for the configuration step 2 just reported as stored
4. Same on the update path: create a default policy with {"DefaultPolicy": "VOLUME"} only (no intervals), then PATCH /policies/{PolicyId} with {"CreateInterval": 7} — observe HTTP 200, then GET and observe 7/7
