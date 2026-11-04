Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8f194cc1fc794f910c53fc884e414c1833919c6c

# An associated resource ARN is persisted with no length at all, although the model gives `Arn` none and this crate states every persisted caller string carries one

_Filed: 2026-11-04T09:46:38Z_
_Operation: AssociateBenefitApplicationResource | Class: validation_

> _Note: this issue was identified against the code base at commit `8f194cc1fc794f910c53fc884e414c1833919c6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateBenefitApplicationResource$ResourceArn` targets the modelled `Arn`, a string that carries a `@pattern` whose tail is `(.+)$` and NO `@length`. The handler persists that caller string verbatim into `BenefitApplication::associated_resources` after checking only HOW MANY resources the record holds (`MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`) — never how long one of them is. `CreateBenefitApplication` stores the same member the same way (`distinct_values(input.associated_resources)`), and its own unmodelled-string check covers only the contact phone and the attachment use case.

That contradicts two rules this crate states about itself:

1. the store's `BOUNDED:` invariant, which claims that *every* caller string a record holds carries a length — the model's where it states one and this service's where it states none — "so no caller can grow this store without limit";
2. `state::unconstrained_member_clause`, which the crate documents as "The one function every path that writes such a member goes through" for exactly this class of value (a caller string the service PERSISTS that the model puts no length bound on), and which the create, the update and the amendment already apply to `Contact$Phone` and `FileInput$BusinessUseCase`.

So `associated_resources` is the one remaining persisted caller string of this record that no bound — modelled or this service's own — applies to: a single association can store a megabyte-long ARN, ten per application, in up to `MAX_APPLICATIONS_PER_CATALOG` applications per catalog, and `GetBenefitApplication`/`ListBenefitApplications` then report it. Nothing new is proposed here: the helper and its constant already exist in the crate and are already applied to the sibling members of this class (fixed issue `20261029T121710Z-partnercentral-benefits-createbenefitapplication-validation--r2` did precisely that for `Phone`/`BusinessUseCase`); what is missing is their application on the path that stores an ARN.

## Evidence

The model puts NO length on the member — smithy_query(query="shape",shape="Arn"): `{"shape_id": "com.amazonaws.partnercentralbenefits#Arn", "type": "string", "pattern": "^arn:aws:([a-zA-Z0-9\\-])+:([a-z]{2}(-gov)?-[a-z]+-\\d{1})?:(\\d{12})?:(.+)$"}` — and the input member targets it: smithy_query(query="operation",operation="AssociateBenefitApplicationResource") / OPERATIONS.md:37 — `ResourceArn` (string, pattern ^arn:aws:…:(.+)$, required)`. The pattern tail `(.+)` bounds nothing, so the centralized walk refuses no length here.

The crate's own invariant, broken by that field — src/state.rs:1547-1550:
    /// at most [`MAX_ALLOCATIONS_PER_CATALOG`] allocations, and every caller string a record
    /// holds carries a length — the model's own where it states one, and this service's where
    /// it states none ([`unconstrained_members_refusal`], [`details_requirement`]) — so no
    /// caller can grow this store without limit.

The single helper the crate says every such write goes through — src/state.rs:2677-2687:
    /// The requirement a caller string this service PERSISTS fails when the model puts no
    /// bound on it at all, or `None` when the value is one this service stores.
    …
    /// exactly like a modelled length. The one function every path that writes such a member
    /// goes through, so a value one path stores cannot be one another path refuses.
    pub fn unconstrained_member_clause(value: &str) -> Option<String> {
        length_clause(value, 0, MAX_DETAILS_VALUE_LEN)
    }

The handler that persists the ARN checks only the COUNT — src/ops/associate_benefit_application_resource.rs:287-306:
        if !application
            .associated_resources
            .iter()
            .any(|associated| associated == &input.resource_arn)
        {
            // The one request that GROWS this list, so the modelled bound on it is checked
            // here, before anything is written: …
            if application.associated_resources.len()
                >= crate::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION
            {
                return Err(record_full());
            }
            application
                .associated_resources
                .push(input.resource_arn.clone());
and its module header claims this is where the bound on the list is enforced — src/ops/associate_benefit_application_resource.rs:17-22: "This is the one operation that GROWS an application's association list, so it is also where the modelled bound on that list ([`crate::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`], the `@length(max: 10)` the model gives `AssociatedResources`) is enforced". There is no other check in the file: the only refusal it builds is `record_full()` (:159-172), the count one.

The stored field's doc bounds the COUNT only — src/state.rs:1143-1148:
    /// The AWS resources this application is linked to, as a SET: each distinct ARN once,
    /// in the order it was first stored, whether by the create (see [`distinct_values`])
    /// or by `AssociateBenefitApplicationResource`. At most
    /// [`MAX_ASSOCIATED_RESOURCES_PER_APPLICATION`] of them, which is therefore a count of
    /// distinct resources.
    pub associated_resources: Vec<String>,

The sibling write path drops the same member in unchecked — src/ops/create_benefit_application.rs:268-275 applies `unconstrained_members_refusal(input.partner_contacts…, input.file_details…)` (contacts and attachments only), while src/ops/create_benefit_application.rs:499 stores the ARNs with no length check at all:
            associated_resources: crate::state::distinct_values(input.associated_resources),

Test coverage: tests/all/unconstrained_member_bounds.rs:12-23 states the rule generally — "a caller string a write persists that carries no modelled constraint at all is accepted up to this service's own bound … and refused 400 `ValidationException` past it, with nothing written — on EVERY path that writes it" — but its rows span only "the two the model leaves unconstrained, `Contact$Phone` and `FileInput$BusinessUseCase`"; grep of that suite for `ResourceArn`/`AssociatedResources` returns no match, and tests/all/associate_benefit_application_resource.rs pins the set-union and count behaviour only. So nothing pins today's hole, and no comment, test or doc in the crate states a reason for excluding this member from the class.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement, as tests/all/benefit_application_lifecycle.rs's Harness::new does).
2. CreateBenefitApplication in catalog `AWS` against an ACTIVE benefit, and note the returned `Id`.
3. Call AssociateBenefitApplicationResource with `Catalog: "AWS"`, `BenefitApplicationIdentifier: <Id>` and `ResourceArn: "arn:aws:s3:us-east-1:123456789012:" + "x".repeat(100_000)` — the string satisfies the modelled `Arn` pattern (`(.+)$` accepts any tail) so the centralized walk passes it, and the handler stores it: src/ops/associate_benefit_application_resource.rs:302-304 pushes it with only the count checked at :297-301.
4. Call GetBenefitApplication for that `Id` and observe the 100 kB ARN reported back out of the store, and repeat step 3 nine more times with distinct over-long ARNs — the count bound stops the eleventh, having already let ~1 MB of caller text into one record.
5. Contrast with the same class of value on a sibling member: a create or update carrying `PartnerContacts: [{"Email": …, "Phone": "9".repeat(1001)}]` is refused 400 ValidationException with `Member must have length less than or equal to 1000` through state::unconstrained_member_clause (src/state.rs:2685-2687, called from src/ops/create_benefit_application.rs:268 and src/ops/update_benefit_application.rs:267), which src/state.rs:2683-2684 calls "The one function every path that writes such a member goes through".
6. Re-read src/state.rs:1547-1550: the store claims every caller string a record holds carries a length, "so no caller can grow this store without limit" — which `associated_resources` now falsifies.
