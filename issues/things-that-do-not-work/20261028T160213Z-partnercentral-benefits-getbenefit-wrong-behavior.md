Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# Benefit and allocation ARN identifiers resolve regardless of partition, region and account, while the sibling application lookup requires an exact ARN match

_Filed: 2026-10-28T16:02:13Z_
_Operation: GetBenefit | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate resolves the "bare id or ARN" identifier members of the `^(arn:.+|<prefix>-[0-9a-z]{14})$` family TWICE, and the two implementations disagree about what an ARN has to match.

* `ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::resolve_application_id` (used by `GetBenefitApplication`, `Submit`, `Cancel`, `Recall`, `Update`, `Amend`, `Associate`, `Disassociate`) resolves an ARN through the `arns` index, i.e. by EXACT string match against the ARN the create minted, and its doc states the rule that follows: an ARN naming another partition, region or account resolves to nothing.
* `resolve_identifier` (used by `Catalog::resolve_benefit_id` and `Catalog::resolve_allocation_id`, and therefore by `GetBenefit`, `GetBenefitAllocation`, `CreateBenefitApplication`'s `BenefitIdentifier`, and the `BenefitIdentifiers` filters of `ListBenefits`/`ListBenefitApplications`/`ListBenefitAllocations`) parses the ARN with `parse_resource_arn`, which binds the partition, region and account fields to `_partition`, `_region`, `_account` and never looks at them. Only the literal `arn` prefix, the `partnercentral` service, the `catalog/<catalog>/<resource>/<id>` tail and the catalog name are checked.

So one and the same cross-account/cross-region spelling is answered two different ways by one service: `GetBenefit` with `arn:anything:partnercentral:eu-west-1:000000000000:catalog/AWS/benefit/<real id>` answers 200 with THIS emulator's benefit (and rebuilds `Arn` for the caller's own region, so the response silently re-labels the resource the caller asked for), while `GetBenefitApplication` with the analogous foreign ARN of a stored application answers 404 `ResourceNotFoundException`. The same lax path lets `CreateBenefitApplication` accept a `BenefitIdentifier` naming another partition/account and mint an application against this catalog's benefit.

The check is a check this crate already owns and documents; the benefit/allocation path is the one path that skips it.

## Evidence

The strict sibling, which states the rule — src/state.rs:1551-1565:
    /// The application `identifier` names inside `catalog`, accepting either form the
    /// modelled `BenefitApplicationIdentifier` allows (`^(arn:.+|benappl-[0-9a-z]{14})$`).
    ///
    /// An ARN must match a stored ARN exactly, so an ARN naming another partition,
    /// region or account resolves to nothing rather than to the application that happens
    /// to share its id segment — and an ARN whose catalog is not the one the request
    /// named resolves to nothing either.
    pub fn resolve_application_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        if identifier.starts_with("arn:") {
            return self
                .arns
                .get(identifier)
                .filter(|(arn_catalog, _)| arn_catalog == catalog)
                .map(|(_, id)| id.clone());
        }

The lax path, whose own doc enumerates only catalog/service/resource-type — src/state.rs:1412-1426:
    /// One construction for every identifier member whose modelled pattern admits both forms
    /// (`^(arn:.+|<prefix>-[0-9a-z]{14})$`), so an ARN naming another catalog, another service
    /// or another resource type resolves to nothing rather than to the record that happens to
    /// share its id segment. Whether a record of that id EXISTS is the caller's own check.
    fn resolve_identifier(catalog: &str, identifier: &str, resource: &str) -> Option<String> {
        if !identifier.starts_with("arn:") {
            return Some(identifier.to_string());
        }
        let (arn_catalog, arn_id) = parse_resource_arn(identifier, resource)?;
        (arn_catalog == catalog).then(|| arn_id.to_string())
    }

and the parser it relies on, which discards exactly the three fields the sibling rejects on — src/state.rs:2574-2586:
    pub fn parse_resource_arn<'a>(arn: &'a str, resource: &str) -> Option<(&'a str, &'a str)> {
        let mut fields = arn.splitn(6, ':');
        let (prefix, _partition, service, _region, _account, tail) = (
            fields.next()?,
            ...
        );
        if prefix != "arn" || service != ARN_SERVICE {
            return None;
        }

The two callers of the lax path — src/state.rs:1396-1398 and :1406-1408:
    pub fn resolve_benefit_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        let id = resolve_identifier(catalog, identifier, BENEFIT_RESOURCE)?;
...
    pub fn resolve_allocation_id(&self, catalog: &str, identifier: &str) -> Option<String> {
        let id = resolve_identifier(catalog, identifier, ALLOCATION_RESOURCE)?;

and the handlers, each asserting the narrower rule in a comment — src/ops/get_benefit.rs:192-194:
        // Either form the modelled `Identifier` admits names the same record; an ARN of
        // another catalog, service or resource type names none.
        let Some(benefit) = store.benefit(&input.catalog, &input.identifier) else {
src/ops/get_benefit_allocation.rs:276-278:
        // Either form the modelled `BenefitAllocationIdentifier` admits names the same
        // record; an ARN of another catalog, service or resource type names none.
        let Some(allocation) = store.allocation(&input.catalog, &input.identifier) else {
versus src/ops/get_benefit_application.rs:214:
        let Some(application) = store.application(&input.catalog, &input.identifier) else {

The create reaches the lax path for a caller-supplied benefit ARN — src/ops/create_benefit_application.rs:383-385:
            let Some(benefit) = catalog
                .resolve_benefit_id(&input.catalog, &input.benefit_identifier)
                .and_then(|id| catalog.benefits.get(&id))

Test coverage: no test pins the lax answer with a stated reason. tests/all/benefit_allocations.rs:616-621 is the only row that feeds a benefit ARN with a foreign region AND a foreign account (`"arn:aws:partnercentral:us-west-2:123456789012:catalog/Sandbox/benefit/ben-00000000000000"`), and its case name says what it is pinning — `"a benefit ARN of another catalog"` — so it asserts the CATALOG mismatch, not that the region and account are ignored. tests/all/service_error_rules.rs:984-987 and tests/all/unknown_catalog_lookup.rs:195 build their ARNs from the harness's own `REGION`/`ACCOUNT`, so they never exercise a foreign one; tests/all/resource_tagging.rs:492-509 varies the account only on the tagging path, which uses the exact `arns` index.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_application_lifecycle.rs:86-91 does.
2. Call ListBenefits with {"Catalog": "AWS"} and keep one summary's Id (ben-…) and Arn (the ARN carries the caller's own region and engine::EMULATOR_ACCOUNT_ID).
3. Call GetBenefit with {"Catalog": "AWS", "Identifier": "arn:aws:partnercentral:eu-west-1:000000000000:catalog/AWS/benefit/<that ben- id>"} — another region AND another account. Observe 200 with this emulator's benefit, and an Arn rebuilt for the caller's region, where src/state.rs:1554-1556 states that an ARN naming another region or account resolves to nothing.
4. Repeat with the partition changed too ("arn:not-aws:partnercentral:eu-west-1:000000000000:catalog/AWS/benefit/<id>") — still 200, since src/state.rs:2584 checks only that the first field is "arn".
5. Repeat against GetBenefitAllocation with a seeded allocation's benalloc- id spelled as "arn:aws:partnercentral:eu-west-1:000000000000:catalog/AWS/benefit-allocation/<id>" — 200.
6. Now CreateBenefitApplication in catalog AWS, keep the returned Id, and call GetBenefitApplication with {"Catalog": "AWS", "Identifier": "arn:aws:partnercentral:eu-west-1:000000000000:catalog/AWS/benefit-application/<that benappl- id>"} — 404 ResourceNotFoundException. One service, two answers for the same cross-account spelling of the same identifier family.
7. Also call CreateBenefitApplication with BenefitIdentifier set to the foreign-account benefit ARN from step 3 and observe the application is created against this catalog's benefit.
