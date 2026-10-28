Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.import-documentation-parts-bulk-atomic apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.import-documentation-parts-bulk-atomic|_error#19|AssertionError|raise AssertionError("expected a ClientError, but the call SUCCEEDED")
Issue-Key: apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare|_poll#15|Timeout|time.sleep(interval_s)
Issue-Commit: 3bfaf82953d93db9f40ee26ed0fb0b9c83dbff8e

# API Gateway: 2 of the seven causes of the request-validator issue are still open — ImportDocumentationParts' failOnWarnings refusal cannot be worded from any source available in this repo, and the four-mode validator check still times out with no response captured

> _Split out of `issues/things-that-now-work/20261027T101902Z-apigateway-api-gateway-7-net-new-root-causes-request.md`, whose other five causes are fixed and whose units pass. This report carries the two replay keys that do NOT pass, so neither cause is claimed closed. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._

## Operation
ImportDocumentationParts

## Description

Cause 1 of 2: `failOnWarnings` is still a no-op on `ImportDocumentationParts`, because the WARNING SENTENCE the refusal must state cannot be sourced.
AWS: with `mode=merge` and `failOnWarnings=true`, an import whose body earns a warning is refused with HTTP 400 `BadRequestException` and nothing is created; with `failOnWarnings=false` the same body commits and the warnings are reported in `DocumentationPartIds$warnings`.
Emulator: the call still SUCCEEDS (`warnings` absent), exactly as the parent report recorded.
What the parent fix ESTABLISHED, which is the useful part of this report:
* The warning class is an entry whose location names a RECORD of the API by name that the API does not hold. Implementing exactly that rule — a `MODEL` or `AUTHORIZER` location whose `name` resolves to no record — moved the recorded check PAST its own key (`_error#19`, "expected a ClientError, but the call SUCCEEDED") and onto its next assertion, `run#21 assert blocked_message == (…)`. So the condition is right: the body the check imports documents the model `Empty`, which a `CreateRestApi`-created API does not hold.
* What blocks it is the SENTENCE. The check compares the refusal's whole message verbatim, and the replay reports only what the emulator produced (`expected` is empty for this assertion and its custom message quotes the actual value alone), so AWS's wording is not readable from the replay. It is in neither the Smithy model (`ImportDocumentationParts`'s `warnings` member documents only "a list of warning messages reported during import of documentation parts") nor the parent report, which records only the status, the code and the atomicity.
* Five candidate wordings were tried against the recorded unit, each inside the recorded lead `"Warnings found during import:\n\t…"` this service's other two importers answer with, and each rejected:
  1. `Invalid model name specified: Empty` (this service's own recorded sentence for a model name that resolves to nothing)
  2. `Unable to resolve reference 'Empty'. Ignoring.` (the definition importer's recorded unresolved-reference warning)
  3. `Unable to create documentation part for the specified location: type 'MODEL', name 'Empty'.`
  4. `Unable to create documentation part for MODEL 'Empty'. Ignoring.`
  5. `Unable to resolve model reference 'Empty'. Ignoring.`
Nothing of that rule was landed, so no invented sentence is in the tree: `src/ops/import_documentation_parts.rs` is as it was, and `tests/all/documentation_part_import_warnings.rs` pins only the half of the flag that IS sourced (a body that warns about nothing is imported identically whichever way the flag and the mode are set, and an entry the import cannot STORE refuses the whole body either way).
Fix: the warning collection and the `failOnWarnings` gate in `src/ops/import_documentation_parts.rs`, with the warning sentence and the refusal taken from AWS's own wording for this import. A fixer who can read the recorded response for `apigateway.import-documentation-parts-bulk-atomic` (the `run#21` expectation) can close this in one edit; guessing it is not a fix.
Traces: apigateway.import-documentation-parts-bulk-atomic

Cause 2 of 2: the four-mode request-validator check still times out, with no response and no assertion captured, and the mechanism the parent report hypothesised is refuted.
AWS: with a validator in any of the four modes (body-only, params-only, both, none), a request that satisfies everything the method declares is served by the integration with HTTP 200 and the mapped integration response.
Emulator: "Check timed out after 60.0s" — the check polls `POST <invoke-url>/<route>` with a valid body and the required `tenant` query parameter for each of `/body`, `/params`, `/both`, `/none` until one answers 200, and no response was captured at all.
What the parent fix ESTABLISHED:
* The request validator IS now applied at a deployed stage and the four modes are each enforced only as far as they declare (its sibling check, `apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses`, passes, including the customized `BAD_REQUEST_BODY`/`BAD_REQUEST_PARAMETERS` gateway responses and the validation detail).
* The parent's hypothesis — "a method carrying both `requestModels` and `requestParameters` is not servable at the stage" — is REFUTED by `tests/all/stage_request_validation.rs::each_validator_mode_refuses_only_what_it_declares`: four routes on one api, each method declaring `requestModels` AND a required `requestParameters` with a validator in one of the four modes, answer the fully valid POST 200 with the integration's own body; `a_body_validator_refuses_every_payload_the_model_does_not_admit` drives the same positive request through all three addressings a client can reach a stage by (the api's own invoke host, `/_aws/execute-api/{api-id}/…`, and the api id as the first path segment) with the same answer.
* So the timeout has a mechanism this replay does not expose. The one MOCK configuration a deployed stage still refuses is an integration whose mapping template has to be EXECUTED — a Velocity directive, which this emulator does not evaluate and answers `API_CONFIGURATION_ERROR` (500) for; if the check's integration or integration-response template carries one, that is the mechanism, and it is the absence the open report `20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md` carries for the `deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response` family.
Fix: establish what the check's first data-plane request is answered with (a captured status would settle it), then either the mapping-template evaluation that family needs or whatever the captured answer names.
Traces: apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare

## Evidence
Replay of the parent issue's seven units after its fix: 5 passed (`apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses`, `e2e-usage-plan-key-throttle-consequence`, `authorizer-deletion-blocked-while-referenced-rejected`, `model-invalid-schema-json-rejected`, `op-contract-delete-model`), 2 failed:
* `apigateway.import-documentation-parts-bulk-atomic` — with the warning rule in place: `AssertionError: expected the unresolvable model reference to be named in the rejection, got "Warnings found during import:\n\tUnable to resolve model reference 'Empty'. Ignoring."` (and the same assertion for each of the other four candidate wordings). Without it, where the tree now stands: "expected a ClientError, but the call SUCCEEDED".
* `apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare` — "Check timed out after 60.0s", no response, no assertion message.

## Steps to Reproduce
1 (Cause 1). `CreateRestApi`; `ImportDocumentationParts(restApiId, mode="merge", failOnWarnings=true, body=<a documentation set whose entries include {"location": {"type": "MODEL", "name": "Empty"}}>)`: expect HTTP 400 `BadRequestException` whose message states the warning as AWS words it, and `GetDocumentationParts` unchanged; with `failOnWarnings=false` the same body must commit and report that warning. Observe HTTP 200 with no `warnings`.
2 (Cause 2). Follow the parent report's step 1 (a model with a draft-04 schema, a validator in each of the four modes, `PutMethod` POST with `requestModels` + `requestParameters` + `requestValidatorId`, a `MOCK` integration, `CreateDeployment` + `CreateStage`), then `POST <invoke-url>/<route>` with a valid body and the required query parameter and capture the STATUS and BODY of the first answer — that is the datum this cause lacks.
Then replay the two check ids in trace_ids.
