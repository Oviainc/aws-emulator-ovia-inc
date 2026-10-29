Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-import-api-basepath-split
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-import-api-basepath-split apigatewayv2-import-export-reimport-api-round-trip apigatewayv2-integration-aws-proxy-crud apigatewayv2-model-template-by-schema-type apigatewayv2-quick-create-managed-resources apigatewayv2-routes-pagination-max-results-one
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetRoutes|2xx->2xx|$.items[*].routeId
Issue-Key: apigateway|GetRoutes|2xx->2xx|$.items[*].target
Issue-Key: apigateway|CreateIntegration|2xx->2xx|$.integrationId
Issue-Key: apigateway|CreateModel|2xx->2xx|$.modelId
Issue-Commit: 900a7beaab356ed52889492ca7befa08b17bdc0c

# apigateway generated resource identifiers diverge from the recorded id format

> _Note: this issue was identified against the code base at commit `900a7beaab356ed52889492ca7befa08b17bdc0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoutes

## Description
Six net-new wire divergences on the API Gateway v2 surface served under the `apigateway` service token share one root cause: the identifier the emulator mints for every child resource (route, integration, model) is out of family with the identifier shape AWS returns, and that identifier then leaks into every member that embeds it (`Route.target` = `integrations/<integrationId>`). A sixth, related cause is that the route-to-integration binding is not preserved across an import/export/reimport round trip.

Shared shape of the defect — AWS: Smithy `com.amazonaws.apigatewayv2#Id` (the target of `Route.routeId`, `Route.authorizerId`, `Integration.integrationId`, `Model.modelId`) is an unconstrained string with no @pattern and no @length, so the concrete format is recorded behavior only. In every recording the server-minted id is a short, 6-7 character, lowercase-alphanumeric opaque token (recorded examples from this corpus: routeId `kw8ui5s`, integrationId `5jiki1c`, authorizerId `d6tyvv`, modelId `e94sid`, routeResponseId `47kks8`), and `Route.target` is the literal `integrations/` followed by an id of that same shape. Emulator: it answers a 10-character token whose final three characters are always the constant literal `aaa` (`oowklxjaaa`, `nmdy4dkaaa`, `mkuboukaaa`, `7pwxqqraaa`, `8rfk79qaaa`), so the value fails the recorded shape for that member regardless of which concrete id a given run produces. The fixed `aaa` tail also shows the generator is a per-process counter rather than an independent per-resource token, so the same id value recurs for unrelated resources across a run. Why: a client that validates, indexes, or round-trips API Gateway v2 ids against the documented short-opaque-id family — including anything that parses `integrations/<id>` out of `Route.target`, or that treats an id as unique across APIs — accepts a value real AWS never emits, and a per-run-stable counter value makes two different resources collide in client-side caches keyed by id. Fix: the shared identifier-minting helper in the apigatewayv2 service crate's state module, so that every Create*/Get* handler that returns an `Id`-typed member (and every serializer that composes `integrations/<id>` into `Route.target`) emits an id of the recorded shape; the route/integration binding in cause 3 belongs in the ReimportApi handler's state update.

Cause 1 of 6 — GetRoutes returns route ids and targets of the wrong shape after an import that splits a base path. AWS: `$.items[*].routeId` is a short opaque id and `$.items[*].target` is `integrations/<id>` of the same shape. Emulator: `$.items[0].routeId` = `oowklxjaaa`, `$.items[0].target` = `integrations/nmdy4dkaaa`, `$.items[1].routeId` = `8rfk79qaaa`, `$.items[1].target` = `integrations/7pwxqqraaa` — all 10 characters with the constant `aaa` tail, so all four asserted members fail their recorded shape while the status is correctly 200. Trace: apigatewayv2-import-api-basepath-split#task#2 (also task#4, task#6).

Cause 2 of 6 — the same GetRoutes shape divergence on the import/export/reimport round trip: `$.items[0].routeId` = `oowklxjaaa` and `$.items[0].target` carries the same mis-shaped integration id, status 200 on both sides. Trace: apigatewayv2-import-export-reimport-api-round-trip#task#2.

Cause 3 of 6 — after ReimportApi of the API, GetRoutes reports a route whose `target` names a DIFFERENT integration than the one the same fixture observed before the round trip. AWS: recorded behavior only — in the recording the route's target after the round trip is still the integration the earlier step reported, i.e. re-importing a spec whose path/method is unchanged leaves the route bound to the existing integration. Emulator: the reimport mints a fresh integration and rebinds the route to it, so `$.items[0].target` moves from `integrations/<id captured earlier>` to `integrations/<a newly minted id>` (observed `integrations/nmdy4dkaaa` -> `integrations/7pwxqqraaa`); status 200 on both sides. Why: a client that captured an integration id before a reimport (to patch the integration, to read it, or to delete it) finds that id orphaned and the route pointing elsewhere. Trace: apigatewayv2-import-export-reimport-api-round-trip#task#6.

Cause 4 of 6 — CreateIntegration returns an `integrationId` of the wrong shape. AWS: 201 with `$.integrationId` a short opaque id (recorded family, e.g. `5jiki1c`). Emulator: 201 with `$.integrationId` = `nmdy4dkaaa`. Trace: apigatewayv2-integration-aws-proxy-crud#task#2.

Cause 5 of 6 — CreateModel returns a `modelId` of the wrong shape. AWS: 201 with `$.modelId` a short opaque id (recorded family, e.g. `e94sid`). Emulator: 201 with `$.modelId` = `mkuboukaaa`, identically for every model created in the fixture, which also shows the counter is not advanced per resource of the same kind within the run. Trace: apigatewayv2-model-template-by-schema-type#task#3 (also task#5, task#7, task#9).

Cause 6 of 6 — GetRoutes on a quick-create API and on the paginated route listing returns route ids of the wrong shape: `$.items[0].routeId` = `oowklxjaaa`, status 200 on both sides. Trace: apigatewayv2-quick-create-managed-resources#task#3 (also apigatewayv2-routes-pagination-max-results-one#task#5, #task#6, #task#7).

No open issue carries these replay keys. The open issues `20261029T200808Z-apigateway-api-gateway-integration-resource-representation-drops-responsetransfermode-and.md` ("...and alters ids") and `20261029T220031Z-apigateway-resource-representations-omit-derived-members-and-drop.md` are adjacent in subject matter but do not own any of the six fixture steps above, so they do not cover them.

## Evidence
Replay of trace set simparity-wire-aws-v1 (95 fixtures, 799 steps). Six clusters with no owning issue and no blocking stub, all service=apigateway:

1. GetRoutes, apigatewayv2-import-api-basepath-split#task#2, expected 200 / got 200: `$.items[0].routeId: value_mismatch (expected '<shaped-not-asserted>', got 'oowklxjaaa')`; `$.items[0].target: value_mismatch (expected '<shaped-not-asserted>', got 'integrations/nmdy4dkaaa')`; `$.items[1].routeId: ... got '8rfk79qaaa'`; `$.items[1].target: ... got 'integrations/7pwxqqraaa'`. affected_count 3.
2. GetRoutes, apigatewayv2-import-export-reimport-api-round-trip#task#2, 200/200: `$.items[0].routeId: ... got 'oowklxjaaa'`; `$.items[0].target: value_mismatch (expected '{{route_target_after}}', got 'integrations/nmdy4dkaaa')`.
3. GetRoutes, apigatewayv2-import-export-reimport-api-round-trip#task#6, 200/200: `$.items[0].target: value_mismatch (expected 'integrations/nmdy4dkaaa', got 'integrations/7pwxqqraaa')` — the expected value is the integration the fixture captured earlier in the same replay, so the emulator rebound the route to a newly minted integration across the reimport.
4. CreateIntegration, apigatewayv2-integration-aws-proxy-crud#task#2, 201/201: `$.integrationId: value_mismatch (expected '<shaped-not-asserted>', got 'nmdy4dkaaa')`.
5. CreateModel, apigatewayv2-model-template-by-schema-type#task#3, 201/201: `$.modelId: value_mismatch (expected '<shaped-not-asserted>', got 'mkuboukaaa')`. affected_count 4 (task#3/5/7/9 all report the identical modelId).
6. GetRoutes, apigatewayv2-quick-create-managed-resources#task#3, 200/200: `$.items[0].routeId: value_mismatch (expected '<shaped-not-asserted>', got 'oowklxjaaa')`. affected_count 4.

Recorded AWS id family observed elsewhere in the same corpus: authorizerId `d6tyvv`, integrationId `5jiki1c`, routeId `kw8ui5s`, deploymentId `re2gib`, modelId `e94sid`, routeResponseId `47kks8` — 6-7 chars, [a-z0-9]. Emulator family: 10 chars ending in the constant `aaa`.

Smithy consulted: apigatewayv2 `Id` (unconstrained string, doc "The identifier."), `Route.routeId`/`Route.target` (target `StringWithLengthBetween1And128`), `CreateIntegration` output `integrationId` -> `Id` (http POST /v2/apis/{ApiId}/integrations, code 201), `CreateModel` output `modelId` -> `Id` (code 201), `GetRoutes` (GET /v2/apis/{ApiId}/routes, code 200).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay apigatewayv2-import-api-basepath-split and inspect step task#2 (GetRoutes) — every `items[*].routeId` and the integration id embedded in `items[*].target` is a 10-character token ending in `aaa` instead of a short opaque id.
3. Cause 2/3: replay apigatewayv2-import-export-reimport-api-round-trip; compare the route `target` reported at task#2 with the one at task#6 — the emulator changes the target integration id across the ReimportApi, where the recording keeps the previously observed one.
4. Cause 4: replay apigatewayv2-integration-aws-proxy-crud step task#2 (CreateIntegration) and read `$.integrationId`.
5. Cause 5: replay apigatewayv2-model-template-by-schema-type steps task#3/5/7/9 (CreateModel) and read `$.modelId` — the same value is returned for every model.
6. Cause 6: replay apigatewayv2-quick-create-managed-resources step task#3 and apigatewayv2-routes-pagination-max-results-one steps task#5-7 (GetRoutes) and read `$.items[0].routeId`.
7. All six steps return the AWS status; only the identifier-shaped members diverge.
