Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a669b06d661e5ead8877ca850e0fd26c45a4bbfe

# The amendment's Name/Description bounds are hand-copied numbers, while every other amendable member is checked by the schema walk the same file documents as the one enforcement point

_Filed: 2026-11-04T08:46:20Z_
_Operation: AmendBenefitApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a669b06d661e5ead8877ca850e0fd26c45a4bbfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`apply_amendment` enforces the target member's modelled constraints in TWO different ways, so the two copies can drift.

For a member of a `PartnerContacts` or `FileDetails` element the handler runs the framework's own constraint walk over the modelled shape (`member_requirement` -> `aws_smithy_schema_server::constraints::validate`), which is what the module header and that helper's doc both state the rule is: the value an amendment writes must satisfy exactly what the create or the update must supply, judged against the model.

For `Name` and `Description` the same handler instead re-states the model's numbers as local constants (`NAME_MAX_LEN = 255`, `DESCRIPTION_MAX_LEN = 1000`, `MIN_LEN = 1`) and checks them with `crate::state::length_clause`. The constants' own docs say they ARE the modelled `BenefitApplicationName` `@length(min: 1, max: 255)` and `BenefitApplicationDescription` `@length(min: 1, max: 1000)` — i.e. the file holds a second, hand-maintained copy of a rule the schema it already walks carries. Nothing ties the copy to the model: if either modelled `@length` moves, the centralized walk (which refuses a create's/update's `Name` and `Description`) follows it and the amendment path does not, and the amendment then accepts a value the create refuses or refuses one the create accepts, on the same stored member.

There is no behavioural divergence today (the copied numbers equal the modelled ones), which is why this is LOW: the defect is that one rule is implemented twice inside this one file, with only a comment keeping the copies equal.

Both refusals already render through the same wording helper (`crate::service::constraint_requirement`), so routing `Name`/`Description` through the same schema-driven check the sibling branches use leaves every message, status, reason and field list exactly as they are.

## Evidence

The module header states ONE enforcement point for an amendment's `NewValue` — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/amend_benefit_application.rs:44-47:
    //! `ValidationException`, as is a `NewValue` the member the path names is not modelled to
    //! accept: `Amendment$NewValue` is an unconstrained `String`, so the target member's own
    //! constraints are enforced here, by the same validating walk the centralized request
    //! validation runs.

Copy 1 — the schema walk, used for every `Contact`/`FileInput` member (src/ops/amend_benefit_application.rs:327-337):
    fn member_requirement(
        value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
        schema: &::aws_smithy_schema::Schema,
        member: &str,
    ) -> Option<String> {
        let path = ::aws_smithy_schema_server::constraints::path::member(member);
        ::aws_smithy_schema_server::constraints::validate(value, schema)
            .into_iter()
            .filter(|violation| violation.path == path)
            .find_map(|violation| crate::service::constraint_requirement(&violation.kind))
    }
its doc (src/ops/amend_benefit_application.rs:320-326): "by the SAME validating walk the centralized request validation runs (`aws_smithy_schema_server::constraints::validate` over the shape's own schema), so a value an amendment writes into a member must satisfy exactly what a value the create or the update supplied for it must."
called at src/ops/amend_benefit_application.rs:573 (`member_requirement(&amended, &Contact::SCHEMA, member)`) and :608 (`member_requirement(&supplied, &FileInput::SCHEMA, member)`).

Copy 2 — the same rule written out as numbers (src/ops/amend_benefit_application.rs:222-232):
    /// The longest value `Name` accepts: the modelled `BenefitApplicationName`
    /// `@length(min: 1, max: 255)`. `Amendment$NewValue` is an unconstrained `String`, so
    /// the target member's own constraint is enforced here.
    const NAME_MAX_LEN: usize = 255;
    /// The longest value `Description` accepts: the modelled
    /// `BenefitApplicationDescription` `@length(min: 1, max: 1000)`.
    const DESCRIPTION_MAX_LEN: usize = 1000;
    /// The shortest value either string member accepts, from those same `@length` traits.
    const MIN_LEN: usize = 1;
and applied at src/ops/amend_benefit_application.rs:505-521:
        FIELD_NAME if rest.is_empty() => {
            if let Some(clause) =
                crate::state::length_clause(&amendment.new_value, MIN_LEN, NAME_MAX_LEN)
            { return Err(refused_value(clause)); }
            ...
        FIELD_DESCRIPTION if rest.is_empty() => {
            if let Some(clause) = crate::state::length_clause(
                &amendment.new_value, MIN_LEN, DESCRIPTION_MAX_LEN,
            ) { return Err(refused_value(clause)); }

The model is the authority the copy restates — smithy_query(query="operation",operation="UpdateBenefitApplication"): `"Name": {"target": "BenefitApplicationName", "length": {"min": 1, "max": 255}}`, `"Description": {"target": "BenefitApplicationDescription", "length": {"min": 1, "max": 1000}}`.

Why responses cannot change: both paths word the requirement with the one helper — src/service.rs:85-90:
    ViolationKind::MinLength(min) => { format!("Member must have length greater than or equal to {min}") }
    ViolationKind::MaxLength(max) => { format!("Member must have length less than or equal to {max}") }
which `length_clause` reaches through `count_length_clause` (src/state.rs:3481-3505) and `member_requirement` reaches directly (line 336 above).

Test coverage that pins the current answers byte for byte: tests/all/handler_enforced_length_wording.rs:36-47 (its own `NAME_MAX_LEN`/`DESCRIPTION_MAX_LEN`/`STRING_MIN_LEN`) with the rows at :162-194 — "an empty value for Name" -> `min_length(1)`, "a Name one character past its maximum" -> `max_length(255)`, "a multi-byte Description one character past its maximum" -> `max_length(1000)`, each asserted as `one(&clause("amendments.1.member.newValue", expected))`; plus tests/all/amend_benefit_application.rs and tests/all/amend_caller_supplied_members.rs for the contact/file branches.

## Steps to Reproduce

1. Open services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/amend_benefit_application.rs and read the module header at :44-47: an amendment's `NewValue` is to be judged "by the same validating walk the centralized request validation runs".
2. Read `member_requirement` at :327-337 and its two call sites at :573 and :608 — the `Contact` and `FileInput` members are judged against `Contact::SCHEMA` / `FileInput::SCHEMA`, so their bounds come from the model.
3. Read :222-232 and :505-521 — `Name` and `Description` are judged against local `NAME_MAX_LEN`/`DESCRIPTION_MAX_LEN`/`MIN_LEN` literals instead, which the constants' own docs identify as the modelled `BenefitApplicationName` / `BenefitApplicationDescription` `@length` traits.
4. Confirm with smithy_query(query="operation",operation="UpdateBenefitApplication") that those two members' lengths live in the model (1..255 and 1..1000), so the handler holds a second copy of a value the schema it already walks carries.
5. Simulate the drift: change either modelled `@length` in the Smithy model. The centralized walk refuses a create's/update's `Name` against the new bound while the amendment path keeps refusing against the literal, so the two write paths disagree about the same stored member — and no test fails, because tests/all/handler_enforced_length_wording.rs:39-43 keeps its own copy of the numbers too.
6. Verify a fix is response-neutral: both paths word a length violation through crate::service::constraint_requirement (src/service.rs:85-90), so a schema-driven check reports the identical `Member must have length less than or equal to 255` clause the rows at tests/all/handler_enforced_length_wording.rs:174-194 assert.
