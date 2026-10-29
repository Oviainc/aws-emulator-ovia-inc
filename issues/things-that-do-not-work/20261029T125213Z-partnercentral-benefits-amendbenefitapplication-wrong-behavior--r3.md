Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# AmendBenefitApplication measures FieldPath in BYTES, bypassing the crate's single character-counted length check, so a legal non-ASCII details key is reported as naming no amendable field

_Filed: 2026-10-29T12:52:13Z_
_Operation: AmendBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Amendment$FieldPath` is an unconstrained `String` in the model, so this crate enforces its own bound (`state::MAX_FIELD_PATH_LEN`). The crate documents exactly one way to enforce a handler-side length — `state::length_clause`, which its own doc says exists so "a bound a handler enforces cannot be worded — or counted — two ways" and which counts "in CHARACTERS, as the walk counts a `@length` on a string". Every other handler-enforced string bound in the crate goes through it: amend's own `newValue` into the details document, amend's `amendmentReason` and `clientToken`, recall's `reason` and `clientToken`, and `state::file_name_of`.

`path_segments` does not. It compares `field_path.len()` — the UTF-8 BYTE length — against `MAX_FIELD_PATH_LEN`. Because `BenefitApplicationDetails` is an unconstrained `Document`, its member names are caller-chosen text, and this operation's own doc says such a key "is a name the CALLER chose and is addressed verbatim". A path into the details document whose key is written in a non-ASCII script therefore fails the bound at roughly a third of the characters an ASCII path gets: `BenefitApplicationDetails.` + 100 CJK characters is 126 characters but 326 bytes, so it is refused although the same 126 characters spelled in ASCII are accepted. The amendment the caller is entitled to make cannot be made, and the stored field stays unamendable forever.

Two compounding symptoms of the same line:
1. The bound is counted in a unit no other bound in the crate uses (`MAX_DETAILS_BYTES` is the one byte-counted guard and says so in its name; every `*_LEN` constant is char-counted through `length_clause`).
2. `path_segments` returns a bare `None` for all four of its failure modes, so an over-long path is refused with `unamendable()` — "Member must name one of the amendable fields (Name, Description, …)" — which misstates the reason: the path may name `BenefitApplicationDetails` perfectly well and fail only on length. No length clause is ever produced for this member.

## Evidence

The model leaves the member unconstrained, so nothing upstream enforces anything and the handler's own count is the only one — `smithy_query(query="shape",shape="Amendment")`:
```
"FieldPath": {"target": "String", "required": true, "doc": "<p>The JSON path or field identifier specifying which field in the benefit application to modify.</p>"}
```
and `UpdateBenefitApplicationInput$BenefitApplicationDetails` / `CreateBenefitApplicationInput$BenefitApplicationDetails` -> `{"target": "Document", "document": true}` — the document's member names are entirely caller-chosen, with no character-class restriction.

The byte count — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/amend_benefit_application.rs:266-274:
```
/// `None` for a path this service cannot read: one longer than
/// [`crate::state::MAX_FIELD_PATH_LEN`], one with an empty segment, one whose subscript is
/// unclosed, or one with more than [`crate::state::MAX_DETAILS_DEPTH`] segments.
/// `FieldPath` is an unconstrained `String`, so those two bounds are what keep the walk
/// below from recursing without limit and the path from being stored at arbitrary length.
fn path_segments(field_path: &str) -> Option<Vec<&str>> {
    if field_path.len() > crate::state::MAX_FIELD_PATH_LEN {
        return None;
    }
```

The rule it breaks — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:3135-3146:
```
/// Whether `value` satisfies the `@length(min: min, max: max)` of the member it is
/// written to or stored in.
///
/// The one length check every handler of this service makes for itself goes through, so a
/// bound a handler enforces cannot be worded — or counted — two ways. The requirement is
/// worded by [`crate::service::constraint_requirement`], the same words the centralized
/// walk uses for a length, and the length is counted in CHARACTERS, as the walk counts a
/// `@length` on a string.
///
/// `None` when the value satisfies the bound.
pub fn length_clause(value: &str, min: usize, max: usize) -> Option<String> {
    let length = value.chars().count();
```

The same function's own sibling bounds, which DO go through it — amend_benefit_application.rs:513-518 (the `NewValue` written into the same details document):
```
            if let Some(clause) = crate::state::length_clause(
                &amendment.new_value,
                0,
                crate::state::MAX_DETAILS_VALUE_LEN,
            ) {
                return Err(refused_value(clause));
```
and amend_benefit_application.rs:724-737 (`amendmentReason`, `clientToken`), and amend_benefit_application.rs:391-400:
```
fn unconstrained_member_clause(
    unconstrained: &[&str],
    member: &str,
    value: &str,
) -> Option<String> {
    if !unconstrained.contains(&member) {
        return None;
    }
    crate::state::length_clause(value, 0, crate::state::MAX_DETAILS_VALUE_LEN)
}
```

The constant is named and documented as a character-style `*_LEN` bound like the others, not as a byte guard — src/state.rs:348-354:
```
/// The longest `Amendment$FieldPath` this service reads.
///
/// The member is an unconstrained `String`, so this is what keeps a path (and the record
/// of it below) from being arbitrarily long. CHOSEN: wider than any path into a document
/// this service stores, since [`MAX_DETAILS_DEPTH`] also caps the segments a path may
/// have.
pub const MAX_FIELD_PATH_LEN: usize = 256;
```
(contrast src/state.rs:463-466, the one bound that IS bytes and says so: "The most bytes of text … `MAX_DETAILS_BYTES`".)

The caller-chosen-key doctrine this refusal defeats — amend_benefit_application.rs:261-264:
```
/// A subscript is read on the FIELD NAME of a LIST member ([`LIST_FIELDS`]) and nowhere
/// else, because that is where a list is addressed: on any other field name, and on every
/// segment below a field name, the brackets are part of the name — a key of the details
/// document is a name the CALLER chose and is addressed verbatim, brackets included.
```

The misstated refusal: `path_segments`' `None` is turned into the unamendable-field requirement — amend_benefit_application.rs:460-470 and :478:
```
    let unamendable = || {
        AmendmentRefusal::new(
            member_path(position, "fieldPath"),
            format!(
                "Member must name one of the amendable fields ({}), a `.`-separated path \
                 into {FIELD_DETAILS}, or a member of one element of \
                 {FIELD_PARTNER_CONTACTS} or {FIELD_FILE_DETAILS}",
                AMENDABLE_FIELDS.join(", ")
            ),
        )
    };
...
    let segments = path_segments(&amendment.field_path).ok_or_else(&unamendable)?;
```

Test coverage, and why no test justifies the divergence — tests/all/amend_benefit_application.rs:163-169 builds every boundary probe from ASCII only, so both the `MAX_FIELD_PATH_LEN` row (:713) and the `MAX_FIELD_PATH_LEN + 1` row (:629) have identical byte and character counts and stay green under either count:
```
/// A `FieldPath` into the details document whose whole text is exactly `length` bytes,
/// so each published `MAX_FIELD_PATH_LEN` bound and one past it can be sent.
fn details_path_of_length(length: usize) -> String {
    let prefix = "BenefitApplicationDetails.";
    let filler = length.saturating_sub(prefix.len());
    format!("{prefix}{}", "a".repeat(filler))
}
```
No test anywhere in the crate sends a non-ASCII `FieldPath`, and no comment states a reason for counting this one bound in bytes.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:90 does.
2. CreateBenefitApplication(Catalog='AWS', ClientToken='t0', BenefitIdentifier=<the seeded 'Marketing Development Funds' ben- id>, BenefitApplicationDetails={"CampaignName":"c","RequestedAmount":1}). Note the returned Id and Revision.
3. Build a FieldPath of 126 CHARACTERS whose key is non-ASCII: 'BenefitApplicationDetails.' followed by 100 repetitions of a 3-byte character (e.g. U+30AD). Its character length (126) is far inside MAX_FIELD_PATH_LEN=256; its UTF-8 length is 326 bytes.
4. Call AmendBenefitApplication(Catalog='AWS', ClientToken='t1', Identifier=<Id>, Revision=<Revision>, AmendmentReason='add a field', Amendments=[{FieldPath: <that path>, NewValue: 'v'}]).
5. Observe 400 ValidationException whose clause is 'Value at 'amendments.1.member.fieldPath' failed to satisfy constraint: Member must name one of the amendable fields (Name, Description, BenefitApplicationDetails, PartnerContacts, FileDetails), …' — the path DOES name BenefitApplicationDetails; it was refused only on length, and no length clause is reported.
6. Repeat step 4 with the same 126 characters spelled in ASCII ('BenefitApplicationDetails.' + 100 'a') and observe 200 plus the new field on GetBenefitApplication — the identical character count succeeds, so the bound a caller actually gets depends on the script the key is written in.
