Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.transform-data-sync-csv-to-fhir-bundle
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.transform-data-sync-csv-to-fhir-bundle|_datatransformation_request#8|URLError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Commit: a03f808379986133e0f00cdb9a2ead2b78e9a26e

# TransformData is unserved: the data-transformation endpoint aborts the TLS handshake with an illegal_parameter alert and POST /transform-data has no handler

> _Note: this issue was identified against the code base at commit `a03f808379986133e0f00cdb9a2ead2b78e9a26e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransformData

## Description
One cause.

Cause 1 of 1: HealthLake's synchronous transform surface — `TransformData`, POST /transform-data on the data-transformation endpoint (the `datatransformation.`-prefixed host of the healthlake endpoint) — cannot be reached at all: the emulator's listener aborts the TLS handshake for that host with a server-sent `illegal parameter` alert, and even behind it there is no handler for the path. Failing check: healthlake.transform-data-sync-csv-to-fhir-bundle.

AWS: Smithy model `services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json`, operation `TransformData` (service HealthLake, awsJson1_0, sigv4 name `healthlake`), documented as transforming clinical data synchronously to a FHIR R4 Bundle using a specified data transformation profile. Input: `ProfileId` (required, `ProfileIdString`, @length min 32 max 32, @pattern ^[a-f0-9]{32}$), `InputData` (required, sensitive union `TransformInputData` with `CcdaInput` (String) or `CsvInput` (`FileInputMap`: filename → file content)), `DriftDetectionEnabled` (Boolean), `ProvenanceEnabled` (Boolean, @default true). Output: `TransformedData` (required String) and optional `DriftReport`; @http(method POST, uri /transform-data, code 200). Modelled errors: AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. So AWS terminates TLS on the data-transformation endpoint, answers HTTP 200, and returns `TransformedData` holding a FHIR `Bundle` whose entries are the named profile's stored mapping applied to the submitted input — for a CSV profile, the YAML `tables`/`resourceMappings`/`fieldMappings` (`sourceColumn` → `fhirPath`) applied once per CSV data row, so N data rows yield N `Patient` resources each carrying its own row's values — and refuses an unknown `ProfileId` with ResourceNotFoundException (404).

Emulator: the request never gets as far as HTTP. Opening the connection to the data-transformation endpoint fails in the client's TLS handshake with `urllib.error.URLError: <urlopen error [SSL: SSLV3_ALERT_ILLEGAL_PARAMETER] ssl/tls alert illegal parameter>` — the alert is sent by the peer, i.e. the emulator's own listener refuses the handshake for the `datatransformation.`-prefixed host (the same address and port it answers the control-plane healthlake client on), so the endpoint the operation is bound to is not answerable. Behind it there is no handler either: the healthlake crate still has no `src/ops/transform_data.rs` and no `/transform-data` arm in `EmulatedService::override_operation` — the only non-RPC surfaces wired through that seam are the data store FHIR REST plane, `/validate-source` and the REST tagging paths (see the module doc and `addresses_validate_source` in `src/service/non_rpc.rs`). Consequently neither the 200 + `TransformedData`/`Bundle` response nor the modelled ResourceNotFoundException for an unknown ProfileId is reachable.

Why: a client doing this service's headline data-plane workflow — create a CSV transformation profile, then synchronously convert CSV rows into a FHIR R4 Bundle — gets no response it can interpret. Against AWS it gets 200 and a parsable Bundle; against the emulator the connection to the transform endpoint is dropped during the handshake, which a client cannot distinguish from a network/certificate problem of its own, so every FHIR-conversion path built on `TransformData` is untestable and the SDK surfaces a transport error rather than any modelled error.

Fix: two components. (1) The crate's endpoint/listener seam: the data-transformation endpoint must be served at the single address the emulator is reached on, so a client addressing the `datatransformation.`-prefixed host completes TLS instead of receiving a handshake alert (any SNI/host name resolving to the emulator's address must be accepted, not just the bare endpoint host). (2) The data-transformation frontend surface of the healthlake crate: add the `TransformData` handler and route POST /transform-data through `EmulatedService::override_operation` path-based and host-insensitive, exactly as `ValidateSource` and the REST tagging paths are routed in `src/service/non_rpc.rs` / `src/service/validate_source.rs`; the handler must read the stored profile named by `ProfileId` from the crate state, apply its mapping to `InputData` (CSV: the profile's YAML column mapping per data row; CCDA: its template mapping), answer 200 with `TransformedData` carrying the FHIR Bundle (plus `DriftReport` when `DriftDetectionEnabled`), and refuse an unknown/deleted `ProfileId` with ResourceNotFoundException at 404.

Related open issues, named here because neither covers this failure: `20261101T195947Z-healthlake-transformdata-synchronous-csv-fhir-transform-is-unreachable-the.md` describes the same operation being unreachable but is keyed on an `EgressRefused` signature (the harness refused the egress before any connection was made); the failure now observed is a different, emulator-emitted one — the emulator accepts the connection attempt and aborts the TLS handshake — and the check is not reported as owned by that issue. `20261024T000307Z-healthlake-transformdata-missing.md` records the missing handler from the static surface and carries no replay key for this check. The not-found wording this check also asserts after it deletes its profile (`Profile '<ProfileId>' not found` vs the emulator's fixed `Could not find specified profile.`) is owned by `20261029T182942Z-healthlake-data-transformation-profile-surface-7-divergences-fixed.md` and is NOT part of this issue; this issue is the reachability + transform cause only.

## Evidence
Failing check healthlake.transform-data-sync-csv-to-fhir-bundle (verdict: error).

Expected (AWS, per the model): POST /transform-data on the data-transformation endpoint, SigV4-signed with service name `healthlake`, body {"ProfileId":"<32 lowercase hex chars>","InputData":{"CsvInput":{"patients":"PAT_ID,LAST_NM,FIRST_NM\nP001,Smith,John\nP002,Jones,Amy"}}} → HTTP 200 whose body carries the required member `TransformedData`, itself a JSON FHIR Bundle (`"resourceType":"Bundle"`) with one Patient entry per CSV data row, each entry's `name[0].family` / `name[0].given[0]` taken from the profile's YAML `sourceColumn` → `fhirPath` mapping. (The two pairs (Smith, John) and (Jones, Amy) are this run's CSV input filling that template; the requirement is one mapped Patient per data row with the mapped columns, not these literal names. Host, port and the generated 32-hex ProfileId are per-run values.)

Actual (emulator): the call never reaches HTTP. Replay output:
  File "request.py", line 1405, in https_open
    return self.do_open(http.client.HTTPSConnection, req,
  File "request.py", line 1360, in do_open
    raise URLError(err)
  urllib.error.URLError: <urlopen error [SSL: SSLV3_ALERT_ILLEGAL_PARAMETER] ssl/tls alert illegal parameter (_ssl.c:1010)>
The alert is received by the client, i.e. sent by the emulator's listener, for a connection to the `datatransformation.`-prefixed form of the same healthlake endpoint host/port the control-plane calls in the same check succeed against (CreateDataTransformationProfile in step 1 of the check returns a ProfileId normally).

Code: no `services/service-com-amazonaws-healthlake-healthlake/src/ops/transform_data.rs` exists, and `services/service-com-amazonaws-healthlake-healthlake/src/service/non_rpc.rs` (module doc lines 7-14, `addresses_validate_source` lines 35-39) shows the non-RPC routing seam carries only the data store FHIR REST plane, `/validate-source` and the REST tagging surface — there is no `/transform-data` arm.

Smithy citation: TransformData input `ProfileId` (required, @length 32/32, @pattern ^[a-f0-9]{32}$), `InputData` (required, union CcdaInput|CsvInput, sensitive), `DriftDetectionEnabled`, `ProvenanceEnabled` (@default true); output `TransformedData` (required) + `DriftReport`; errors AccessDeniedException 403 / InternalServerException 500 / ResourceNotFoundException 404 / ThrottlingException 429 / ValidationException 400; @http POST /transform-data code 200.

## Steps to Reproduce
1. CreateDataTransformationProfile with SourceFormat="CSV" and Source.ProfileMapping.ProfileMapping = {"mapping-config.yaml": <YAML mapping table `patients` with primaryKeyColumn PAT_ID and fieldMappings LAST_NM → name[0].family, FIRST_NM → name[0].given[0] on targetResourceType Patient>}; note the returned ProfileId.
2. Build the data-transformation endpoint URL for the same emulator: host = "datatransformation." + the healthlake client's endpoint host, same scheme and port. Sign the request with SigV4 using service name "healthlake".
3. POST /transform-data with body {"ProfileId": "<ProfileId>", "InputData": {"CsvInput": {"patients": "PAT_ID,LAST_NM,FIRST_NM\nP001,Smith,John\nP002,Jones,Amy"}}}.
4. Observe the TLS handshake is aborted by the emulator with an `illegal parameter` alert, so no HTTP response is produced. Expected: HTTP 200 with `TransformedData` holding a FHIR Bundle containing two Patient resources whose family/given names are the mapped CSV columns (Smith/John and Jones/Amy).
5. For the refusal path: repeat step 3 with a 32-hex ProfileId that does not exist (or after DeleteDataTransformationProfile). Expected: ResourceNotFoundException at HTTP 404; observed: the same handshake abort, so the modelled error is unreachable.
