Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# A cost estimate is refused with 404 unless the named stacks/tags are already in the analyzed resource collection

_Filed: 2026-10-24T00:38:29Z_
_Operation: StartCostEstimation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartCostEstimation resolves the submitted `ResourceCollection` against the account's DevOps Guru analysis coverage (the set `UpdateResourceCollection` maintains) and answers `ResourceNotFoundException` (404) for any CloudFormation stack name or (AppBoundaryKey, TagValue) pair that is not already covered. That inverts the purpose of cost estimation: DevOps Guru's cost estimate exists so a customer can price analysis of resources BEFORE enabling analysis for them, which is why the operation takes its own `CostEstimationResourceCollectionFilter` instead of reusing the account's coverage. Expected: a start that names a stack or tag the account has not onboarded is accepted (HTTP 200) and a subsequent GetCostEstimation reports an estimate for the resources that collection selects; a 404 should be reserved for a collection whose named resources do not exist at all, not for one outside the current analysis coverage. Actual: the common pre-onboarding workflow (StartCostEstimation → GetCostEstimation → then UpdateResourceCollection) fails at the first call with HTTP 404 ResourceNotFoundException, and an estimate is obtainable only for resources already being analyzed (and therefore already being billed).

## Evidence

Spec: smithy_query(operation="StartCostEstimation") — input `ResourceCollection` (CostEstimationResourceCollectionFilter, required) + `ClientToken`; the operation documentation is "Starts the creation of an estimate of the monthly cost to analyze your Amazon Web Services resources", and `GetCostEstimation` returns the estimate for THAT collection — neither operation references the analyzed resource collection, which is modeled by a separate shape (ResourceCollectionFilter / UpdateResourceCollectionFilter) and served by Get/UpdateResourceCollection. Handler: src/ops/start_cost_estimation.rs lines ~247-300 — `let coverage = &guard.resource_collection;` then `if let Some(missing) = stack_names.iter().find(|name| !coverage.cloudformation_stack_names.contains(*name)) { return Err(uncovered_resource(...)) }` and the equivalent tag check, both returning `ResourceNotFoundException`. Tests: tests/all/cost_estimation.rs `a_start_resolves_its_collection_against_the_analysis_coverage` pins the 404-for-uncovered-resource behavior, so the divergent rule is asserted rather than the pre-onboarding estimate.

## Steps to Reproduce

1. Start a fresh emulator (no UpdateResourceCollection calls, i.e. nothing onboarded).
2. PUT /cost-estimation with {"ResourceCollection":{"CloudFormation":{"StackNames":["my-stack"]}},"ClientToken":"tok1"}.
3. Observe HTTP 404 ResourceNotFoundException naming 'my-stack'; expected HTTP 200 with the estimation started.
4. GET /cost-estimation and observe no estimation at all (TotalCost 0, no Status), where an ONGOING/COMPLETED estimate for the submitted collection is expected.
5. Repeat after UpdateResourceCollection ADD of 'my-stack' and observe the start now succeeds — showing the refusal is driven by analysis coverage rather than by the named resource existing.
