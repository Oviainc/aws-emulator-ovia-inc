Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# The association-bound refusal names `associatedResources`, a member AssociateBenefitApplicationResource does not bind

_Filed: 2026-11-07T16:53:48Z_
_Operation: AssociateBenefitApplicationResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When an application already holds `MAX_ASSOCIATED_RESOURCES_PER_APPLICATION` resources, `AssociateBenefitApplicationResource` refuses the request with this service's modelled `ValidationException` built against the member name `associatedResources`. That operation binds exactly three input members — `Catalog`, `BenefitApplicationIdentifier`, `ResourceArn` — and none of them is `associatedResources`, which is a member of `CreateBenefitApplicationInput` / `GetBenefitApplicationOutput`.

So both halves of the refusal a client reads — the clause inside `Message` (`Value at 'associatedResources' failed to satisfy constraint: Member must have length less than or equal to 10`) and `ValidationExceptionField$Name` — name a field the request never sent and that the caller cannot correct. This contradicts the documented contract of the single construction the call goes through (`state::member_validation_exception`, whose `member` argument is documented as "the input member as the wire spells it"), and it contradicts the rule this crate's own suite states over the whole class of in-handler refusals (`tests/all/refusal_member_path.rs`): a refusal names the member by the path the centralized constraint walk reports for it — and the walk reports no path for a member this operation has no schema slot for.

Note that the BOUND itself is modelled (`AssociatedResources` carries `@length(max: 10)` on both the create input and the get output), so the number and the requirement wording are not in question; only the member the refusal attributes it to is. The handler already has the member that carried the ARN which would not fit (`resourceArn`), and that is the one input member of this request a caller could act on.

## Evidence

The operation binds no `associatedResources` member — `smithy_query(query="operation",operation="AssociateBenefitApplicationResource")`:

    "input": {"Catalog": {...}, "BenefitApplicationIdentifier": {...}, "ResourceArn": {"target": "Arn", "length": {"min": 0, "max": 1600}, "required": true, ...}}

and `services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/OPERATIONS.md:37`:

    - **Input:** `Catalog` (string, pattern ^[A-Za-z0-9_-]+$, required), `BenefitApplicationIdentifier` (string, pattern ^(arn:.+|benappl-[0-9a-z]{14})$, required), `ResourceArn` (string, pattern ^arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)$, required)

The handler nevertheless refuses against that name — `src/ops/associate_benefit_application_resource.rs:144-148`:

    /// The member a refusal of this operation's association bound names: the application's own
    /// `AssociatedResources` list, spelled as the wire spells it, because that is the list the
    /// request would take past its modelled length — the same member a create carrying too
    /// many resources is refused on.
    const ASSOCIATED_RESOURCES_MEMBER: &str = "associatedResources";

and `src/ops/associate_benefit_application_resource.rs:159-172`:

    fn record_full() -> AssociateBenefitApplicationResourceError {
        let requirement = crate::service::constraint_requirement(&ViolationKind::MaxLength(
            crate::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION as i64,
        ))
        .unwrap_or_default();
        AssociateBenefitApplicationResourceErrorModeled::ValidationException(
            crate::state::member_validation_exception(
                ASSOCIATED_RESOURCES_MEMBER,
                &requirement,
                crate::state::VALIDATION_REASON_FIELD,
            ),
        )
        .into()
    }

reached from `src/ops/associate_benefit_application_resource.rs:297-301`:

            if application.associated_resources.len()
                >= crate::state::MAX_ASSOCIATED_RESOURCES_PER_APPLICATION
            {
                return Err(record_full());
            }

The contract that call breaks — `src/state.rs:3521-3525` (`member_validation_exception`):

    /// `member` is the input member as the wire spells it, optionally followed by the steps a
    /// handler addressed below it; the PATH the clause and the `ValidationExceptionField$Name`
    /// state is spelled here by [`crate::service::walk_member_path`], so one input member is
    /// named one way whether it was refused by the centralized walk or by a handler's own
    /// rule.

and the rule this crate's own suite states over the class — `tests/all/refusal_member_path.rs:12-17`:

    //! **A refusal of an input member names that member by the path the centralized constraint
    //! walk reports for it — the member in lowerCamel, followed by the steps a handler
    //! addressed below it — in the clause of the AWS validation envelope and in
    //! `ValidationExceptionField$Name` alike.** So one input member has ONE path whichever
    //! bound it broke: a constraint the model declares, which the centralized walk enforces, or
    //! a rule a handler enforces itself.

Test coverage: that suite's "every refusal a HANDLER of this service decides about a member" table (`tests/all/refusal_member_path.rs:264-350`) carries rows only for the three listings' `NextToken`, the tagging/create tag keys, the create's details guard and the create's withdrawn benefit — no row for this operation, so nothing pins the member name it reports. The association bound itself is exercised in `tests/all/associate_benefit_application_resource.rs`.

## Steps to Reproduce

1. Grant the emulator account the partner entitlement (`state::grant_partner_entitlement`), since every writing operation of this service is behind that gate.
2. `CreateBenefitApplication` in catalog `AWS` for a seeded ACTIVE benefit, with `AssociatedResources` holding `MAX_ASSOCIATED_RESOURCES_PER_APPLICATION` (10) distinct ARNs (`src/state.rs:489`).
3. Call `AssociateBenefitApplicationResource` with `Catalog: "AWS"`, the application's `Id` as `BenefitApplicationIdentifier`, and one further `ResourceArn` the record does not already carry.
4. Observe the 400 `ValidationException`: `Message` is `1 validation error detected: Value at 'associatedResources' failed to satisfy constraint: Member must have length less than or equal to 10` and `FieldList[0].Name` is `associatedResources` — a member this request has no slot for, instead of `resourceArn`, the one input member that carried the ARN which did not fit.
