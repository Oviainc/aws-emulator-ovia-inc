Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-managed-thing-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-managed-thing-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetManagedThing|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|GetManagedThing|2xx->2xx|$.SerialNumber
Issue-Key: iot-managed-integrations|GetManagedThing|2xx->2xx|$.UniversalProductCode
Issue-Key: iot-managed-integrations|GetManagedThing|2xx->2xx|$.WiFiSimpleSetupConfiguration
Issue-Key: iot-managed-integrations|ListManagedThings|2xx->2xx|$.Items[*]
Issue-Key: iot-managed-integrations|UpdateManagedThing|2xx->2xx|$headers.content-length
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# ManagedThing responses drop stored state and required headers

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetManagedThing

## Description
Across a single managed-thing lifecycle the emulator accepts CreateManagedThing and returns the right status codes, but its read/update responses do not reflect what was stored: optional-but-set attributes vanish from GetManagedThing, ListManagedThings reports no items at all, and the 204 from UpdateManagedThing omits the Content-Length header. All three are the managed-thing state/serializer layer failing to surface what the create call persisted.

Cause 1 of 3: GetManagedThing omits attributes supplied at create time.
  AWS: Smithy: GetManagedThingResponse models SerialNumber (@length 1..128, @pattern ^[A-Za-z0-9-_ ]+$, @sensitive), UniversalProductCode (@length 12..12, @pattern ^[0-9]+$) and WiFiSimpleSetupConfiguration; CreateManagedThingRequest accepts SerialNumber and WiFiSimpleSetupConfiguration as inputs (GET /managed-things/{Identifier}, code 200, readonly). A GET on a managed thing returns the attributes the resource actually holds, including those set by the create call.
  Emulator: returns 200, but SerialNumber, UniversalProductCode and WiFiSimpleSetupConfiguration are all absent from the body (replay detail: each "missing ... got None"), even though the create step supplied them. Only the identity fields come back.
  Why: a client that creates a managed thing and then reads it back cannot see its own serial number or Wi-Fi simple-setup configuration, so round-trip assertions, device-matching by serial number, and provisioning flows that branch on WiFiSimpleSetupConfiguration all behave as if the device were never configured.
  Fix: GetManagedThing's handler/state — persist the create-time attributes on the managed-thing record and project them into the response serializer.
  Traces: iot-managed-integrations-managed-thing-lifecycle#task#2 (same cluster also covers task#5 of that fixture).

Cause 2 of 3: ListManagedThings returns an empty Items list while a managed thing exists.
  AWS: Smithy: ListManagedThings (GET /managed-things, code 200, paginated items=Items) returns Items -> list of ManagedThingSummary, whose members include Id, Arn, Name, Owner, Brand, Model, SerialNumber, Role, ProvisioningStatus, CreatedAt, UpdatedAt. A managed thing created and not yet deleted appears as one summary entry with those fields populated.
  Emulator: returns 200 with no element at $.Items[0] — the created managed thing is not listed at all (replay detail: "$.Items[0]: missing ... got None").
  Why: list is how clients discover and reconcile resources; an always-empty list makes every created device invisible, so paginate-and-reconcile code concludes nothing exists and may re-create or skip cleanup.
  Fix: ListManagedThings' handler/state — enumerate the created managed-thing records and serialize each as a ManagedThingSummary (identity, Role/ProvisioningStatus, and the create-time attributes from Cause 1).
  Traces: iot-managed-integrations-managed-thing-lifecycle#task#3.

Cause 3 of 3: the 204 from UpdateManagedThing carries no Content-Length header.
  AWS: Smithy: UpdateManagedThing is PUT /managed-things/{Identifier} with http code 204 and output shape Unit (no body). The recorded response for the empty-body success sends `content-length: 0` (recorded behavior for the header value).
  Emulator: returns 204 but emits no content-length header at all (replay detail: "$headers.content-length: missing (expected '0', got None)").
  Why: HTTP clients and proxies that key framing off Content-Length on a bodyless response can hold the connection open waiting for a body or treat the framing as indeterminate; the emulator's response is not byte-shaped like the service's for an otherwise identical success.
  Fix: UpdateManagedThing's response serializer — set Content-Length: 0 on the empty 204 (applies to the service's other Unit-output/204 operations too).
  Traces: iot-managed-integrations-managed-thing-lifecycle#task#4.

## Evidence
All from fixture iot-managed-integrations-managed-thing-lifecycle, replayed in order after a CreateManagedThing that supplied SerialNumber, UniversalProductCode-equivalent product data and WiFiSimpleSetupConfiguration.
Cause 1 — step task#2 (GetManagedThing), expected 200 / actual 200: $.SerialNumber missing (recorded example value '5645029729099'), $.UniversalProductCode missing (recorded example '805730036828'), $.WiFiSimpleSetupConfiguration missing (recorded example {EnableAsProvisionee: false, EnableAsProvisioner: false}). Same cluster also fails at step task#5. (The cluster additionally reports an $.Id 'mismatch' whose expected and actual strings are identical — noise, not part of the defect.)
Cause 2 — step task#3 (ListManagedThings), expected 200 / actual 200: $.Items[0] missing; the recorded response holds one ManagedThingSummary for the thing created in setup (Id/Arn/Name/Owner/Brand/Model/SerialNumber/Role=CONTROLLER/ProvisioningStatus=UNASSOCIATED/CreatedAt/UpdatedAt). Generated id, ARN account/region and timestamps are environment-specific; the requirement is one populated summary entry, not those literals.
Cause 3 — step task#4 (UpdateManagedThing), expected 204 / actual 204: $headers.content-length missing (expected '0', got None).
Model citations: GetManagedThingResponse members SerialNumber/UniversalProductCode/WiFiSimpleSetupConfiguration; ListManagedThings output Items -> ManagedThingListDefinition of ManagedThingSummary, paginated items=Items; UpdateManagedThing http {PUT /managed-things/{Identifier}, code 204}, output Unit.

## Steps to Reproduce
1. Replay fixture iot-managed-integrations-managed-thing-lifecycle against the emulator.
2. setup#1 creates a managed thing with SerialNumber and WiFiSimpleSetupConfiguration; note the returned Id.
3. Step task#2 — GET /managed-things/{Id}: the 200 body omits SerialNumber, UniversalProductCode and WiFiSimpleSetupConfiguration (repeats at task#5).
4. Step task#3 — GET /managed-things: the 200 body's Items list has no entry for the thing created in step 2.
5. Step task#4 — PUT /managed-things/{Id}: the 204 response carries no Content-Length header; it should be Content-Length: 0.
