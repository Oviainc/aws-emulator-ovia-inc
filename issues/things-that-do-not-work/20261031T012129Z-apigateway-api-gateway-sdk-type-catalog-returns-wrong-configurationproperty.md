Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-catalog-get-sdk-type
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-catalog-get-sdk-type apigateway-import-rest-api-deploy-export-sdk apigateway-rest-api-update-list-and-sdk-generation apigateway-sdk-type-catalog-and-generation apigateway-catalog-get-sdk-types
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.artifact-id"].description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.artifact-version"].description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.build-system"].description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.group-id"].description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="ruby.gem-name"].description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="ruby.gem-version"].description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="service.name"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="android"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.artifact-id"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.artifact-version"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.build-system"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.group-id"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="javascript"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="objectivec"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].configurationProperties[name="ruby.gem-name"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].configurationProperties[name="ruby.gem-version"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].configurationProperties[name="service.name"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="swift"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[*].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].configurationProperties[*].description
Issue-Commit: 64b86d5e14a7a25edd6b553817d01f64764c1ea9

# API Gateway SDK-type catalog returns wrong configurationProperty descriptions and extra SDK types (4 causes)

> _Note: this issue was identified against the code base at commit `64b86d5e14a7a25edd6b553817d01f64764c1ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSdkTypes

## Description
Four net-new wire divergence clusters (service=apigateway) share one root cause: the static SDK-type catalog the emulator serves from GetSdkType and GetSdkTypes carries its own, paraphrased prose for every `SdkConfigurationProperty.description`, and the catalog enumerates more SDK types than API Gateway actually offers. Every step returns the correct HTTP status (200); only the catalog content diverges.

Shared shape of the defect — AWS: Smithy `com.amazonaws.apigateway#GetSdkType` (GET /sdktypes/{id}, code 200, output shape `SdkType` with members id/friendlyName/description/configurationProperties -> ListOfSdkConfigurationProperty) and `GetSdkTypes` (GET /sdktypes, code 200, output shape `SdkTypes`, list member serialized with jsonName `item`). The model constrains neither the description strings nor the set of SDK types, so the catalog content is recorded behavior only: the recordings show a fixed, service-authored description per configuration property and a catalog containing exactly the `java` and `ruby` SDK types. Emulator: it serves its own wording for each description and six catalog entries. Why: the SDK-type catalog is reference data — a client (console, CLI, or a generator front-end) renders these descriptions verbatim to a user and enumerates the catalog to decide which SDKs it may request from GetSdk, so paraphrased text is user-visible drift and the four surplus entries advertise generators that API Gateway no longer offers, making a subsequent GetSdk request for `android`/`javascript`/`objectivec`/`swift` appear supported when AWS does not list it. Fix: the static SDK-type catalog backing the GetSdkType/GetSdkTypes handlers in the apigateway service crate (the table of SdkType entries and their SdkConfigurationProperty descriptions), not the serializer — the envelope and status are already correct.

Cause 1 of 4: GetSdkType for the `java` SDK type returns four paraphrased configurationProperty descriptions. AWS (recorded): `configurationProperties[name="java.build-system"].description` = 'Build system to setup for project; Currently supported: maven, gradle'; `java.group-id` = 'Group id for your Maven or Gradle project. Defaults to package name'; `java.artifact-id` = 'Artifact Id for your Maven project or project name for your Gradle project. Defaults to service name'; `java.artifact-version` = 'Version of your Maven or Gradle project. Defaults to 1.0-SNAPSHOT'. These strings are entirely fixed text — they contain no request- or state-derived value. Emulator: 'Build system to setup for project; Allowed values are maven and gradle; Default is maven', 'groupId of your Maven project; Default is the package name', 'artifactId of your Maven project; Default is the service name', 'Version of your Maven project; Default is 1.0-SNAPSHOT'. Status 200 on both sides. Trace: apigateway-catalog-get-sdk-type#task#1 (cluster also covers apigateway-import-rest-api-deploy-export-sdk#task#5 and apigateway-rest-api-update-list-and-sdk-generation#task#11).

Cause 2 of 4: GetSdkType for the `ruby` SDK type returns three paraphrased configurationProperty descriptions. AWS (recorded, all fixed text): `service.name` = 'Name of the service which is used to derive the name for your client'; `ruby.gem-name` = 'Name of the Ruby gem your code will be generated under'; `ruby.gem-version` = 'Version number for your service gem. Defaults to 1.0.0'. Emulator: 'Name of the service which is used to derive the name of the Ruby client class', 'Name of the Ruby gem; Default is the service name', 'Version of the Ruby gem; Default is 1.0.0'. Status 200 on both sides. Trace: apigateway-catalog-get-sdk-type#task#2 (cluster also covers apigateway-sdk-type-catalog-and-generation#task#3).

Cause 3 of 4: GetSdkTypes returns the same seven paraphrased descriptions inside the `item` list AND four SDK types the recording does not contain. AWS (recorded): the `item` list holds exactly the `java` and `ruby` SDK types, each with the description strings quoted in causes 1 and 2. Emulator: the same list additionally contains `item[id="javascript"]` (friendlyName 'JavaScript SDK', description 'JavaScript SDK generator for API Gateway APIs', configurationProperties []), plus `item[id="android"]`, `item[id="objectivec"]` and `item[id="swift"]` — reported as `extra (expected None, got {...})` for each. Status 200 on both sides. Trace: apigateway-catalog-get-sdk-types#task#1.

Cause 4 of 4: the same seven description divergences are reported again from GetSdkTypes when the catalog is read positionally (`item[id="java"].configurationProperties[2..5].description` and `item[id="ruby"].configurationProperties[0..2].description`), confirming the drift is in the catalog data rather than in one response path. Status 200 on both sides. Trace: apigateway-import-rest-api-deploy-export-sdk#task#4 (cluster also covers apigateway-rest-api-update-list-and-sdk-generation#task#10 and apigateway-sdk-type-catalog-and-generation#task#2).

No open issue carries these replay keys (`owned_by` was empty for all four clusters and no stub blocks them). `20261029T193654Z-apigateway-api-gateway-refusal-contracts-part-2-10-root.md` mentions the SDK-type surface but only its refusal contracts, and `20261029T200808Z-apigateway-api-gateway-list-envelopes-and-openapi-export-serialization.md` concerns list envelopes, so neither covers the catalog content above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (193 fixtures, 1873 steps; 103 divergence clusters). Four clusters with empty `owned_by` and empty `stub_path`, all service=apigateway:

1. GetSdkType, apigateway-catalog-get-sdk-type#task#1, expected 200 / got 200, affected_count 3: `$.configurationProperties[name="java.build-system"].description: value_mismatch (expected 'Build system to setup for project; Currently supported: maven, gradle', got 'Build system to setup for project; Allowed values are maven and gradle; Default is maven')`; `[name="java.group-id"].description: value_mismatch (expected 'Group id for your Maven or Gradle project. Defaults to package name', got 'groupId of your Maven project; Default is the package name')`; `[name="java.artifact-id"].description: value_mismatch (expected 'Artifact Id for your Maven project or project name for your Gradle project. Defaults to service name', got 'artifactId of your Maven project; Default is the service name')`; `[name="java.artifact-version"].description: value_mismatch (expected 'Version of your Maven or Gradle project. Defaults to 1.0-SNAPSHOT', got 'Version of your Maven project; Default is 1.0-SNAPSHOT')`.
2. GetSdkType, apigateway-catalog-get-sdk-type#task#2, 200/200, affected_count 2: `$.configurationProperties[name="service.name"].description: value_mismatch (expected 'Name of the service which is used to derive the name for your client', got 'Name of the service which is used to derive the name of the Ruby client class')`; `[name="ruby.gem-name"].description: value_mismatch (expected 'Name of the Ruby gem your code will be generated under', got 'Name of the Ruby gem; Default is the service name')`; `[name="ruby.gem-version"].description: value_mismatch (expected 'Version number for your service gem. Defaults to 1.0.0', got 'Version of the Ruby gem; Default is 1.0.0')`.
3. GetSdkTypes, apigateway-catalog-get-sdk-types#task#1, 200/200: the seven description mismatches above repeated under `$.item[id="java"]`/`$.item[id="ruby"]`, plus `$.item[id="javascript"]: extra (expected None, got {'id': 'javascript', 'friendlyName': 'JavaScript SDK', 'description': 'JavaScript SDK generator for API Gateway APIs', 'configurationProperties': []})` and the same `extra` verdict for `$.item[id="android"]`, `$.item[id="objectivec"]` and `$.item[id="swift"]` (divergence_key lists all four).
4. GetSdkTypes, apigateway-import-rest-api-deploy-export-sdk#task#4, 200/200, affected_count 3: `$.item[id="java"].configurationProperties[2..5].description` and `$.item[id="ruby"].configurationProperties[0..2].description` carry the identical expected-vs-actual pairs.

Smithy consulted (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json): GetSdkType — input id (httpLabel, required), output SdkType {id, friendlyName, description, configurationProperties -> ListOfSdkConfigurationProperty}, http GET /sdktypes/{id} code 200, errors BadRequestException 400 / NotFoundException 404 / TooManyRequestsException 429 / UnauthorizedException 401; GetSdkTypes — output SdkTypes {items -> ListOfSdkType, jsonName 'item'}, http GET /sdktypes code 200. The model places no constraint on the description strings or on the set of SDK types, so those are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay apigateway-catalog-get-sdk-type, step task#1 (GET /sdktypes/java) and compare each `configurationProperties[*].description` with the recorded response — all four java properties carry the emulator's own wording.
3. Cause 2: same fixture, step task#2 (GET /sdktypes/ruby) — the three ruby/service properties carry the emulator's own wording.
4. Cause 3: replay apigateway-catalog-get-sdk-types, step task#1 (GET /sdktypes) — the `item` list carries the same wrong descriptions and four surplus entries (android, javascript, objectivec, swift) that the recording does not contain.
5. Cause 4: replay apigateway-import-rest-api-deploy-export-sdk, step task#4 (GET /sdktypes) — the same descriptions diverge when read positionally.
6. All four steps answer 200, as the recording does; only the catalog content diverges.
