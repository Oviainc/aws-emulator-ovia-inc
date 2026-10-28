Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The invalid-NextToken refusal skips the validation envelope and names the member 'NextToken' where the walk names 'nextToken'

_Filed: 2026-10-28T05:34:16Z_
_Operation: ListBenefits | Class: error-response_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three paginated listings refuse a `NextToken` this service never issued with a hand-built `ValidationException` whose `Message` is a bare sentence (`"NextToken is not a pagination token issued by ListBenefits"`) and whose `FieldList[0].Name` is the PascalCase `NextToken`.

Two rules of this crate are broken by that one construction:

1. **The envelope.** This crate routes every in-handler refusal through `service::member_clause` + `service::validation_envelope` so that, in the words of that helper, "an in-handler refusal reads exactly like one the centralized walk produced". The crate's own service-wide rule-1 invariant checker asserts that EVERY refusal's `Message` starts with the AWS envelope (`N validation error(s) detected: `) and that every clause inside it starts with `Value at '`. The three `invalid_next_token()` refusals satisfy neither, so a client that parses a clause to learn which member and which requirement failed can read every other refusal of this service but not these three.

2. **The member path.** The member path a handler puts in the clause and in `ValidationExceptionField$Name` is documented in this crate as "members in lowerCamel ... (see `smithy_schema_server::constraints::path`, which spells the paths the centralized walk reports)", and that is what the sibling in-handler refusals use (`amendments`, `amendments.1.member.newValue`, `amendmentReason`, `clientToken`, `associatedResources`, `tags`, `tagKeys`). For this very member the walk reports `nextToken` — pinned by the crate's own test — so one operation answers two different member paths for one input member depending on which bound it broke: `nextToken` for the modelled `@length`, `NextToken` for the service's own cursor rule.

The same construction is copied in all three listings (`ListBenefits`, `ListBenefitApplications`, `ListBenefitAllocations`), so a fix has three call sites. The same PascalCase choice also appears at `CreateBenefitApplication`'s `reserved_tag_key_refusal("Tags", ..)` and at the `"BenefitApplicationDetails"` field of the document-guard refusals, where the walk reports `tags` and `benefitApplicationDetails` respectively — whichever spelling is adopted, the handler-filled name should be the one the walk reports for the same member.

## Evidence

The refusal, identical in the three list handlers — src/ops/list_benefits.rs:122-135:
    /// Why a `NextToken` that is not one this operation issued is refused.
    fn invalid_next_token() -> ListBenefitsError {
        let message = "NextToken is not a pagination token issued by ListBenefits";
        ListBenefitsErrorModeled::ValidationException(ValidationException {
            message: message.to_string(),
            reason: crate::state::VALIDATION_REASON_FIELD.to_string(),
            field_list: Some(crate::state::validation_field(
                "NextToken",
                message,
                crate::state::VALIDATION_CODE_INVALID_VALUE,
            )),
        })
        .into()
    }
(used at src/ops/list_benefits.rs:215-221; the same function at src/ops/list_benefit_applications.rs:127-140 used at :226-232, and at src/ops/list_benefit_allocations.rs:128-141 used at :261-267)

The rule it skips — src/service.rs:267-272:
    /// The AWS validation envelope (`N validation error(s) detected: `) over clauses already
    /// rendered by [`member_clause`]. `None` for no clause at all.
    ///
    /// A handler refusing an input member of its own states it through here, so an
    /// in-handler refusal reads exactly like one the centralized walk produced.
    pub fn validation_envelope(clauses: &[String]) -> Option<String> {

The crate's own invariant, declared over EVERY operation — tests/all/service_error_rules.rs:3-13:
    //! The three service-wide answer rules of PartnerCentral Benefits, over the wire.
    //! Each is a rule about EVERY operation, not about one of them ...
    //! 1. **A refused input** is answered 400 `ValidationException` whose message is the AWS
    //!    validation envelope (`N validation error(s) detected: `) over one clause per
    //!    violation, and each clause names the MEMBER PATH and the VIOLATED CONSTRAINT only
and its checker — tests/all/service_error_rules.rs:152-172:
    pub(crate) fn assert_validation_invariants(case: &str, message: &str) {
        assert!(
            message.starts_with("1 validation error detected: ") ...
            assert!(
                clause.starts_with("Value at '") || clause.starts_with("Value null at '"),

The member-path convention this crate states — src/ops/amend_benefit_application.rs:174-181:
    /// The path of one member of `Amendments[position]`, spelled the way this service's
    /// validation messages spell a member path: members in lowerCamel, a list element as
    /// its 1-BASED position followed by the collection step `member` (see
    /// `smithy_schema_server::constraints::path`, which spells the paths the centralized
    /// walk reports) — e.g. `amendments.1.member.fieldPath`.
    fn member_path(position: usize, member: &str) -> String {

The path the walk reports for THIS member, pinned by the crate — tests/all/validation_error_body.rs:295-302:
        (
            "a nextToken under the modelled 1-character minimum",
            "ListBenefits",
            json!({ "Catalog": CATALOG, "NextToken": "" }),
            one(&clause(
                "nextToken",
                "Member must have length greater than or equal to 1",
            )),
        ),

Sibling in-handler refusals that DO use both the envelope and the walk's spelling: src/ops/associate_benefit_application_resource.rs:148 (`const ASSOCIATED_RESOURCES_MEMBER: &str = "associatedResources";` rendered through `member_clause`/`validation_envelope` at :158-175), src/state.rs:2680-2698 (`reserved_tag_key_refusal`, called with `"tags"`/`"tagKeys"`), src/ops/amend_benefit_application.rs:249-262 (`refused`).

Test coverage: tests/all/pagination_cursor_issuance.rs:109-139 (`assert_refused`) only asserts the 400, the error code, `error_message(&out).contains("NextToken")` and `FieldList[0].Name == Some("NextToken")` — it never calls `assert_validation_invariants` and states no reason for the PascalCase spelling; tests/all/benefit_allocations.rs:783 asserts only `.contains("NextToken")`. So neither the message form nor the casing is pinned with a stated reason.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:90 does.
2. POST / with X-Amz-Target: PartnerCentralBenefitsService.ListBenefits and body {"Catalog": "AWS", "NextToken": ""} — a modelled @length violation. Observe Message = "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must have length greater than or equal to 1" (pinned in tests/all/validation_error_body.rs:295-302).
3. POST / with X-Amz-Target: PartnerCentralBenefitsService.ListBenefits and body {"Catalog": "AWS", "NextToken": "6161616161"} — a well-formed hex string this service never issued. Observe Message = "NextToken is not a pagination token issued by ListBenefits" with FieldList[0].Name = "NextToken": no `N validation error(s) detected: ` envelope, no `Value at '…' failed to satisfy constraint: …` clause, and a different spelling of the same member than step 2.
4. Run the crate's own invariant checker (tests/all/service_error_rules.rs:154 assert_validation_invariants) against that message and observe it fails the envelope assertion.
5. Repeat step 3 against ListBenefitApplications and ListBenefitAllocations for the same answer from src/ops/list_benefit_applications.rs:127-140 and src/ops/list_benefit_allocations.rs:128-141.
