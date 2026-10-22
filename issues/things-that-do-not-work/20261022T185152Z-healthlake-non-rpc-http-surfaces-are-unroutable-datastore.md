Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.fhir-rest-api-native-protocol-roundtrip healthlake.validate-source-reports-structural-issues
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.fhir-rest-api-native-protocol-roundtrip|run#12|AssertionError|assert status == 201, f"unexpected create status {status}: {created_patient}"
Issue-Key: healthlake.validate-source-reports-structural-issues|run#4|AssertionError|assert status == 200, f"unexpected ValidateSource status {status}: {valid_result}"
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# HealthLake non-RPC HTTP surfaces are unroutable: datastore FHIR REST endpoint and /validate-source answer 400 SerializationException "missing required header `x-amz-target`" (2 causes)

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFHIRDatastore

## Description
The HealthLake emulator serves only its awsJson1_0 RPC surface: every request is dispatched by the `x-amz-target` header, so any sigv4-signed HTTP request to a HealthLake path that is NOT an awsJson1_0 RPC call is rejected at the protocol layer with HTTP 400 and `{"__type":"SerializationException","message":"missing required header `x-amz-target`"}`. Two distinct HealthLake surfaces that AWS exposes this way are therefore completely unreachable. Both causes must be closed before submitting.

Cause 1 of 2: the data store's FHIR REST data plane (DatastoreEndpoint) is not served.
AWS: Smithy (com-amazonaws-healthlake-healthlake.json) models the control plane as awsJson1_0, and DatastoreProperties.DatastoreEndpoint is a required member documented as "The Amazon Web Services endpoint for the data store". The FHIR interactions on that endpoint are not modeled operations — recorded behavior only: a sigv4-signed (service name `healthlake`) `POST <DatastoreEndpoint>/Patient` with Content-Type application/fhir+json returns HTTP 201 with the created FHIR resource JSON (resourceType "Patient", a server-assigned `id`, and the submitted `name`/`identifier` content echoed back), and a subsequent `GET <DatastoreEndpoint>/Patient/{id}` returns HTTP 200 with the same resource. No `x-amz-target` header is involved on this wire at all.
Emulator: the create POST returns HTTP 400 `SerializationException` / message "missing required header `x-amz-target`" (example text), so no FHIR resource is ever created or readable. The control-plane part of the check passes: the datastore is created, reaches ACTIVE and reports a DatastoreEndpoint — the endpoint it advertises just does not accept FHIR requests.
Why: a client that does what HealthLake is for — writing and reading FHIR resources through the data store endpoint returned by DescribeFHIRDatastore — gets a protocol-level serialization error on every interaction instead of 201/200 with resource bodies. There is no workaround: the entire data plane is inaccessible.
Fix: the service's HTTP request router/protocol dispatch must route requests whose path is a datastore FHIR endpoint path to a FHIR REST data-plane handler instead of requiring `x-amz-target`, and that handler plus the datastore's resource state must implement at minimum the FHIR create (201, server-assigned id, echoed resource) and read-by-id (200) interactions. New handler alongside services/service-com-amazonaws-healthlake-healthlake/src/ops/ with resource storage in the service's state.
Traces: healthlake.fhir-rest-api-native-protocol-roundtrip.

Cause 2 of 2: the ValidateSource endpoint (`POST /validate-source?sourceFormat=...`) is not served.
AWS: ValidateSource is not in the Smithy model (digest of com-amazonaws-healthlake-healthlake.json lists 26 operations, none named ValidateSource) — recorded behavior only: a sigv4-signed `POST /validate-source?sourceFormat=CCDA` with an octet-stream body returns HTTP 200 with a validation report (`Valid`, `SourceFormat`, `Summary.ErrorCount`, `Issues[]` with per-issue `Code` such as CRITICAL_ERROR) for ALL content-level outcomes — a structurally complete C-CDA yields Valid=true/ErrorCount=0, an incomplete or wholly unparseable body yields 200 with Valid=false and at least one Issue — while a request-level problem, an unsupported `sourceFormat=CSV`, is refused with HTTP 400.
Emulator: every ValidateSource request returns HTTP 400 `SerializationException` / "missing required header `x-amz-target`" (example text), including the positive-control valid C-CDA that must be 200.
Why: a client validating C-CDA source documents before a data-transformation job cannot distinguish "document has structural issues" (200 + report) from "request rejected", and sees a protocol error for well-formed requests; the soft-vs-hard failure contract (content problems = 200 report, bad sourceFormat = 400) is entirely absent.
Fix: route the `/validate-source` path (including the `datatransformation.` host prefix form the client uses for data-transformation operations) in the service's HTTP router rather than demanding `x-amz-target`, and add a handler that returns the validation-report body and 400s an unsupported sourceFormat. New handler under services/service-com-amazonaws-healthlake-healthlake/src/ops/.
Traces: healthlake.validate-source-reports-structural-issues.

## Evidence
healthlake.fhir-rest-api-native-protocol-roundtrip — expected HTTP 201 from `POST <DatastoreEndpoint>/Patient`; got status 400 body {'__type': 'SerializationException', 'message': 'missing required header `x-amz-target`'} (AssertionError: unexpected create status 400). The read step (GET /Patient/{id}, expected 200) is never reached.

healthlake.validate-source-reports-structural-issues — expected HTTP 200 with Valid=true/Summary.ErrorCount=0 for a structurally complete C-CDA CCD from `POST /validate-source?sourceFormat=CCDA`; got status 400 body {'__type': 'SerializationException', 'message': 'missing required header `x-amz-target`'} (AssertionError: unexpected ValidateSource status 400). The Valid=false report cases and the CSV 400 rejection are never reached.

Both requests are sigv4-signed for service `healthlake` with ordinary REST headers (Content-Type application/fhir+json / application/octet-stream) and intentionally carry no x-amz-target, because neither surface is an awsJson1_0 RPC call.

## Steps to Reproduce
1. CreateFHIRDatastore(DatastoreTypeVersion="R4", DatastoreName=<name>) and poll DescribeFHIRDatastore until DatastoreStatus == ACTIVE; take DatastoreProperties.DatastoreEndpoint.
2. SigV4-sign (service `healthlake`) and send `POST {DatastoreEndpoint}/Patient` with Content-Type: application/fhir+json and a minimal FHIR Patient JSON body. Observe 400 SerializationException instead of 201 with the created resource; a following `GET {DatastoreEndpoint}/Patient/{id}` likewise cannot succeed.
3. SigV4-sign and send `POST /validate-source?sourceFormat=CCDA` (datatransformation. host prefix where the client injects host prefixes) with Content-Type: application/octet-stream and a complete C-CDA CCD body. Observe 400 SerializationException instead of 200 with {Valid: true, Summary:{ErrorCount:0}, ...}; repeat with an incomplete C-CDA / garbage bytes (expected 200, Valid=false, Issues non-empty) and with sourceFormat=CSV (expected 400).
