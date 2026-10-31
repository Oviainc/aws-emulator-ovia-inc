Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 3302ef7557ecef1ce1978c32967329b865cc9c8a

# The minimum-length bound AmendBenefitApplication puts on `amendments` is worded by hand, not through this crate's single length-wording helper that the maximum two lines below uses

_Filed: 2026-10-28T23:48:43Z_
_Operation: AmendBenefitApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `3302ef7557ecef1ce1978c32967329b865cc9c8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AmendmentList` carries no `@length` in the model, so `AmendBenefitApplication`'s handler enforces BOTH ends of the bound itself — its own comment says exactly that ("so both bounds live here"). The maximum goes through `crate::service::constraint_requirement(&ViolationKind::MaxLength(..))`, the helper this crate documents as existing precisely so "a handler refusing an unmodelled constraint of the same CLASS (a length an operation enforces itself, say) states it with the same words the centralized walk uses, rather than a copy that can drift". The minimum, enforced two lines above against the SAME member, bypasses that helper and states a hand-written sentence instead.

The result: a caller that sends `Amendments: []` is refused with `Value at 'amendments' failed to satisfy constraint: Member must name at least one field to amend`, while a caller that sends 21 amendments is refused with `Value at 'amendments' failed to satisfy constraint: Member must have length less than or equal to 20`. Two length violations on one member, in two vocabularies, from one handler — which is the drift `constraint_requirement` is documented to prevent. `ViolationKind::MinLength` already has a wording in that helper (`Member must have length greater than or equal to {min}`), and every other in-handler length refusal of this crate reaches it through `crate::state::length_clause` (recall's `reason`/`clientToken`, amend's own `amendmentReason`/`clientToken`/`newValue`) or directly (`AssociateBenefitApplicationResource::record_full`). This one refusal is the only handler-enforced length in the crate that does not.

No behavioural impact beyond the message/`ValidationExceptionField$Message` text, hence LOW — but it is the one member whose two bounds a client cannot parse uniformly.

## Evidence

The helper's own stated purpose — services/.../src/service.rs:75-82:
```
/// The requirement one violation class states, in this service's wording.
///
/// Separate from [`constraint_clause`] so a handler refusing an unmodelled constraint of
/// the same CLASS (a length an operation enforces itself, say) states it with the same
/// words the centralized walk uses, rather than a copy that can drift.
///
/// `None` for a class this service has no wording for — see [`constraint_clause`].
pub fn constraint_requirement(kind: &ViolationKind) -> Option<String> {
```
and it already words a MinLength — services/.../src/service.rs:85-87:
```
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
```

The handler calls both ends "bounds" on the same member, then words only one of them through the helper — services/.../src/ops/amend_benefit_application.rs:434-446:
```
        // An amendment request has to name what it amends, and no more fields than this
        // service applies in one request: the model requires the list but constrains no
        // length, so both bounds live here — ahead of the store lock, so the loop that
        // applies them runs under the lock with a cited bound.
        if input.amendments.is_empty() {
            return Err(refused(
                "amendments",
                "Member must name at least one field to amend",
            ));
        }
        if let Some(clause) = amendments_length_clause(input.amendments.len()) {
            return Err(refused("amendments", &clause));
        }
```
and the max-half helper it skips — services/.../src/ops/amend_benefit_application.rs:243-252:
```
/// The number of amendments one request may carry, as the clause states it — a length
/// bound on the list, worded like any other.
fn amendments_length_clause(count: usize) -> Option<String> {
    (count > crate::state::MAX_AMENDMENTS_PER_REQUEST).then(|| {
        crate::service::constraint_requirement(&ViolationKind::MaxLength(
            crate::state::MAX_AMENDMENTS_PER_REQUEST as i64,
        ))
        .unwrap_or_default()
    })
}
```

The sibling in-handler length bound that DOES go through the helper, with the policy restated — services/.../src/ops/associate_benefit_application_resource.rs:154-163:
```
/// service's modelled `ValidationException`, stating the modelled length the stored list
/// may not exceed. The requirement is worded by the helper the centralized walk words a
/// length with ([`crate::service::constraint_requirement`]) ...
fn record_full() -> AssociateBenefitApplicationResourceError {
    let requirement = crate::service::constraint_requirement(&ViolationKind::MaxLength(
        crate::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION as i64,
    ))
```

Test coverage: the existing test pins the hand-written sentence directly under a comment asserting the opposite rule — services/.../tests/all/amend_benefit_application.rs:532-543:
```
    // `expected` is the clause the refusal must state, where the rule fixes it: the
    // envelope and the length wording are the ones this service answers every refused
    // request with (`service::constraint_envelope`).
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "no amendment at all",
            json!([]),
            Some(clause(
                "amendments",
                "Member must name at least one field to amend",
            )),
```
while the max bound in the same table (lines 625-636) is pinned with the central wording `Member must have length less than or equal to {MAX_AMENDMENTS_PER_REQUEST}`. `tests/all/member_refusal_one_construction.rs` (the crate's one-construction table) covers amend's unamendable-path and the association bound but never the empty-`Amendments` case, so no test states a reason for this member's min bound being worded differently.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as the lifecycle tests do
2. CreateBenefitApplication in catalog `AWS` for an ACTIVE benefit, note the returned Id and Revision
3. Call AmendBenefitApplication with Catalog=AWS, Identifier=<id>, Revision=<revision>, ClientToken="tok-min", AmendmentReason="why", Amendments=[]
4. Observe 400 ValidationException with Message `1 validation error detected: Value at 'amendments' failed to satisfy constraint: Member must name at least one field to amend`
5. Repeat with Amendments containing MAX_AMENDMENTS_PER_REQUEST + 1 (21) entries and observe `... Member must have length less than or equal to 20` — the wording `crate::service::constraint_requirement` produces
6. Compare: both are length bounds on the same member enforced by the same handler, yet only the second uses the helper the crate documents as the single wording for a handler-enforced length
