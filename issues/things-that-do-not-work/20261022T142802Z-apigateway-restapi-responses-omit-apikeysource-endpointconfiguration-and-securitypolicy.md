Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-authorizer-lifecycle-test-invoke
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-authorizer-lifecycle-test-invoke apigateway-documentation-part-error-paths apigateway-documentation-part-lifecycle apigateway-documentation-parts-crud-and-import apigateway-documentation-parts-multiple-locations-filtering apigateway-documentation-version-crud apigateway-documentation-version-crud-lifecycle apigateway-documentation-version-error-paths apigateway-documentation-version-errors apigateway-documentation-version-lifecycle apigateway-documentation-version-lifecycle-basic apigateway-documentation-version-multiple-list apigateway-documentation-versions-crud-lifecycle apigateway-duplicate-location-conflict-error apigateway-gateway-response-default-override apigateway-gateway-response-full-crud-lifecycle apigateway-gateway-response-lifecycle apigateway-gateway-response-multiple-types-and-errors apigateway-gateway-responses-listing-and-notfound-errors apigateway-gatewayresponse-lifecycle apigateway-gatewayresponse-multiple-types apigateway-import-documentation-parts apigateway-import-documentation-parts-merge apigateway-import-documentation-parts-merge-and-overwrite apigateway-integration-and-responses-workflow apigateway-isolated-multi-response-type-customization apigateway-list-gateway-responses apigateway-models-request-validators apigateway-multi-resource-tag-isolation apigateway-multiple-documentation-versions apigateway-put-rest-api-merge-and-overwrite apigateway-resource-and-method-documentation apigateway-resource-method-crud-lifecycle apigateway-resource-method-integration-full-lifecycle apigateway-rest-api-lifecycle apigateway-rest-api-tag-lifecycle apigateway-rest-api-update-list-and-sdk-generation apigateway-stage-api-key-toggle apigateway-stage-restricted-api-key-toggle apigateway-tag-isolation-across-resources apigateway-tag-operations-error-paths apigateway-tag-rest-api-lifecycle apigateway-tag-restapi-add-update-remove-lifecycle apigateway-tag-stage-workflow-and-cleanup apigateway-tag-untag-rest-api-lifecycle apigateway-tagging-error-paths apigateway-deployment-and-stage-lifecycle apigateway-documentation-part-crud-lifecycle apigateway-documentation-parts-bulk-import
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateRestApi|2xx->2xx|$.apiKeySource
Issue-Key: apigateway|CreateRestApi|2xx->2xx|$.endpointConfiguration
Issue-Key: apigateway|CreateRestApi|2xx->2xx|$.securityPolicy
Issue-Key: apigateway|CreateRestApi|2xx->2xx|$.endpointConfiguration.ipAddressType
Issue-Key: apigateway|GetRestApi|2xx->2xx|$.apiKeySource
Issue-Key: apigateway|GetRestApi|2xx->2xx|$.endpointConfiguration
Issue-Key: apigateway|GetRestApi|2xx->2xx|$.securityPolicy
Issue-Key: apigateway|GetRestApi|2xx->2xx|$.tags
Issue-Key: apigateway|GetRestApi|2xx->2xx|$.description
Issue-Key: apigateway|GetRestApis|2xx->2xx|$.item[name="sim2real-probe-apigateway-restapi-53fd527e"].apiKeySource
Issue-Key: apigateway|GetRestApis|2xx->2xx|$.item[name="sim2real-probe-apigateway-restapi-53fd527e"].description
Issue-Key: apigateway|GetRestApis|2xx->2xx|$.item[name="sim2real-probe-apigateway-restapi-53fd527e"].endpointConfiguration
Issue-Key: apigateway|GetRestApis|2xx->2xx|$.item[name="sim2real-probe-apigateway-restapi-53fd527e"].securityPolicy
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# RestApi responses omit apiKeySource endpointConfiguration and securityPolicy

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRestApi

## Description
Five causes, one shared fault: every operation that serializes the RestApi shape omits the members AWS populates with service-side defaults when the request did not set them — apiKeySource, endpointConfiguration (both its types list and its ipAddressType) and securityPolicy. This is the single highest-fanout divergence in the corpus (46 affected steps on the CreateRestApi cluster alone).

Cause 1 of 5: CreateRestApi response omits the defaulted members.
AWS: CreateRestApi is POST /restapis with success 201 and output shape RestApi, whose members include apiKeySource (ApiKeySourceType enum: HEADER, AUTHORIZER), endpointConfiguration (EndpointConfiguration {types: ListOfEndpointType, ipAddressType: IpAddressType, vpcEndpointIds}) and securityPolicy (SecurityPolicy enum, first value TLS_1_0) — Smithy: CreateRestApi input/output as above. None of the three is required on input; AWS fills them in on create and echoes them back. The recording shows apiKeySource `HEADER`, endpointConfiguration `{types: ['EDGE'], ipAddressType: 'ipv4'}` and securityPolicy `TLS_1_0` for an edge-optimized API created without those inputs.
Emulator: all three members are absent from the 201 body (`$.apiKeySource: missing`, `$.endpointConfiguration: missing`, `$.securityPolicy: missing`); status matches (201).
Why: a client that reads restApi.endpointConfiguration.types to decide edge vs regional behavior, or apiKeySource to decide where to send the API key, gets null and either branches wrongly or null-dereferences. Typed SDKs surface these as unset optionals where AWS always returns a value for a created API.
Fix: the CreateRestApi handler's construction of the RestApi entity (default these members at create time in state), plus the RestApi serializer so the stored values are emitted.
Traces: fixture `apigateway-authorizer-lifecycle-test-invoke`, step `setup#1`; 46 affected steps across the documentation-part, documentation-version, gateway-response and integration fixtures.

Cause 2 of 5: CreateRestApi omits apiKeySource, securityPolicy and the nested endpointConfiguration.ipAddressType.
AWS: as Cause 1; EndpointConfiguration.ipAddressType is documented as ipv4 or dualstack and is populated for a created API.
Emulator: here the endpointConfiguration object itself is present but its `ipAddressType` is missing, alongside the missing apiKeySource and securityPolicy — so the defaulting gap also exists one level down in the nested structure, not only at the top level.
Why: a client reading ipAddressType to choose an IPv4/dualstack call path sees nothing.
Fix: default and serialize the nested EndpointConfiguration member as well as the top-level ones.
Traces: fixture `apigateway-deployment-and-stage-lifecycle`, step `task#8` (also apigateway-documentation-part-crud-lifecycle#setup#1, apigateway-documentation-parts-bulk-import#setup#1).

Cause 3 of 5: GetRestApi omits the same members plus the empty `tags` map.
AWS: GetRestApi returns the same RestApi shape, so a read of an API created without those inputs still carries apiKeySource HEADER, endpointConfiguration {types:['EDGE'], ipAddressType:'ipv4'}, securityPolicy TLS_1_0 and `tags: {}`.
Emulator: `$.apiKeySource`, `$.endpointConfiguration`, `$.securityPolicy` and `$.tags` all missing from the 200 body.
Why: the defaulting gap is in the read path too, so it is not merely a create-response omission — nothing a client ever reads back carries these members.
Fix: the RestApi serializer shared by GetRestApi/GetRestApis plus the create-time defaults in state.
Traces: fixture `apigateway-rest-api-lifecycle`, step `task#2` (also apigateway-tag-restapi-add-update-remove-lifecycle#task#5).

Cause 4 of 5: GetRestApi after UpdateRestApi omits the same members.
Emulator: `$.apiKeySource`, `$.endpointConfiguration`, `$.securityPolicy`, `$.tags` missing. The same step additionally shows `$.description: value_mismatch (expected 'updated description', got 'initial description')` — that un-applied PATCH is the separate fault filed as the "PATCH update operations are accepted but never applied" issue; this cause is the missing RestApi members only, but the step will not replay clean until both are fixed.
Why/Fix: as Cause 3.
Traces: fixture `apigateway-rest-api-lifecycle`, step `task#4`.

Cause 5 of 5: GetRestApis list items omit the same members.
AWS: GetRestApis returns `item` (jsonName of the items list) of RestApi, each element carrying the same defaulted members.
Emulator: the matched list element is missing apiKeySource, endpointConfiguration and securityPolicy (and carries the stale description noted in Cause 4).
Why: list and get disagree with AWS identically, so paginating clients cannot recover the values either.
Fix: the shared RestApi element serializer used by the list path.
Traces: fixture `apigateway-rest-api-lifecycle`, step `task#5`.

## Evidence
Representative (Cause 1): fixture apigateway-authorizer-lifecycle-test-invoke step setup#1, operation CreateRestApi, expected HTTP 201 / actual HTTP 201, divergence_key `service=apigateway|CreateRestApi|status=201->201|keys=$.apiKeySource,$.endpointConfiguration,$.securityPolicy`, detail `$.apiKeySource: missing (expected 'HEADER', got None)` / `$.endpointConfiguration: missing (expected {'ipAddressType': 'ipv4', 'types': ['EDGE']}, got None)` / `$.securityPolicy: missing (expected 'TLS_1_0', got None)`; affected_count=46.
Cause 2: apigateway-deployment-and-stage-lifecycle#task#8 — `$.apiKeySource: missing`, `$.endpointConfiguration.ipAddressType: missing (expected 'ipv4', got None)`, `$.securityPolicy: missing`; affected_count=3.
Cause 3: apigateway-rest-api-lifecycle#task#2 — GetRestApi 200/200, `$.apiKeySource`, `$.endpointConfiguration`, `$.securityPolicy`, `$.tags: missing (expected {}, got None)`.
Cause 4: apigateway-rest-api-lifecycle#task#4 — same four missing members (plus stale `$.description`).
Cause 5: apigateway-rest-api-lifecycle#task#5 — GetRestApis `$.item[name=...].apiKeySource/.endpointConfiguration/.securityPolicy: missing`.
Smithy evidence: CreateRestApi POST /restapis code 201, output shape RestApi with members apiKeySource (ApiKeySourceType: HEADER|AUTHORIZER), endpointConfiguration (EndpointConfiguration{types, ipAddressType, vpcEndpointIds}), securityPolicy (SecurityPolicy, includes TLS_1_0), tags (MapOfStringToString); GetRestApis output items has jsonName `item`.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigateway-authorizer-lifecycle-test-invoke` and inspect step `setup#1`: the CreateRestApi 201 body has no apiKeySource, endpointConfiguration or securityPolicy where the recording has HEADER / {types:['EDGE'], ipAddressType:'ipv4'} / TLS_1_0.
3. Replay `apigateway-deployment-and-stage-lifecycle` step `task#8` for the nested endpointConfiguration.ipAddressType omission.
4. Replay `apigateway-rest-api-lifecycle` steps `task#2`, `task#4` and `task#5` for the GetRestApi and GetRestApis read paths (also expecting `tags: {}`).
5. All five units must carry the defaulted RestApi members before this issue is closed.
