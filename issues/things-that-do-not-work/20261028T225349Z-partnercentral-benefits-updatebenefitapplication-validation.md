Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f08062a3928279f71031748dc453b175f9086c5b

# An update naming no member to change is accepted and still bumps Revision/UpdatedAt, although its siblings refuse such a request or leave the revision alone

_Filed: 2026-10-28T22:53:49Z_
_Operation: UpdateBenefitApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `f08062a3928279f71031748dc453b175f9086c5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every optional member of `UpdateBenefitApplicationInput` (`Name`, `Description`, `BenefitApplicationDetails`, `PartnerContacts`, `FileDetails`) may be omitted, so a request carrying only the four required members (`Catalog`, `ClientToken`, `Identifier`, `Revision`) reaches the handler. The handler applies nothing — `applied_members` stays empty — and then calls `application.revise(now)` UNCONDITIONALLY, so a request that wrote nothing still:

* bumps the application's `Revision` (and returns the bumped value), invalidating the revision every other holder of that record is carrying;
* advances `UpdatedAt`, so a caller comparing two reads sees a write that never happened;
* clears `recall_client_token`, silently closing the idempotency window a preceding `RecallBenefitApplication` opened (`state::BenefitApplication::revise` documents exactly that side effect); and
* consumes a slot of the bounded applied-write history with a record whose `fields` list is empty.

This contradicts this crate on both of the two ways it already handles a change-nothing request. `AmendBenefitApplication` — the sibling that writes the same caller-supplied members — REFUSES a request that names nothing to change, with its own `ValidationException`. `AssociateBenefitApplicationResource` and `DisassociateBenefitApplicationResource` accept such a request but explicitly guard the revise so that "the revision and `UpdatedAt` move only when the record actually changed", and `TagResource`/`UntagResource` leave both alone for the same stated reason. `UpdateBenefitApplication` is the one write path that neither refuses the request nor guards the write, which is also what `state::BenefitApplication::revise` says it must do ("EVERY call that CHANGES the stored record moves it").

## Evidence

The code: nothing is applied, yet the revision moves — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/update_benefit_application.rs:381-408:
        // Only the members the request carried are applied: an absent one leaves the
        // stored value alone, which is what lets a caller update one field without
        // having to resend the rest. Each one applied is named on the write record below.
        let mut applied_members: Vec<String> = Vec::new();
        if let Some(name) = input.name { ... }
        ... (Description, BenefitApplicationDetails, PartnerContacts, FileDetails)
        application.revise(now);

and the write record it then keeps with an empty field list — update_benefit_application.rs:417-426:
        application.record_write(crate::state::AppliedWrite {
            operation: Self::NAME,
            ...
            fields: applied_members,

The rule `revise` states for itself — src/state.rs:1200-1216:
    /// Record a change to the stored application: bump the revision and advance
    /// `UpdatedAt`.
    /// The revision is the application's optimistic-concurrency token ... so EVERY call
    /// that changes the stored record moves it ...
    /// Any such change also ENDS a recall's idempotency window ...
    pub fn revise(&mut self, now_millis: i64) {
        self.recall_client_token = None;
        self.revision = self.revision.saturating_add(1);
        self.touch(now_millis);
    }

Sibling 1 — the write that GUARDS the revise, with the rule stated in the comment — src/ops/associate_benefit_application_resource.rs:278-302:
        // A set union: the named ARN joins the associations the application already
        // carries ... The revision and `UpdatedAt` move only when the record actually changed, so a request
        // naming a resource this application is already linked to reports the revision it
        // already had.
        if !application.associated_resources.iter().any(...) {
            ...
            application.revise(now);
        }

Same rule, same shape — src/ops/disassociate_benefit_application_resource.rs:241-251:
        // ... The revision and `UpdatedAt`
        // move only when the record actually changed, so a request naming a resource this
        // application never carried reports the revision it already had.
        let associated_before = application.associated_resources.len();
        application.associated_resources.retain(...);
        if application.associated_resources.len() != associated_before {
            application.revise(now);
        }

Sibling 2 — the write that REFUSES a request naming nothing to change — src/ops/amend_benefit_application.rs:434-443:
        // An amendment request has to name what it amends, and no more fields than this
        // service applies in one request ...
        if input.amendments.is_empty() {
            return Err(refused(
                "amendments",
                "Member must name at least one field to amend",
            ));
        }

Test coverage: no test sends an update that names no member. tests/all/application_status_gates.rs:140-171 enumerates the update rows and every one carries at least one member ("The members `UpdateBenefitApplication` can change, one row each plus one row changing several at once"), and asserts only that "an accepted update must bump the revision" for those rows (application_status_gates.rs:192-198). The one "only the required members" row in the suite is a CREATE row — tests/all/benefit_application_lifecycle.rs:487-488, inside `create_then_get_round_trips_every_supplied_member`. So nothing pins the no-op update's answer or its effect on the record.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:86-91 does.
2. CreateBenefitApplication in catalog `AWS` for an ACTIVE benefit; keep the returned `Id` and `Revision` ("1").
3. GetBenefitApplication and note `Revision` and `UpdatedAt`.
4. Call UpdateBenefitApplication with ONLY the required members: {"Catalog":"AWS","ClientToken":"t1","Identifier":<Id>,"Revision":"1"} — no Name, Description, BenefitApplicationDetails, PartnerContacts or FileDetails.
5. Observe 200 with Revision "2". GetBenefitApplication now reports a bumped `Revision` and an advanced `UpdatedAt` while every reported member is byte-for-byte what the create stored — a write that changed nothing moved the optimistic-concurrency token, so any caller holding revision "1" is now refused 409.
6. Compare the two siblings on the same application: AssociateBenefitApplicationResource naming an ARN the application already carries answers 200 with the revision UNCHANGED (src/ops/associate_benefit_application_resource.rs:278-302), and AmendBenefitApplication with "Amendments": [] is refused 400 ValidationException "Member must name at least one field to amend" (src/ops/amend_benefit_application.rs:438-443).
7. Extra effect: RecallBenefitApplication a submitted application with ClientToken "r1", then send the no-op update above, then replay the recall with "r1" — the replay is no longer recognised as a retry, because the no-op update's `revise` cleared `recall_client_token` (src/state.rs:1209-1213).
