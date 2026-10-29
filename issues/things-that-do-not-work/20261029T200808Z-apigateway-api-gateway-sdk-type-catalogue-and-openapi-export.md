Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-catalog-get-sdk-type
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-catalog-get-sdk-type apigateway-sdk-type-catalog-and-generation apigateway-catalog-get-sdk-types apigateway-import-rest-api-deploy-export-sdk apigateway-rest-api-update-list-and-sdk-generation apigateway-error-paths apigateway-error-paths-invalid-references apigateway-import-export-sdk-error-paths apigateway-import-put-export-sdk-error-paths apigateway-invalid-input-error-paths apigateway-deploy-and-export-api-definition apigateway-import-rest-api-deploy-export apigateway-documentation-parts-versions-and-export apigateway-import-configure-deploy-export-sdk
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.artifact-id"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.artifact-version"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.build-system"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.group-id"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.license-text"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="java.package-name"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="javaPackageName"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="service.name"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[name="serviceName"]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.description
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.configurationProperties[*]
Issue-Key: apigateway|GetSdkType|2xx->2xx|$.friendlyName
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="android"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.artifact-id"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.artifact-version"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.build-system"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.group-id"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.license-text"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="java.package-name"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="javaPackageName"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="service.name"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[name="serviceName"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="javascript"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="objectivec"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].configurationProperties[*]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].description
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="ruby"].friendlyName
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="swift"]
Issue-Key: apigateway|GetSdkTypes|2xx->2xx|$.item[id="java"].configurationProperties[*]
Issue-Key: apigateway|GetSdkType|4xx->4xx|$.message
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./pets.get.consumes
Issue-Key: apigateway|GetExport|2xx->2xx|$.x-amazon-apigateway-security-policy
Issue-Key: apigateway|GetExport|2xx->2xx|$
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./items.get.consumes
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./.get.consumes
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./.post.consumes
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./.post.produces
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway SDK-type catalogue and OpenAPI export content diverge (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSdkType

## Description
The SDK-type catalogue is emulator-authored rather than AWS's, and GetExport omits fields AWS always writes. Smithy: SdkType = {id, friendlyName, description, configurationProperties: ListOfSdkConfigurationProperty{name, friendlyName, description, required, defaultValue}}; GetSdkType models NotFoundException @httpError 404; GetExport returns the definition as its payload.

Cause 1 of 10 - GetSdkType 'java'. AWS serves seven configurationProperties, named `service.name` (required), `java.package-name` (required), `java.build-system`, `java.group-id`, `java.artifact-id`, `java.artifact-version`, `java.license-text`, each with AWS's friendlyName and description, plus description 'Java SDK generator for API Gateway APIs'. The emulator serves two invented properties (`serviceName`, `javaPackageName`) and its own description text. Why: a client that drives SDK generation by setting the documented property names cannot configure anything. Fix: the static SDK-type catalogue. Trace: apigateway-catalog-get-sdk-type#task#1.

Cause 2 of 10 - GetSdkType 'ruby'. AWS: friendlyName 'Ruby', description 'Ruby SDK generator for API Gateway APIs', configurationProperties [service.name (required), ruby.gem-name, ruby.gem-version]. Emulator: friendlyName 'Ruby SDK', its own description, no properties. Trace: apigateway-catalog-get-sdk-type#task#2 (2 steps).

Cause 3 of 10 - GetSdkTypes catalogue membership. AWS lists android, java, javascript, objectivec, ruby and swift; the emulator's list is missing android, javascript, objectivec and swift, and its java/ruby entries carry the wrong properties. Fix: same catalogue. Trace: apigateway-catalog-get-sdk-types#task#1.

Cause 4 of 10 - GetSdkTypes java/ruby entries in the other fixture (same catalogue defect, separate replay key). Trace: apigateway-import-rest-api-deploy-export-sdk#task#4 (3 steps).

Cause 5 of 10 - GetSdkType java in that fixture (same catalogue defect, separate replay key). Trace: apigateway-import-rest-api-deploy-export-sdk#task#5 (2 steps).

Cause 6 of 10 - GetSdkType with an unknown id. AWS: 404 NotFoundException whose message is `No SDK generator for '<id>'` - fixed text, the id taken from the request path. Emulator: `Invalid SDK type identifier specified: <id>`. Fix: GetSdkType's not-found message. Trace: apigateway-error-paths#task#5 (7 steps).

Cause 7 of 10 - GetExport (swagger): AWS writes a `consumes` list on each method it generated from the imported definition and the document-level extension `x-amazon-apigateway-security-policy: TLS_1_0`. Both are absent from the emulator's export. Why: an exported definition re-imported into AWS loses the method's content negotiation and the API's security policy. Fix: the OpenAPI/Swagger exporter. Trace: apigateway-deploy-and-export-api-definition#task#5 (2 steps).

Cause 8 of 10 - GetExport YAML serialisation. AWS emits a YAML document with no `---` start marker and with scalars double-quoted (titles, descriptions, enum-like values) plus the security-policy extension. The emulator emits a leading `---`, unquoted scalars and no extension. Why: byte-comparing or re-importing the export behaves differently. Fix: the exporter's YAML writer. Trace: apigateway-deploy-and-export-api-definition#task#6.

Cause 9 of 10 - the same missing `consumes` and `x-amazon-apigateway-security-policy` on another API's export. Trace: apigateway-documentation-parts-versions-and-export#task#17.

Cause 10 of 10 - GetExport of a CLI-configured API additionally omits a method's `produces` list alongside `consumes` and the security policy. Trace: apigateway-import-configure-deploy-export-sdk#task#12.

No open issue owns these clusters.

## Evidence
Cause 1: $.configurationProperties[name="service.name"] ... [name="java.license-text"] all missing; $.configurationProperties[name="serviceName"]/[name="javaPackageName"] extra; $.description diverges.
Cause 2: $.configurationProperties[0..2] missing; $.description expected 'Ruby SDK generator for API Gateway APIs' got 'Generates a Ruby client SDK for a deployed stage of a REST API.'; $.friendlyName expected 'Ruby' got 'Ruby SDK'.
Cause 3: $.item[id="android"], [id="javascript"], [id="objectivec"], [id="swift"] missing; java/ruby entries' properties and descriptions diverge.
Cause 6: $.message expected "No SDK generator for 'totally-bogus-sdk-type'", got 'Invalid SDK type identifier specified: totally-bogus-sdk-type' (404 both).
Causes 7/9/10: $.paths./<path>.<method>.consumes missing (expected ['application/json']), $.paths./.post.produces missing, $.x-amazon-apigateway-security-policy missing (expected 'TLS_1_0').
Cause 8: whole-body mismatch - expected unquoted-free YAML without a '---' header and with the security-policy extension; got '---'-prefixed YAML with unquoted scalars and no extension.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-5: GET /sdktypes and GET /sdktypes/java|ruby and compare ids, friendlyName, description and configurationProperties.
3. Cause 6: GET /sdktypes/totally-bogus-sdk-type and compare $.message.
4. Causes 7-10: GET /restapis/{id}/stages/{stage}/exports/swagger|oas30 (apigateway-deploy-and-export-api-definition#task#5/#task#6) and diff the document against the recording.
