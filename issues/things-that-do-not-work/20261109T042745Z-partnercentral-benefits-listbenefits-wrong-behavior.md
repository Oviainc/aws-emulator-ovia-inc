Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cef70499ebb4de11bd935d0814e43f93e778fd02

# Benefit ARNs are minted with an account id although the model declares the Benefit resource @arn noAccount

_Filed: 2026-11-09T04:27:45Z_
_Operation: ListBenefits | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cef70499ebb4de11bd935d0814e43f93e778fd02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy model gives the `Benefit` resource an `@arn` trait carrying `noAccount: true`, i.e. a benefit ARN has an EMPTY account-id field — and it is the ONLY one of this service's three resources that declares it (`BenefitApplication` and `BenefitAllocation` do not). The crate ignores the distinction: `benefit_arn` goes through the one `resource_arn` construction that always fills the account field, so every benefit ARN this service issues carries the emulator account id.

Two consequences:

1. `ListBenefits` reports `BenefitSummary$Arn` — and `GetBenefit` reports `GetBenefitOutput$Arn` — in a form the model says a benefit ARN does not have (`arn:aws:partnercentral:us-west-2:<account>:catalog/AWS/benefit/ben-...` instead of `arn:aws:partnercentral:us-west-2::catalog/AWS/benefit/ben-...`).

2. Because the identifier resolution compares the WHOLE ARN against the same `resource_arn` construction, a client that builds the benefit ARN the model declares (empty account field) names no record: `GetBenefit`, `CreateBenefitApplication$BenefitIdentifier`, and the `BenefitIdentifiers` filters of `ListBenefitApplications`/`ListBenefitAllocations` all answer `ResourceNotFoundException` / select nothing for it.

The crate's own stated justification for using ONE ARN form for all three resource types does not cover a benefit: it is grounded in the `ListTagsForResource$resourceArn` pattern (which requires `[0-9]{12}` in the account field), yet the same module states that the benefit application is the only taggable resource of this service, so a benefit ARN never has to satisfy that tagging pattern.

This is distinct from the already-fixed "Benefit and allocation ARN identifiers resolve regardless of partition, region and account" gap: that one established that every ARN field must MATCH exactly, which stays. What is wrong here is the canonical form the service MINTS for a benefit.

## Evidence

Model — `smithy_query(query="shape",shape="Benefit,BenefitApplication,BenefitAllocation")`, the benefit alone declares `noAccount`:

  "Benefit": {"shape_id": "com.amazonaws.partnercentralbenefits#Benefit", "type": "resource", "arn": {"template": "catalog/{CatalogName}/benefit/ben-{BenefitId}", "noAccount": true}, ...}
  "BenefitApplication": {... "arn": {"template": "catalog/{CatalogName}/benefit-application/benappl-{BenefitApplicationId}"}, ...}
  "BenefitAllocation": {... "arn": {"template": "catalog/{CatalogName}/benefit-allocation/benalloc-{BenefitAllocationId}"}, ...}

Code — one account-bearing form for all three, `services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:3026-3048`:

  /// The ARN of the benefit `id` of `catalog`, in `region`.
  pub fn benefit_arn(region: &str, catalog: &str, id: &str) -> String {
      resource_arn(region, catalog, BENEFIT_RESOURCE, id)
  }
  ...
  /// `arn:aws:partnercentral:{region}:{account}:catalog/{catalog}/{resource}/{id}`.
  ///
  /// The `catalog/...` resource shape is the one the modelled
  /// `ListTagsForResource$resourceArn` pattern declares for this service
  /// (`^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/[\w+=,.@-]+(/[\w+=,.@-]+)*$`),
  /// so one ARN form serves both the resource members and the tagging operations.
  fn resource_arn(region: &str, catalog: &str, resource: &str, id: &str) -> String {
      ::engine::emulator_arn(ARN_SERVICE, region, &format!("catalog/{catalog}/{resource}/{id}"))
  }

That justification does not reach a benefit — same file, `src/state.rs:2057-2061`:

  /// The benefit application is the one resource of this service that holds a tag set:
  /// it is the only shape with a `Tags` member (`CreateBenefitApplicationInput$Tags`),
  /// while a benefit and a benefit allocation carry none on any input or output. An
  /// ARN of any other resource type therefore resolves to no taggable resource.

Where the wrong form is reported — `src/ops/list_benefits.rs:248-252`:

      arn: Some(crate::state::benefit_arn(
          &region,
          &input.catalog,
          &benefit.id,
      )),

and `src/ops/get_benefit.rs:215-219`:

      arn: Some(crate::state::benefit_arn(
          &region,
          &input.catalog,
          &benefit.id,
      )),

Where the model-declared form is then refused — `src/state.rs:3075` (reached for a benefit from `src/state.rs:1502`):

      (arn == resource_arn(region, catalog, resource, id)).then_some((catalog, id))

      let (arn_catalog, arn_id) = parse_resource_arn(identifier, region, resource)?;

Test coverage: the account-bearing benefit ARN is pinned by `tests/all/arn_identifier_fields.rs:160-165`, which asserts the issued benefit ARN equals `expected_arn(catalog, BENEFIT_RESOURCE, &id)`, where `tests/all/resource_tagging.rs:156-158` is

  pub(crate) fn expected_arn(catalog: &str, resource: &str, id: &str) -> String {
      format!("arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{catalog}/{resource}/{id}")
  }

The reason that test states is about field exactness ("the service must issue the benefit ARN of the region the request addressed"), not about the presence of an account field on a benefit ARN, so the `@arn(noAccount: true)` trait contradicts it on that point. `tests/all/benefit_catalog_record.rs` and `tests/all/get_benefit.rs` cover the benefit report itself.

## Steps to Reproduce

1. Start the emulator and sign a request for region us-west-2 from an account the partner-entitlement gate admits.
2. Call `ListBenefits` with `{"Catalog": "AWS"}` and read `BenefitSummaries[0].Arn` — it comes back as `arn:aws:partnercentral:us-west-2:<emulator-account-id>:catalog/AWS/benefit/ben-...`, with the account field populated, although the model's `Benefit` resource declares `@arn(... noAccount: true)`.
3. Call `GetBenefit` with that benefit's `Id` and read `Arn` — same account-bearing form.
4. Build the benefit ARN the model declares (empty account field): `arn:aws:partnercentral:us-west-2::catalog/AWS/benefit/ben-<id>`.
5. Call `GetBenefit` with `{"Catalog": "AWS", "Identifier": "<that ARN>"}` — it answers 404 `ResourceNotFoundException` ``Benefit `...` not found`` instead of the benefit.
6. Call `CreateBenefitApplication` with that ARN as `BenefitIdentifier`, and `ListBenefitApplications` / `ListBenefitAllocations` with it in `BenefitIdentifiers` — the create is refused 404 and both filters select nothing.
