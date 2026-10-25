Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.documentation-version-snapshot-pinned
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|GetExport|BadRequestException
Issue-Commit: bf97f8eaa9587c57bb5d5ce296c420281b7febcd

# GetExport refuses extensions=documentation with BadRequestException, so a stage's pinned documentation snapshot can never be exported

> _Note: this issue was identified against the code base at commit `bf97f8eaa9587c57bb5d5ce296c420281b7febcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
One net-new root cause in apigateway (every other failing check in this suite run is already filed as an unimplemented-stub issue or OWNED by an open issue).

Cause 1 of 1: GetExport's `parameters["extensions"]` whitelist rejects the `documentation` value, so the documentation-export flow is unreachable and the DocumentationVersion snapshot a stage pins is never rendered. Failing check: apigateway.documentation-version-snapshot-pinned.

AWS: GetExport (GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}) takes an `extensions` query parameter for exportType `swagger`/`oas30`. The Smithy model's prose for GetExportRequest.parameters names `integrations`, `apigateway`, `authorizers` and `postman` (Smithy: GetExportRequest.parameters, MapOfStringToString, @httpQueryParams; modeled errors BadRequestException/400, NotFoundException/404, UnauthorizedException/401, ConflictException/409, LimitExceededException/429, TooManyRequestsException/429) — it does not enumerate `documentation`, so for that value this is recorded behavior only: the recording shows the real service ACCEPTING `parameters={"extensions": "documentation"}` and answering 200 with a Swagger body carrying an `x-amazon-apigateway-documentation` object whose `documentationParts` list the parts frozen by the DocumentationVersion the stage pins (`CreateStage`/`UpdateStage` `documentationVersion`), NOT the live, later-edited DocumentationPart. In the recording the API-level part exported the value it held when CreateDocumentationVersion was called, while GetDocumentationPart on the same part returned the post-snapshot edited value.

Emulator: the same call is refused with HTTP 400 `BadRequestException`, message `Invalid extensions specified: documentation; extensions must be one of integrations, apigateway, authorizers`. In that message the fixed text is `Invalid extensions specified: ` / `; extensions must be one of ` and the trailing accepted-value list; the variable part is the offending extension name taken from the request (`documentation`). The whole message is a defect here, because the request is legal and must not be refused at all: the emulator's accepted set is only {integrations, apigateway, authorizers} (EXTENSIONS_INTEGRATIONS / EXTENSIONS_AUTHORIZERS), with no branch for `documentation` (nor for `postman`). The module header states outright that the content of the documentation parts a stage's documentationVersion froze is deliberately not merged into the exported definition, so even if the value were accepted there is no `x-amazon-apigateway-documentation` renderer and no snapshot read.

Why: a client that exports API documentation — the documented round-trip that pairs with ImportDocumentationParts, used by SDK/doc pipelines and by the console's "Export documentation" path — gets a hard 400 on a valid request instead of a document. Because the refusal arrives before any rendering, the DocumentationVersion contract itself is unobservable through the emulator: a caller cannot tell whether a stage serves the frozen snapshot or the live parts, and code that diffs an exported snapshot against live parts (release gating, doc publishing) cannot run at all. A 400 on a legal call is also the wrong failure class for a retrying client: it is terminal, not transient.

Fix: GetExport's `parameters` validation must accept the `documentation` extension value (and `postman`, which the model documents and which is likewise rejected today) rather than falling through to the BadRequestException arm of the extensions parser; and GetExport's document rendering must gain an `x-amazon-apigateway-documentation` section sourced from the DocumentationVersion snapshot pinned by the stage being exported — i.e. the stage's `documentationVersion`, resolved against the stored documentation-version snapshot rather than the live DocumentationPart records. That means the operation's validation, its serializer (the document builder for both the swagger and oas30 dialects) and the documentation-version state it reads. File and line citations are under Traces.

This is not covered by any open issue: the open apigateway issues named in this run's report cover PutRestApi/import overwrite, control-plane refusal classification, invented fields, UpdateAccount convergence, stage serving and the CFN-provisioning clusters; none of them names GetExport or the documentation extension.

Traces: apigateway.documentation-version-snapshot-pinned. Emulator code: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_export.rs lines 510-533 (the extensions parser and its BadRequestException arm) and the module header at lines 32-37 (states the frozen documentation parts are intentionally not rendered).

## Evidence
Check: apigateway.documentation-version-snapshot-pinned — verdict [error].

Expected (real AWS, from the recording): GetExport(restApiId=<id>, stageName=<stage>, exportType="swagger", parameters={"extensions": "documentation"}, accepts="application/json") returns HTTP 200 with a JSON body containing x-amazon-apigateway-documentation.documentationParts; exactly one part has location.type == "API" and its properties.description is the value held at CreateDocumentationVersion time (the pre-edit snapshot value), while GetDocumentationPart on the same part returns the post-edit value.

Actual (emulator):
  botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the GetExport operation: Invalid extensions specified: documentation; extensions must be one of integrations, apigateway, authorizers

Status 400 / code BadRequestException where the recording has 200 and a document. No export body is produced, so the snapshot-vs-live assertion is never reached.

Source: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_export.rs:514-530 — the extensions loop accepts only EXTENSIONS_INTEGRATIONS (integrations/apigateway) and EXTENSIONS_AUTHORIZERS, and returns BadRequestException for anything else; get_export.rs:32-37 — "What the document does NOT carry ... the content of the documentation parts a stage's documentationVersion froze".

## Steps to Reproduce
1. apigateway.CreateRestApi(name=<unique>) -> restApiId; add a deployable GET / MOCK method (PutMethod authorizationType=NONE, PutIntegration type=MOCK, PutMethodResponse 200, PutIntegrationResponse 200).
2. CreateDocumentationPart(restApiId, location={"type":"API"}, properties='{"description":"v1-description"}') -> documentationPartId.
3. CreateDocumentationVersion(restApiId, documentationVersion="1.0.0", description="snapshot-1").
4. UpdateDocumentationPart(restApiId, documentationPartId, patchOperations=[{op:"replace", path:"/properties", value:'{"description":"v2-description"}'}]).
5. GetDocumentationPart(restApiId, documentationPartId) -> properties.description == "v2-description" (live part reflects the edit; this positive control passes).
6. CreateDeployment(restApiId) -> deploymentId; CreateStage(restApiId, stageName=<stage>, deploymentId, documentationVersion="1.0.0").
7. GetExport(restApiId, stageName=<stage>, exportType="swagger", parameters={"extensions":"documentation"}, accepts="application/json").
   Observed: HTTP 400 BadRequestException "Invalid extensions specified: documentation; extensions must be one of integrations, apigateway, authorizers".
   Expected: HTTP 200 with a body whose x-amazon-apigateway-documentation.documentationParts contains exactly one location.type=="API" part with properties.description == "v1-description" (the frozen snapshot), not "v2-description".
8. Same call with parameters={"extensions":"postman"} is refused identically, though the model documents that value.
