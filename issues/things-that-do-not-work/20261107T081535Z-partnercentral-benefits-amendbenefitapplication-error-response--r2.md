Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f7aa62a5e729dafdbaba865d00552878247de874

# Amendment document refusals name `benefitApplicationDetails`, an input member AmendBenefitApplication does not bind

_Filed: 2026-11-07T08:15:35Z_
_Operation: AmendBenefitApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `f7aa62a5e729dafdbaba865d00552878247de874`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AmendBenefitApplication` carries no `BenefitApplicationDetails` input member — a caller reaches the details document only through `Amendments[i].FieldPath` / `Amendments[i].NewValue`. Yet the handler's two document-level refusals (the resource guard and the benefit request-schema check) are raised against `crate::state::DETAILS_MEMBER` ("BenefitApplicationDetails"), so the modelled `ValidationException` this operation answers carries a member path — in `Message` and in `ValidationExceptionField$Name` — for a member the request cannot have sent.

This contradicts two statements this crate makes about that argument:

1. `state::details_refusal` documents its `member` argument as "the input member the document arrived on ... so the clause names the member the caller must fix". On the amend path the document arrived on `Amendments`, and what the caller must fix is an amendment element — which is exactly what every OTHER refusal of this handler names, through the handler's own `member_path`/`refused` constructions (`amendments.1.member.fieldPath`, `amendments.1.member.newValue`).

2. `state::SchemaViolation::member` documents the reported path as "the same path form `AmendBenefitApplication` addresses a field of the document with, so a caller can act on it". It is not: `member_validation_exception` runs the path through `service::walk_member_path`, which re-cases the leading input member through the framework's spelling (input member `Key` is reported as `...member.key`, per the sibling tagging handler's own doc), so the schema refusal reports `benefitApplicationDetails.<field>`. Fed back as an amendment `FieldPath`, that lower-cased head does not equal `FIELD_DETAILS` ("BenefitApplicationDetails"), so `apply_amendment` refuses it as naming none of the amendable fields — the caller literally cannot act on the path it was handed.

The handler already has the single construction that names an amendment element (`refused(&refusal.member, &refusal.clause)` with `member_path(position, ...)`); these two refusals are the only ones on the path that bypass it.

## Evidence

The operation binds no such member — `services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/OPERATIONS.md:30`:

    - **Input:** `Catalog` (string, pattern ^[A-Za-z0-9_-]+$, required), `ClientToken` (string, required), `Revision` (string, required), `Identifier` (string, pattern ^(arn:.+|benappl-[0-9a-z]{14})$, required), `AmendmentReason` (string, required), `Amendments` (list<Amendment>, required)

(`smithy_query(query="operation",operation="AmendBenefitApplication")` agrees: input members are Catalog, ClientToken, Revision, Identifier, AmendmentReason, Amendments.)

Both refusals nevertheless name the details member — `src/ops/amend_benefit_application.rs:879-884`:

    if let Some(refusal) = crate::state::details_refusal(
        crate::state::DETAILS_MEMBER,
        amended.benefit_application_details.as_ref(),
    ) {
        return Err(AmendBenefitApplicationErrorModeled::ValidationException(refusal).into());
    }

and `src/ops/amend_benefit_application.rs:889-894`:

    if let Some(refusal) = crate::state::benefit_schema_refusal(
        benefit_schema.as_ref(),
        amended.benefit_application_details.as_ref(),
    ) {
        return Err(AmendBenefitApplicationErrorModeled::ValidationException(refusal).into());
    }

where `src/state.rs:2497-2499` fixes that name to a member of OTHER operations:

    /// The input member a refusal of an application-details document names, spelled as the
    /// wire spells it.
    pub const DETAILS_MEMBER: &str = "BenefitApplicationDetails";

The contract the call breaks — `src/state.rs:2433-2434` (`details_refusal`):

    /// `member` is the input member the document arrived on, spelled as the wire spells it,
    /// so the clause names the member the caller must fix.

and `src/state.rs:2517-2519` (`SchemaViolation::member`):

    /// The input member, followed by the document's own `.`-separated path to the field
    /// that does not satisfy the schema — the same path form `AmendBenefitApplication`
    /// addresses a field of the document with, so a caller can act on it.

The same handler's own discipline for every other refusal — `src/ops/amend_benefit_application.rs:250-256`:

    /// The path of one member of `Amendments[position]`, spelled the way this service's
    /// validation messages spell a member path ... e.g.
    /// `amendments.1.member.fieldPath`.
    fn member_path(position: usize, member: &str) -> String {
        crate::state::element_member_path(AMENDMENTS_MEMBER, position, member)
    }

and the re-casing that makes the reported path unusable as a `FieldPath` — `src/state.rs:3531` inside `member_validation_exception`:

    let path = crate::service::walk_member_path(member);

`src/service.rs:144-149`:

    pub fn walk_member_path(member: &str) -> String {
        let spell = ::aws_smithy_schema_server::constraints::path::member;
        match member.split_once('.') {
            Some((head, below)) => format!("{}.{below}", spell(head)),
            None => spell(member),
        }

that `spell` lower-cases the input member, per this crate's own sibling doc `src/ops/tag_resource.rs:21-23`:

    //! `Key` member of one element of `tags`, so the refusal names that member's own full path
    //! (`tags.<position>.member.key`), which is the path the centralized walk reports for a
    //! violation there.

while the amendable-field match requires the capitalised spelling — `src/ops/amend_benefit_application.rs:158` and `:171-177`:

    const FIELD_DETAILS: &str = "BenefitApplicationDetails";
    const AMENDABLE_FIELDS: &[&str] = &[
        FIELD_NAME,
        FIELD_DESCRIPTION,
        FIELD_DETAILS,
        FIELD_PARTNER_CONTACTS,
        FIELD_FILE_DETAILS,
    ];

so `benefitApplicationDetails.<field>` falls through `apply_amendment`'s match to `_ => return Err(unamendable())` (`src/ops/amend_benefit_application.rs:631`).

Test coverage: the amend refusal paths are pinned in `tests/all/amend_benefit_application.rs`, `tests/all/refusal_member_path.rs`, `tests/all/member_refusal_one_construction.rs`, `tests/all/field_path_length_bound.rs` and `tests/all/details_document_guard.rs`; the member-path spelling rule is pinned in `tests/all/refusal_member_path.rs`. None of them asserts that an amend-path document refusal names a member of a DIFFERENT operation.

## Steps to Reproduce

1. Grant the emulator account the partner entitlement (`state::grant_partner_entitlement`) and `CreateBenefitApplication` in catalog `AWS` for the seeded benefit "Promotional Credits", whose seeded `BenefitRequestSchema` declares `RequestedCredits` with JSON type `number` (`src/state.rs:564-571`), supplying `BenefitApplicationDetails = {"RequestedCredits": 100}`.
2. Call `AmendBenefitApplication` with the application's `Revision`, a fresh `ClientToken`, an `AmendmentReason`, and `Amendments = [{"FieldPath": "BenefitApplicationDetails.RequestedCredits", "NewValue": "many"}]` — `Amendment$NewValue` is a String, so the amended document puts a string where the schema declares a number.
3. Observe the 400 `ValidationException`: `Message` reads `1 validation error detected: Value at 'benefitApplicationDetails.RequestedCredits' failed to satisfy constraint: Member must be of type number`, and `FieldList[0].Name` is `benefitApplicationDetails.RequestedCredits` — a member the request never carried, instead of `amendments.1.member.newValue`, which every other refusal of this handler names.
4. Copy that reported path back into a new amendment as `FieldPath = "benefitApplicationDetails.RequestedCredits"` and observe a second `ValidationException` saying the member must name one of the amendable fields — so the path the first refusal handed back cannot be acted on, contrary to the `SchemaViolation::member` doc.
5. For the resource-guard variant, amend with a `FieldPath` that nests the stored document past `MAX_DETAILS_DEPTH` (`src/state.rs:524`) and observe the refusal named against `benefitApplicationDetails` alone.
