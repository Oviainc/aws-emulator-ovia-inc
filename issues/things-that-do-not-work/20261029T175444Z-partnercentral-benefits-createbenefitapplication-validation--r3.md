Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# A BenefitApplicationDetails leaf longer than MAX_DETAILS_VALUE_LEN is persisted by create and update, while the amendment that writes the same leaf refuses it

_Filed: 2026-10-29T17:54:44Z_
_Operation: CreateBenefitApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BenefitApplicationDetails` is an unconstrained `Document`, so the crate bounds the caller text it persists itself. For a LEAF of that document the crate owns exactly one bound, `state::MAX_DETAILS_VALUE_LEN` (1000 characters), and `AmendBenefitApplication` applies it to every value an amendment writes into the document. `CreateBenefitApplication` and `UpdateBenefitApplication`, which store a whole document the caller supplied, apply only `state::details_refusal` — whose three rules are nesting depth, total member count and TOTAL text size (64 KiB) — so neither bounds an individual leaf at all.

The result is that the two paths disagree about the same value on the same stored member:

* a create (or update) carrying `{"CampaignName": <1001 chars>}` is accepted, stored, and reported by `GetBenefitApplication`;
* an amendment writing that very same value to that very same leaf (`FieldPath: "BenefitApplicationDetails.CampaignName"`) is refused 400 `ValidationException` with `Member must have length less than or equal to 1000`.

So the service reports a value it refuses to write, and a no-op amendment that re-sends the stored value is refused. That is the exact divergence the crate states must not exist for this class of value (a caller string the model constrains not at all that the record persists), and the exact divergence that was already treated as a defect — and fixed — for the sibling pair `PartnerContacts[].Phone` / `FileDetails[].BusinessUseCase` (fixed issue `20261029T121710Z-partnercentral-benefits-createbenefitapplication-validation--r2`: "persisted unbounded by create and update, while the amendment that writes the same two members" bounds them). The leaf of the details document is the one remaining member of that class where only the amendment path enforces the bound. The description does not propose a bound: the constant and the single length helper already exist in the crate; what is missing is their application on the path that stores the document.

## Evidence

The amendment applies the per-leaf bound — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/amend_benefit_application.rs:513-522:
    // A leaf of the document carries no modelled length at all (and an empty
    // string is a valid member of a document), so the only bound on a value
    // written here is this service's own maximum.
    if let Some(clause) = crate::state::length_clause(
        &amendment.new_value,
        0,
        crate::state::MAX_DETAILS_VALUE_LEN,
    ) {
        return Err(refused_value(clause));
    }

The constant states the rule as a property of a LEAF of the document, not of one operation — src/state.rs:356-363:
    /// The longest value an amendment may write into `BenefitApplicationDetails`.
    ///
    /// `Amendment$NewValue` is an unconstrained `String`, while the members a path can name
    /// on the record itself carry the model's own `@length` ...
    /// This is the matching bound for a leaf of
    /// the document, which the model constrains not at all: the longest string member the
    /// model lets a caller store on this resource.
    pub const MAX_DETAILS_VALUE_LEN: usize = 1000;

What the create/update path actually enforces on a supplied document — src/state.rs:2323-2341 (`details_requirement`): `document_fits(document, MAX_DETAILS_DEPTH)`, then `members > MAX_DETAILS_MEMBERS`, then `bytes > MAX_DETAILS_BYTES`. There is no per-leaf length among them, yet src/state.rs:2355-2357 claims one rule for all three writers:
    /// Every operation that writes the member applies this — the create and the update to the
    /// document they were handed, `AmendBenefitApplication` to the document its amendments
    /// produced — so one rule is refused in one form wherever it holds.

The create's only check on the supplied document — src/ops/create_benefit_application.rs:249-256:
        if let Some(refusal) = crate::state::details_refusal(
            crate::state::DETAILS_MEMBER,
            input.benefit_application_details.as_ref(),
        ) {
and the update's, identically — src/ops/update_benefit_application.rs:244-249.

The principle the crate states for exactly this class of persisted, unmodelled caller string — src/state.rs:2623-2626 (`unconstrained_members_refusal`):
    /// Applied by every operation that supplies these lists — the create and the update to the
    /// lists they were handed, `AmendBenefitApplication` to the single value an amendment
    /// writes — so one rule refuses one value in one form wherever it holds, and no read can
    /// report a value no other write path would accept.

Test coverage: tests/all/details_document_guard.rs:129-137 builds documents whose leaves are exactly `MAX_DETAILS_VALUE_LEN` characters (accepted on every path, so the divergence is not reached), and tests/all/details_document_guard.rs:443-460 pins the amendment against the member-count and byte bounds only. No test sends a create, an update or an amendment with a leaf PAST `MAX_DETAILS_VALUE_LEN`, so nothing pins today's asymmetry.

## Steps to Reproduce

1. Boot the emulator, grant the caller's account the partner entitlement (state::grant_partner_entitlement, as tests/all/benefit_application_lifecycle.rs's Harness::new does), and read an ACTIVE benefit of catalog `AWS` whose BenefitRequestSchema declares a string field (the seeded "Marketing Development Funds" declares `CampaignName` string, `RequestedAmount` number).
2. Call CreateBenefitApplication with that BenefitIdentifier and `BenefitApplicationDetails: {"CampaignName": "v" x 1001, "RequestedAmount": 2500}`. It answers 200: the document passes the schema (the leaf is still a `string`), the depth (2), the member count (2) and the 64 KiB text bound, and nothing checks the leaf's own length.
3. Call GetBenefitApplication for the returned Id and observe the 1001-character `CampaignName` reported back out of the store.
4. Call AmendBenefitApplication for that application with `Amendments: [{"FieldPath": "BenefitApplicationDetails.CampaignName", "NewValue": <the same 1001-character value just read back>}]` and the current Revision. It answers 400 ValidationException naming `amendments.1.newValue` with `Member must have length less than or equal to 1000` (src/ops/amend_benefit_application.rs:513-522), so the service refuses to write the value it is reporting, and the no-op re-write cannot be made at all.
5. Repeat step 2 through UpdateBenefitApplication on a PENDING_SUBMISSION application (src/ops/update_benefit_application.rs:244-249) for the same result on the other path that stores a supplied document.
