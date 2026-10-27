Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 22964030325ab1e634e5ef0028a831687ff48548

# CreateDomain ignores its @idempotent trait: an identical repeat mints a second domain and burns another slot of the per-account cap

_Filed: 2026-10-26T00:01:04Z_
_Operation: CreateDomain | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `22964030325ab1e634e5ef0028a831687ff48548`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDomain` is modelled `@idempotent`, and this crate treats that trait everywhere else as a behavioural contract it implements and tests: `DeleteDomain`, `DeactivateSubscription`, `ActivateSubscription` and `TagResource` each carry an explicit `// `@idempotent`: ...` guard whose job is to make a repeat change nothing, and `StartPatientInsightsJob` even keeps a digest of every request parameter (`PatientInsightsJobSpec::fingerprint`) so a repeated request can be told from a different one.

`CreateDomain`'s handler has no such guard and never mentions the trait: it unconditionally mints a fresh `domainId`, records a fresh allocation, builds a fresh ARN and inserts a brand-new `Domain` record on every call. So the one case `@idempotent` exists for — a POST whose 201 the client never saw, retried with a byte-identical body — produces TWO domains instead of one. Both are charged against the account's live per-Region domain quota (`MAX_DOMAINS_PER_ACCOUNT = 10`, enforced at create_domain.rs:235), and the first one is orphaned: the caller never learned its `domainId`, and `DeleteDomain` is addressable only by id, so the slot cannot be reclaimed without listing and guessing which duplicate to remove.

The defect is that the repeat is not recognised at all — the handler resolves nothing about the request before minting. Every other write in this crate that the model marks `@idempotent` recognises its repeat.

## Evidence

The model marks the operation idempotent — `smithy_query(query="operation", operation="CreateDomain")`:
```
"CreateDomain": { ... "http": {"method": "POST", "uri": "/domain", "code": 201}, "idempotent": true}
```
and the denormalised surface repeats it, services/service-com-amazonaws-connecthealth-connecthealth/OPERATIONS.md:36:
```
- **Traits:** @idempotent
```

The handler mints unconditionally — services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_domain.rs:288-293:
```
        let domain_id = store.mint_domain_id();
        // The id is remembered as ALLOCATED for as long as the history holds it, past
        // the domain's own purge: `DeleteDomain` tells an id this account once owned from
        // one that was never allocated anywhere by exactly this record.
        store.record_domain_allocation(&domain_id);
        let arn = crate::state::domain_arn(&region, &domain_id);
```
(nothing between the quota checks at lines 234-263 and this mint looks for a domain the same request already created; the handler's own header, create_domain.rs:4-6, says only "mints a `domainId` ... PERSISTS the record" and never mentions the trait.)

The siblings in THIS crate that do honour the same trait:
- src/ops/delete_domain.rs:359-361:
```
        // `@idempotent`: a domain already being deleted keeps the instants it already
        // has, so repeating the call neither errors nor postpones the deletion.
        if domain.deleted_at.is_none() {
```
- src/ops/tag_resource.rs:244-246:
```
        // `@idempotent`: the merged set is what is stored, so repeating the call writes
        // the same set again and changes nothing.
        store.set_tags(&input.resource_arn, merged);
```
- src/ops/deactivate_subscription.rs:179-185:
```
        // `@idempotent`: deactivating an already-inactive subscription is a no-op, so
        // the stamps it already carries stay put rather than being rewritten per call.
        ...
        if subscription.status != crate::state::SubscriptionStatus::Inactive {
```
- and the crate already owns a "is this the same request?" primitive, src/state.rs:419-427:
```
    /// A digest of every parameter, so a repeated `clientToken` can be told from a token
    /// reused for DIFFERENT parameters without keeping a second copy of the request
    /// around.
    pub fn fingerprint(&self) -> String {
```

What the duplicate costs — src/state.rs:49-63:
```
/// The most `Domain` records ONE ACCOUNT holds at a time IN ONE REGION — the cap
/// `CreateDomain` is subject to.
...
pub const MAX_DOMAINS_PER_ACCOUNT: usize = 10;
```

Test coverage: the crate's tests pin the `@idempotent` repeat-is-a-no-op rule for the other operations with a stated reason — tests/all/domain_subscription_lifecycle.rs:544-552 ("repeats, which `@idempotent` says must not error or change the stored record", rows "activate twice" / "deactivate twice"), tests/all/resource_tagging.rs:169 ("the same call repeated (@idempotent)"), tests/all/domain_response_projection.rs:351 ("A deletion still in flight is observable, and `@idempotent`: repeating the ..."). NO test covers a repeated `CreateDomain`: every create in the suite uses a distinct `name` (e.g. tests/all/domain_subscription_lifecycle.rs:292-321, tests/all/domain_account_cap_and_lookup_semantics.rs:110-119, tests/all/region_scoping.rs:460-462 where the repeated names live in different Regions), so no existing assertion pins the duplicate-on-repeat behaviour.

## Steps to Reproduce

1. POST /domain with body {"name": "clinic-a"} → 201, note the returned domainId (say dom-X).
2. Replay the SAME request byte-for-byte, as an SDK/transport retry of a response that was lost → 201 with a DIFFERENT domainId (dom-Y) and a different ARN, instead of the first call's result.
3. GET /domain?maxResults=100 → both dom-X and dom-Y are listed: one logical create has consumed two of the account's MAX_DOMAINS_PER_ACCOUNT (10) slots in this Region, and dom-X is unreachable to the caller that never saw the first response.
4. Contrast with the repeats the same crate does recognise: DELETE /domain/{dom-Y} twice (src/ops/delete_domain.rs:361 keeps the first deletedAt) and POST /tags/{arn} twice (src/ops/tag_resource.rs:246 re-stores the same merged set) — both leave the store unchanged on the second call, as their modelled @idempotent requires.
