Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8ac986246872ffdcf8619506b271763909cbb21e

# UpdateDeployment dry run answers 200 instead of the documented DryRunOperation error response

_Filed: 2026-10-24T22:14:55Z_
_Operation: UpdateDeployment | Class: error-response_

> _Note: this issue was identified against the code base at commit `8ac986246872ffdcf8619506b271763909cbb21e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateDeploymentInput$dryRun` carries the same documented contract as the create path: the pre-flight "provides an error response", `DryRunOperation` when the caller is permitted. The handler instead returns HTTP 200 with an empty `UpdateDeploymentOutput` (`deployment: None`), explicitly copying the create path's rationale. A client therefore gets a success envelope with no deployment in it, which is indistinguishable from a real update whose output was dropped.

As with CreateDeployment, this crate already owns the means to answer a code its model does not declare (`Scaffolded::unmodeled`, used for the tagging layer's `NotFoundException`/`BadRequestException`), so "the model declares no such error" is not a reason this crate accepts anywhere else. The "persists nothing" behaviour is correct and must be preserved.

## Evidence

Smithy model — `smithy_query(query="doc", shape="UpdateDeploymentInput", member="dryRun")`:
  "Checks whether you have the required permissions for the action, without actually making the request, and provides an error response. If you have the required permissions, the error response is `DryRunOperation`. Otherwise, it is `UnauthorizedOperation`."

Handler — services/service-com-amazonaws-launchwizard-launchwizard/src/ops/update_deployment.rs:300-305:
```
        // `dryRun` checks the request without making it: every check above has run and,
        // by definition, NOTHING is persisted — so there is no updated deployment to
        // report either, which is the answer `CreateDeployment` gives a dry run too.
        if input.dry_run {
            return Ok(UpdateDeploymentOutput { deployment: None });
        }
```

Same-crate mechanism it declines to use: src/scaffolded.rs:298-311 (`pub fn unmodeled(code, status, message)` — "Answer an error code this service's model does not declare, with the HTTP status the real service answers it with"), already used at src/state.rs:1775-1788 for this service's undeclared tagging codes.

Test coverage: tests/all/deployment_update.rs:551 `a_dry_run_update_checks_the_request_and_persists_nothing` pins the current 200/empty answer (":571 Every row is a dry run that passes its checks: it answers success, reports no ..."), and tests/all/deployment_update.rs:15 states the intent as "`dryRun` checks the request and persists nothing" — the persistence half, not the response form the model documents.

## Steps to Reproduce

1. Create a deployment and let it settle to COMPLETED.
2. POST /updateDeployment {"deploymentId": "<id>", "specifications": {<valid map>}, "dryRun": true}.
3. Observe HTTP 200 with body `{}` (no `deployment` member) rather than the `DryRunOperation` error response the model documents for `dryRun`.
4. POST /getDeployment and confirm the record is untouched — only the response form contradicts the model.
