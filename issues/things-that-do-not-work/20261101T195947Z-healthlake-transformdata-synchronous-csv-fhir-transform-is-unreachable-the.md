Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.transform-data-sync-csv-to-fhir-bundle
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.transform-data-sync-csv-to-fhir-bundle|_datatransformation_request#8|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Commit: dc4158ff05b54d9987477de08887f7f0381de207

# TransformData (synchronous CSV→FHIR transform) is unreachable: the data-transformation endpoint answers nothing at the endpoint under test

> _Note: this issue was identified against the code base at commit `dc4158ff05b54d9987477de08887f7f0381de207`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransformData

## Description
One cause.

Cause 1 of 1: the synchronous transform surface of HealthLake's data-transformation endpoint — `TransformData`, POST /transform-data — is not served, so a client that addresses that endpoint gets no transform at all. Failing check: healthlake.transform-data-sync-csv-to-fhir-bundle.

AWS: Smithy model `com-amazonaws-healthlake-healthlake.json`, operation `TransformData` (namespace `com.amazon.awshealthlakedatatransformationfrontendservice`, bound to the data-transformation frontend reached as the `datatransformation.`-prefixed host of the healthlake endpoint; service HealthLake, awsJson1_0, endpointPrefix `healthlake`, sigv4 name `healthlake`). Documented as "Transforms clinical data synchronously to a Fast Healthcare Interoperability Resources R4 (FHIR R4) Bundle using a specified data transformation profile." Input: `ProfileId` (required, `ProfileIdString`, @length 32/32, @pattern ^[a-f0-9]{32}$), `InputData` (required, sensitive union `TransformInputData` with `CcdaInput` (String) or `CsvInput` (`FileInputMap`, filename → CSV content)), `DriftDetectionEnabled` (Boolean), `ProvenanceEnabled` (Boolean, default true). Output: `TransformedData` (required String) plus optional `DriftReport`, @http(method POST, uri /transform-data, code 200). Modelled errors: AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. So AWS answers HTTP 200 with `TransformedData` carrying a FHIR `Bundle` whose entries are produced by applying the named profile's stored mapping to the submitted input — for a CSV profile, the YAML column mapping (`sourceColumn` → `fhirPath`) applied once per CSV data row, so two rows yield two `Patient` resources with their own family/given names — and refuses an unknown `ProfileId` with ResourceNotFoundException (404).

Emulator: the request to the data-transformation endpoint never reaches a handler. The replay fails with `simparity.egress.EgressRefused: egress refused: 'datatransformation.127.0.0.1' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on` — i.e. the data-transformation endpoint the operation is bound to is not answerable at the one address the emulator is reached on, and there is no handler behind it either: the crate has no `src/ops/transform_data.rs` and no `override_operation` arm for the `/transform-data` path (contrast `src/service/validate_source.rs`, whose module doc states that the `datatransformation.` host prefix "addresses the ENDPOINT and leaves the path alone", and `src/service/non_rpc.rs::addresses_validate_source`, which wires the sibling out-of-closure REST path through that same seam). Nothing in the emulator serves POST /transform-data, so no `TransformedData`/`Bundle` can be returned and the modelled ResourceNotFoundException for an unknown ProfileId is unreachable too.

Why: a client doing the headline data-plane workflow of this service — create a CSV transformation profile, then synchronously convert CSV rows to a FHIR R4 Bundle — gets nothing it can use. Against AWS it gets 200 and a Bundle it can parse; against the emulator the call cannot even be delivered, so every FHIR-conversion path built on `TransformData` is untestable, and a caller cannot distinguish "the transform endpoint is down" from "my profile is wrong".

Fix: the data-transformation frontend surface of the healthlake crate — add the `TransformData` handler alongside the existing non-RPC surfaces (`src/service/validate_source.rs`, `src/service/non_rpc.rs`, `src/service/fhir_rest.rs`) and route POST /transform-data through `EmulatedService::override_operation` the same way `ValidateSource` is routed (path-based, host-insensitive), and make the data-transformation endpoint reachable at the single address the emulator is served on so a client addressing it is not sent to an address the emulator does not answer on. The handler must read the stored profile (`crate::state::DataTransformationProfile`) named by `ProfileId`, apply its mapping to `InputData` (CSV: the profile's YAML `tables`/`resourceMappings`/`fieldMappings` applied per data row; CCDA: the profile's template mapping), and answer 200 with `TransformedData` holding the FHIR Bundle (plus `DriftReport` when `DriftDetectionEnabled`), refusing an unknown ProfileId with the modelled ResourceNotFoundException at 404.

Related already-open issues, named here because neither covers this check: `20261024T000307Z-healthlake-transformdata-missing.md` describes the missing `TransformData` handler from the static surface but carries no replay key for this check, and `20261029T182942Z-healthlake-data-transformation-profile-surface-7-divergences-fixed.md` owns the not-found wording (`Profile '<ProfileId>' not found` vs the emulator's fixed `Could not find specified profile.`) that this check also asserts after it deletes its profile. This issue is the data-plane reachability/transform cause only.

## Evidence
Failing check healthlake.transform-data-sync-csv-to-fhir-bundle (verdict: error).

Expected (AWS, per the model): POST /transform-data on the data-transformation endpoint with body {"ProfileId":"<32 hex chars>","InputData":{"CsvInput":{"patients":"PAT_ID,LAST_NM,FIRST_NM\nP001,Smith,John\nP002,Jones,Amy"}}} → HTTP 200, body carries required member `TransformedData`, whose JSON is a FHIR Bundle (`"resourceType":"Bundle"`) containing one Patient entry per CSV data row, each with `name[0].family` / `name[0].given[0]` taken from the profile's YAML `sourceColumn` mapping (here the pairs (Smith, John) and (Jones, Amy) — those literal values are this run's CSV input filling the template, the requirement is one mapped Patient per row with the mapped columns).

Actual (emulator): the call is never delivered. Replay output:
  File "egress.py", line 281, in _hook
    refuse(host, endpoint_host)
  simparity.egress.EgressRefused: egress refused: 'datatransformation.127.0.0.1' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on
(the host is the `datatransformation.`-prefixed form of the healthlake endpoint host, which is how the data-transformation frontend is addressed; the hostname/port/profile id in that line are per-run values, not requirements.)

Code: no `services/service-com-amazonaws-healthlake-healthlake/src/ops/transform_data.rs` and no `/transform-data` arm in `EmulatedService::override_operation`; the sibling out-of-closure REST operation IS wired that way in `services/service-com-amazonaws-healthlake-healthlake/src/service/validate_source.rs` (path constant at line 63) via `services/service-com-amazonaws-healthlake-healthlake/src/service/non_rpc.rs` lines 28-39, so the seam exists and was not used for TransformData.

Smithy citation: TransformData input ProfileId (@length 32/32, @pattern ^[a-f0-9]{32}$, required), InputData (required, union CcdaInput|CsvInput), DriftDetectionEnabled, ProvenanceEnabled(default true); output TransformedData (required) + DriftReport; errors AccessDenied 403 / InternalServer 500 / ResourceNotFound 404 / Throttling 429 / Validation 400; http POST /transform-data 200.

## Steps to Reproduce
1. CreateDataTransformationProfile with SourceFormat="CSV" and Source.ProfileMapping.ProfileMapping = {"mapping-config.yaml": <YAML mapping patients.PAT_ID/LAST_NM/FIRST_NM → Patient.name[0].family / name[0].given[0]>}; note the returned ProfileId.
2. Take the healthlake client's endpoint URL and address the data-transformation endpoint for the same emulator (host = "datatransformation." + endpoint host, same scheme/port), sign with SigV4 (service name "healthlake").
3. POST /transform-data with body {"ProfileId": "<ProfileId>", "InputData": {"CsvInput": {"patients": "PAT_ID,LAST_NM,FIRST_NM\nP001,Smith,John\nP002,Jones,Amy"}}}.
4. Observe the request is not answered by the emulator at all (the harness refuses the egress because that endpoint is not the address the emulator is reached on, and no handler exists for POST /transform-data). Expected: HTTP 200 with TransformedData holding a FHIR Bundle with two mapped Patient resources.
