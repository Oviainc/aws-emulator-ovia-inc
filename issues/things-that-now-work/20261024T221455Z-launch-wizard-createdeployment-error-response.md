Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8ac986246872ffdcf8619506b271763909cbb21e

# CreateDeployment dry run answers 200 instead of the documented DryRunOperation error response

_Filed: 2026-10-24T22:14:55Z_
_Operation: CreateDeployment | Class: error-response_

> _Note: this issue was identified against the code base at commit `8ac986246872ffdcf8619506b271763909cbb21e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `dryRun` as a pre-flight that ALWAYS answers with an error response: `DryRunOperation` when the caller is permitted, `UnauthorizedOperation` when it is not. The handler does the opposite: a dry run that passes every check returns HTTP 200 with an empty `CreateDeploymentOutput` (no `deploymentId`), so a client that sets `dryRun` can never distinguish "pre-flight succeeded" from "a real create that forgot to report its id" — the exact ambiguity the documented error code exists to remove.

The handler's stated reason for the success answer ("The model declares no `DryRunOperation` error for this operation") is contradicted by this same crate: `Scaffolded::unmodeled` exists precisely to answer "an error code this service's model does not declare", is documented for codes known "from a recording or from AWS's documentation", and is already used by this crate's tagging paths (`NotFoundException`, `BadRequestException`). So the crate has the mechanism and uses it elsewhere; only the dry-run branch declines to.

The persist-nothing half of the behaviour is correct and must stay: the defect is the response, not the storage.

## Evidence

Smithy model — `smithy_query(query="doc", shape="CreateDeploymentInput", member="dryRun")`:
  "Checks whether you have the required permissions for the action, without actually making the request, and provides an error response. If you have the required permissions, the error response is `DryRunOperation`. Otherwise, it is `UnauthorizedOperation`."
(the same sentence is reproduced in the handler's own modelled-contract header, services/service-com-amazonaws-launchwizard-launchwizard/src/ops/create_deployment.rs:55-58)

Handler — services/service-com-amazonaws-launchwizard-launchwizard/src/ops/create_deployment.rs:290-298:
```
        // `dryRun` checks the request without making it. The model declares no
        // `DryRunOperation` error for this operation, so a dry run that passes every
        // check above answers success — and, by definition, persists NOTHING, so it
        // mints no id to report.
        if input.dry_run {
            return Ok(CreateDeploymentOutput {
                deployment_id: None,
            });
        }
```

Same crate, the mechanism the comment says it lacks — services/service-com-amazonaws-launchwizard-launchwizard/src/scaffolded.rs:298-311:
```
    /// Answer an error code this service's model does not declare, with the HTTP
    /// status the real service answers it with.
    ...
    pub fn unmodeled(code: &'static str, status: u16, message: impl Into<String>) -> Self {
```
and its use for this service's undeclared tagging codes — src/state.rs:1757-1763 ("This is the shared tagging layer's own not-found code, which this service's model does not declare ... so it is answered through [`crate::scaffolded::Scaffolded::unmodeled`]") and src/state.rs:1775-1777.

Test coverage: tests/all/deployment_lifecycle.rs:449 `a_dry_run_validates_and_stores_nothing` pins the 200/no-id answer (":459 A dry run that passes every check succeeds and mints no id"), and tests/all/create_deployment_validation.rs:191-207 re-runs each validation row as a dry run. Those tests assert the emulator's current choice, not the modelled sentence above.

## Steps to Reproduce

1. Create a deployment request for a seeded workload/pattern with valid specifications and `"dryRun": true`: POST /createDeployment {"workloadName": "<seeded>", "deploymentPatternName": "<seeded>", "name": "dry-run-probe", "specifications": {<valid map>}, "dryRun": true}.
2. Observe HTTP 200 with body `{}` (no `deploymentId`), i.e. no error response at all.
3. Compare with the modelled documentation for `CreateDeploymentInput$dryRun`, which states a permitted dry run is answered with the `DryRunOperation` error response.
4. POST /listDeployments and confirm nothing was stored — the persist-nothing half is already correct; only the response contradicts the model.
