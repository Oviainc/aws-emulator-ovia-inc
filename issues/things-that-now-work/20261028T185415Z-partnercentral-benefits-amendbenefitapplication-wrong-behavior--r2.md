Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 293c15726352cf981422bfe1d7cee5f6a5389f12

# The amendable-field set omits PartnerContacts and FileDetails, the two caller-supplied members its own rule includes, so neither can be changed once the application is submitted

_Filed: 2026-10-28T18:54:15Z_
_Operation: AmendBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `293c15726352cf981422bfe1d7cee5f6a5389f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AmendBenefitApplication` defines the fields a `FieldPath` may name by a rule written twice in its own module: the amendable members are "the members a caller supplies and `UpdateBenefitApplication` also writes", and "everything else on the record is service-owned". The set it actually implements is only `Name`, `Description` and `BenefitApplicationDetails`.

`UpdateBenefitApplication` writes two further members straight from caller input: `PartnerContacts` and `FileDetails` (its own `MEMBER_PARTNER_CONTACTS` / `MEMBER_FILE_DETAILS` constants, applied to `application.partner_contacts` and `application.file_details`). Neither is service-owned — unlike the application's identity, its status and the programs of its benefit, which the comment names as the examples of what is. Yet a `FieldPath` naming either is answered 400 `ValidationException` with the clause "Member must name one of the amendable fields (Name, Description, BenefitApplicationDetails), or a `.`-separated path into BenefitApplicationDetails", i.e. refused as if it were service-owned.

The consequence is a hole the crate's own docs say is not there. `UpdateBenefitApplication` is accepted only while the application is `PENDING_SUBMISSION` (`crate::state::UPDATABLE_STATUSES`) and its header sends the caller to this operation for anything later: "A submitted application is changed through `RecallBenefitApplication` (back to draft) or `AmendBenefitApplication` instead." So for an `IN_REVIEW` / `ACTION_REQUIRED` application the update is refused 409 by the status gate and the amendment is refused 400 by the field gate: the contacts and the attached files of a submitted application cannot be changed through any operation, although both are members the caller supplied in the first place. The model does not narrow the set either — `Amendment$FieldPath` is documented as "The JSON path or field identifier specifying which field in the benefit application to modify", with no enumeration.

## Evidence

The rule, stated in the handler header — src/ops/amend_benefit_application.rs:26-31:
    //! `Name`, `Description`, or `BenefitApplicationDetails` followed by a `.`-separated
    //! path into that document — each optionally written in JSON-path form (`$.Name`).
    //! Those are the members a caller supplies and `UpdateBenefitApplication` also writes;
    //! everything else on the record is service-owned, so a path naming one is refused with
    //! the modelled `ValidationException`, as is a `NewValue` outside the modelled length of
    //! the member the path names.

and again over the constant — src/ops/amend_benefit_application.rs:142-147:
    /// The members of a benefit application an amendment may write.
    ///
    /// They are the members a caller supplies and `UpdateBenefitApplication` also writes;
    /// everything else on the record is service-owned (its identity, its status, the
    /// programs of the benefit it requests) and no amendment reaches it.
    const AMENDABLE_FIELDS: &[&str] = &[FIELD_NAME, FIELD_DESCRIPTION, FIELD_DETAILS];

Everything outside those three is refused — src/ops/amend_benefit_application.rs:263-272 and :339:
        let unamendable = || {
            AmendmentRefusal::new(
                member_path(position, "fieldPath"),
                format!(
                    "Member must name one of the amendable fields ({}), or a `.`-separated path \
                     into {FIELD_DETAILS}",
                    AMENDABLE_FIELDS.join(", ")
                ),
            )
        };
...
            _ => return Err(unamendable()),

The sibling the rule is defined against writes two more caller-supplied members — src/ops/update_benefit_application.rs:122-129:
    /// The input members an update may apply, as the model spells them on the wire. Each is
    /// recorded on the applied-write record of the application it changed, so a stored write
    /// says which members that request wrote.
    const MEMBER_NAME: &str = "Name";
    const MEMBER_DESCRIPTION: &str = "Description";
    const MEMBER_DETAILS: &str = "BenefitApplicationDetails";
    const MEMBER_PARTNER_CONTACTS: &str = "PartnerContacts";
    const MEMBER_FILE_DETAILS: &str = "FileDetails";

and applies them from the request — src/ops/update_benefit_application.rs:397-407:
        if let Some(contacts) = input.partner_contacts {
            application.partner_contacts = contacts;
            applied_members.push(MEMBER_PARTNER_CONTACTS.to_string());
        }
        if let Some(files) = input.file_details {
            application.file_details = files
                .iter()
                .map(|file| crate::state::file_detail(file, &principal, now))
                .collect();
            applied_members.push(MEMBER_FILE_DETAILS.to_string());
        }

while refusing to apply anything at all outside a draft — src/ops/update_benefit_application.rs:7-12 and :355-366:
    //! An update edits the partner's own DRAFT: it is accepted only while the application is
    //! `PENDING_SUBMISSION` ([`crate::state::UPDATABLE_STATUSES`]) ... A submitted application is
    //! changed through `RecallBenefitApplication` (back to draft) or
    //! `AmendBenefitApplication` instead.
...
        if !application.is_updatable() {
            return Err(UpdateBenefitApplicationErrorModeled::ConflictException(
...
with src/state.rs:176:
    pub const UPDATABLE_STATUSES: &[&str] = &[STATUS_PENDING_SUBMISSION];

The model puts no enumeration on the member — smithy_query(query="shape", shape="com.amazonaws.partnercentralbenefits#Amendment"), member `FieldPath`:
    "The JSON path or field identifier specifying which field in the benefit application to modify."

Test coverage: no test pins either member as unamendable. tests/all/amend_benefit_application.rs:535-636 enumerates the refused paths and every row names a genuinely service-owned member or a malformed path — `Status` (:544), `Id` (:547), `Programs` (:596), `Name.First` (:550), an empty path (:551), an empty segment (:554) — and `PartnerContacts`/`FileDetails` appear in no amend row (grep over tests/ finds them only in create/update rows). The update side, by contrast, calls them exactly what the amend rule calls amendable — tests/all/application_status_gates.rs:140-142: "The members `UpdateBenefitApplication` can change, one row each plus one row changing several at once (the operation's input: `Name`, `Description`, `BenefitApplicationDetails`, `PartnerContacts`, `FileDetails`)."

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:90 does.
2. CreateBenefitApplication in catalog `AWS` for an ACTIVE benefit, carrying PartnerContacts: [{"Email": "ada@example.com"}] and FileDetails: [{"FileURI": "s3://b/first.csv"}]; keep the returned Id and Revision.
3. SubmitBenefitApplication for that Id → the application is IN_REVIEW (so UpdateBenefitApplication is now refused 409 by src/ops/update_benefit_application.rs:355).
4. AmendBenefitApplication with Catalog=AWS, Identifier=<Id>, the current Revision, a ClientToken, AmendmentReason="new contact", Amendments=[{"FieldPath": "PartnerContacts", "NewValue": "grace@example.com"}]. Observe 400 ValidationException whose clause is "Member must name one of the amendable fields (Name, Description, BenefitApplicationDetails), or a `.`-separated path into BenefitApplicationDetails" — the refusal reserved for members the handler calls service-owned, although src/ops/update_benefit_application.rs:397-399 writes this member straight from caller input.
5. Repeat with FieldPath="FileDetails" (and with a path below either member) for the same refusal.
6. GetBenefitApplication for that Id: PartnerContacts and FileDetails still carry the create's values, and no operation can change them — the update answers 409 on status, the amendment 400 on the field gate.
