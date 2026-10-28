Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# An AssociatedResources filter whose entries carry no ResourceArn selects every application instead of filtering

_Filed: 2026-10-28T02:39:56Z_
_Operation: ListBenefitApplications | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBenefitApplications` honours the deprecated `AssociatedResources` filter by extracting each element's optional `ResourceArn` with `filter_map`, so an element that carries no `ResourceArn` (only `ResourceType` and/or the deprecated `ResourceIdentifier` — both optional members of the modelled `AssociatedResource`) is silently dropped. When every supplied element is dropped, `resource_arns` ends up EMPTY, and `BenefitApplication::matches_resource_arns` treats an empty filter as matching everything — so a request that asked for the applications linked to one resource is answered with EVERY application of the catalog.

That contradicts the filter rule this same handler states fifteen lines earlier for `BenefitIdentifiers`: a filter the caller DID supply whose entries resolve to nothing must select NOTHING, and only an absent or empty filter selects everything. The handler applies that rule to `BenefitIdentifiers` (`Option<Vec<_>>`, `is_none_or`) and breaks it for `AssociatedResources` (a flat `Vec<String>` whose emptiness no longer records whether a filter was supplied).

## Evidence

The rule, in this handler — src/ops/list_benefit_applications.rs:236-249:
        // A filter the caller DID supply whose entries name no benefit of this catalog
        // therefore selects nothing — unlike an absent or empty filter, which selects
        // everything.
        let benefit_filter: Option<Vec<String>> =
            input.benefit_identifiers.as_ref().and_then(|wanted| {
                (!wanted.is_empty()).then(|| { ... .filter_map(...).collect() })
            });

The code that breaks it — src/ops/list_benefit_applications.rs:250-260:
        // `AssociatedResources` is `@deprecated` ("This field is no longer used") in
        // favour of `AssociatedResourceArns`, but a client may still send it, so the ARNs
        // it carries filter alongside the newer member's.
        let mut resource_arns: Vec<String> = input.associated_resource_arns.unwrap_or_default();
        resource_arns.extend(
            input
                .associated_resources
                .unwrap_or_default()
                .into_iter()
                .filter_map(|resource| resource.resource_arn),
        );
...applied at src/ops/list_benefit_applications.rs:287:
                    && application.matches_resource_arns(&resource_arns)

What an empty list means to that predicate — src/state.rs:1232-1239:
    /// Whether this application carries at least one of `arns` in its associated
    /// resources. An empty filter matches everything.
    pub fn matches_resource_arns(&self, arns: &[String]) -> bool {
        arns.is_empty()
            || arns.iter().any(|wanted| self.associated_resources.contains(wanted))
    }

That such an element is valid input — smithy_query(query="shape", shape="AssociatedResource"): members `ResourceType`, `ResourceIdentifier` (deprecated) and `ResourceArn` — none of the three carries `"required": true`, so `{"ResourceType": "..."}` passes the centralized constraint walk.

Test coverage: no test in the crate sends the `AssociatedResources` filter to ListBenefitApplications — every resource-filter row uses `AssociatedResourceArns` (tests/all/associate_benefit_application_resource.rs:677,698; tests/all/disassociate_benefit_application_resource.rs:512,533,544; tests/all/benefit_application_lifecycle.rs:1549), so the dropped-element path is unpinned.

## Steps to Reproduce

1. Create two applications in catalog `AWS` and associate one of them with an ARN via AssociateBenefitApplicationResource.
2. POST / with X-Amz-Target: PartnerCentralBenefitsService.ListBenefitApplications and body {"Catalog": "AWS", "AssociatedResources": [{"ResourceType": "EC2_INSTANCE", "ResourceIdentifier": "i-0123456789abcdef0"}]} — a supplied resource filter naming no ARN.
3. Observe that BenefitApplicationSummaries carries BOTH applications (the filter selected everything) rather than none.
4. Compare with {"Catalog": "AWS", "BenefitIdentifiers": ["ben-00000000000000"]}, a supplied filter that resolves to nothing, which correctly reports no summaries.
