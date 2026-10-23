Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-put-rest-api-merge-and-overwrite
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-put-rest-api-merge-and-overwrite apigateway-put-rest-api-merge-overwrite-lifecycle apigateway-put-rest-api-merge-then-overwrite
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetResources|2xx->2xx|$.item[*]
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# PutRestApi import does not create resources from the body

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRestApi

## Description
Three causes, one shared fault: PutRestApi accepts the OpenAPI document and answers success, but the resource tree described by that document is never materialized, so the following GetResources returns only the root resource. Both `merge` and `overwrite` modes are affected.

Cause 1 of 3: PutRestApi in merge mode does not add the imported paths.
AWS: PutRestApi is PUT /restapis/{restApiId} with a required `body` blob (@httpPayload, the OpenAPI definition), a `mode` query member (PutMode) and success 200 returning RestApi; the imported definition's paths become Resource entities, so GetResources (GET /restapis/{restApiId}/resources, 200, items jsonName `item`, each a Resource {id, parentId, pathPart, path}) lists the root plus one resource per imported path. The recording shows the root followed by `/pets` and `/items`, each with parentId set to the root resource id.
Emulator: GetResources returns only the root element — `$.item[1]` and `$.item[2]` are missing entirely.
Why: the whole import feature is a no-op on state while reporting success. A client that imports a definition and then wires integrations or deployments against the imported resource ids finds no resources and cannot proceed; nothing in the response signals the failure.
Fix: the PutRestApi handler — parse the body payload and create/merge the Resource tree (and the methods hanging off it) into state for both PutMode values; GetResources then lists them from state.
Traces: fixture `apigateway-put-rest-api-merge-and-overwrite`, step `task#4`.

Cause 2 of 3: PutRestApi in overwrite mode leaves only the root resource.
AWS: in overwrite mode the existing definition is replaced by the imported one, so GetResources lists the imported paths (recording: `/pets` and `/orders`, both parented to the root).
Emulator: the first list element is still the bare root (`got {'id': '<root>', 'path': '/'}` where the recording has the `/pets` resource) and the second element is missing — i.e. nothing was imported and the ordering/length of `item` diverges as a result.
Why: as Cause 1; additionally a client that re-imports to overwrite believes it replaced the API surface when the API in fact has no paths.
Fix: as Cause 1, in the overwrite branch of the PutRestApi handler.
Traces: fixture `apigateway-put-rest-api-merge-overwrite-lifecycle`, step `task#4`.

Cause 3 of 3: merge-then-overwrite sequence loses the second imported path.
AWS: after the merge followed by the overwrite the recording lists an `/orders` resource at `$.item[1]`.
Emulator: `$.item[1]` is missing.
Why/Fix: as Causes 1 and 2 — recorded separately because this unit exercises the sequence of both modes against one API, which must also converge on the recorded resource list.
Traces: fixture `apigateway-put-rest-api-merge-then-overwrite`, step `task#7`.

## Evidence
Representative (Cause 1): fixture apigateway-put-rest-api-merge-and-overwrite step task#4, operation GetResources, expected HTTP 200 / actual HTTP 200, divergence_key `service=apigateway|GetResources|status=200->200|keys=$.item[1],$.item[2]`, detail `$.item[1]: missing (expected {'id': '<shaped-not-asserted>', 'parentId': '<root-id>', 'pathPart': 'pets', 'path': '/pets'}, got None)` / `$.item[2]: missing (expected {... 'pathPart': 'items', 'path': '/items'}, got None)`.
Cause 2: apigateway-put-rest-api-merge-overwrite-lifecycle#task#4 — `$.item[0]: missing (expected {'parentId': '<root-id>', 'path': '/pets', 'pathPart': 'pets'}, got {'id': '<root-id>', 'path': '/'})` and `$.item[1]: missing (expected {... 'path': '/orders' ...}, got None)`.
Cause 3: apigateway-put-rest-api-merge-then-overwrite#task#7 — `$.item[1]: missing (expected {'path': '/orders'}, got None)`.
The PutRestApi step itself reports success in each fixture; the divergence surfaces on the following GetResources, so the import handler accepts and discards the definition.
Smithy evidence: PutRestApi PUT /restapis/{restApiId} code 200, input {restApiId @httpLabel @required, mode: PutMode @httpQuery, failOnWarnings, parameters, body: Blob @httpPayload @required}, output RestApi; GetResources GET /restapis/{restApiId}/resources code 200, output items (jsonName `item`) of Resource {id, parentId, pathPart, path, resourceMethods}.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigateway-put-rest-api-merge-and-overwrite` and inspect step `task#4`: GetResources returns only the root resource, while the recording lists the imported `/pets` and `/items` resources parented to the root.
3. Replay `apigateway-put-rest-api-merge-overwrite-lifecycle` step `task#4` (overwrite mode) and `apigateway-put-rest-api-merge-then-overwrite` step `task#7` (merge then overwrite) and observe the same missing `item` elements.
4. All three units must list the imported resources before this issue is closed.
