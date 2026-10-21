Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the CloudControl (Cloud Control API) emulator

_Filed: 2026-10-21T21:57:58Z_
_Operation: service-com-amazonaws-cloudcontrol-cloudapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding zero state. There is no emulator to have gaps against yet, so this issue defines the FIRST buildable slice.

Wire protocol (per README/OPERATIONS.md): `awsJson1_0`, `POST /` with `X-Amz-Target: CloudApiService.<OperationName>`, sigv4 signing name `cloudcontrolapi`.

## Happy path to implement (in order)

1. **CreateResource** — input `TypeName` (required, e.g. `AWS::S3::Bucket`), `DesiredState` (required, JSON blob of properties), optional `TypeVersionId`, `RoleArn`, `ClientToken`. Must: parse `DesiredState` as JSON, derive a primary `Identifier` from it (e.g. the `Name`/`BucketName`-style property, else a generated id), store the resource under (TypeName, Identifier) with its properties, mint a fresh `RequestToken`, record a request record, and return `ProgressEvent{TypeName, Identifier, RequestToken, Operation=CREATE, OperationStatus=SUCCESS (or IN_PROGRESS then settling), EventTime}`. Creating a second resource with the same (TypeName, Identifier) must fail `AlreadyExistsException` (HTTP 400). Reusing a `ClientToken` with a *different* payload must fail `ClientTokenConflictException` (409); replaying the same `ClientToken` with the same payload must return the SAME `RequestToken`/`ProgressEvent` (idempotency, `@idempotencyToken`).
2. **GetResourceRequestStatus** — input `RequestToken` (required). Must look the stored request record up by token and return its `ProgressEvent` (same `Operation`, `OperationStatus`, `Identifier`, `TypeName`, `EventTime`, plus `StatusMessage`/`ErrorCode` for FAILED). An unknown token must fail `RequestTokenNotFoundException` (HTTP 404) — never a fabricated SUCCESS.
3. **GetResource** — input `TypeName` + `Identifier` (both required). Must return `TypeName` and `ResourceDescription{Identifier, Properties}` where `Properties` is the JSON currently persisted for that resource (i.e. reflecting the create and any later update). Unknown `Identifier` must fail `ResourceNotFoundException` (HTTP 404).
4. **UpdateResource** — input `TypeName`, `Identifier`, `PatchDocument` (all required). Must apply the JSON Patch (RFC 6902) in `PatchDocument` to the persisted properties, store the result, mint a new `RequestToken` and request record, and return `ProgressEvent{Operation=UPDATE, OperationStatus=SUCCESS, Identifier, RequestToken, ...}`. A subsequent GetResource must show the patched values. Unknown `Identifier` → `ResourceNotFoundException` (404).
5. **ListResources** — input `TypeName` (required), optional `MaxResults` (1..100), `NextToken` (1..4096). Must return `TypeName` and `ResourceDescriptions` for the resources actually persisted under that `TypeName`, honouring `MaxResults` as a page size and returning/accepting an opaque `NextToken` that resumes where the previous page stopped (`@paginated` inputToken=NextToken, outputToken=NextToken, items=ResourceDescriptions). Omit `NextToken` on the last page.
6. **DeleteResource** — input `TypeName` + `Identifier` (required). Must remove the resource from state, record a request record, and return `ProgressEvent{Operation=DELETE, OperationStatus=SUCCESS, Identifier, RequestToken}`. After it, GetResource for that identifier must fail `ResourceNotFoundException` (404) and ListResources must no longer include it. Deleting an absent resource → `ResourceNotFoundException` (404).

Every one of these six must compute its response from persisted state. No canned `ProgressEvent`s, no echoing `DesiredState` straight back out of GetResource without storing it, and no minting a `RequestToken` that GetResourceRequestStatus cannot then resolve.

## State Model

Service state (reached via `crate::state::service_state(&*self.engine)`, guarded by a lock):

```text
CloudControlState {
    // resources, keyed by type then primary identifier
    resources: Map<String /*TypeName*/, Map<String /*Identifier*/, Resource>>,
    // every resource operation request, keyed by the token handed to the client
    requests: Map<String /*RequestToken*/, RequestRecord>,
    // request tokens in creation order, for ListResourceRequests paging
    request_order: Vec<String /*RequestToken*/>,
    // ClientToken -> RequestToken, for idempotent replay / ClientTokenConflict
    client_tokens: Map<String /*ClientToken*/, ClientTokenEntry>,
}

Resource {
    type_name: String,
    identifier: String,
    properties: serde_json::Value,   // current desired/actual state
    type_version_id: Option<String>,
    created_at: SystemTime,
    updated_at: SystemTime,
}

RequestRecord {
    request_token: String,
    hooks_request_token: Option<String>,
    type_name: String,
    identifier: Option<String>,
    operation: Operation,              // CREATE | UPDATE | DELETE
    operation_status: OperationStatus, // PENDING | IN_PROGRESS | SUCCESS | FAILED | CANCEL_IN_PROGRESS | CANCEL_COMPLETE
    event_time: SystemTime,
    resource_model: Option<String>,    // JSON string of properties
    status_message: Option<String>,
    error_code: Option<HandlerErrorCode>,
    retry_after: Option<SystemTime>,
}

ClientTokenEntry { request_token: String, request_fingerprint: String, used_at: SystemTime }
```

`ProgressEvent` responses are rendered from `RequestRecord`; `ResourceDescription` responses are rendered from `Resource`.

## Out of scope for this slice

`ListResourceRequests` and `CancelResourceRequest` stay stubbed (501) and will be filed as individual gap issues later — the `requests`/`request_order` structures above are shaped so they drop in cleanly.

Done = a realistic client script (AWS SDK or CLI against the emulator endpoint) runs the six operations end-to-end and the response bodies are stateful and correct, not merely 2xx.

## Evidence

OPERATIONS.md at the crate root lists all 8 operations with their input/output members and errors: CreateResource (TypeName+DesiredState required → ProgressEvent; AlreadyExistsException 400, ClientTokenConflictException 409, ResourceNotFoundException 404), GetResource (TypeName+Identifier required → TypeName + ResourceDescription), UpdateResource (TypeName+Identifier+PatchDocument required → ProgressEvent), DeleteResource (TypeName+Identifier required → ProgressEvent), ListResources (@paginated inputToken=NextToken, outputToken=NextToken, items=ResourceDescriptions; MaxResults range 1..100), GetResourceRequestStatus (RequestToken required → ProgressEvent + HooksProgressEvent; RequestTokenNotFoundException 404). Shapes from the Smithy model: ProgressEvent{TypeName, Identifier, RequestToken, HooksRequestToken, Operation, OperationStatus, EventTime, ResourceModel, StatusMessage, ErrorCode, RetryAfter}; ResourceDescription{Identifier, Properties}; Operation enum = CREATE|DELETE|UPDATE; OperationStatus enum = PENDING|IN_PROGRESS|SUCCESS|FAILED|CANCEL_IN_PROGRESS|CANCEL_COMPLETE. All-stub status confirmed: src/ops/create_resource.rs lines 4 and 246 show the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateResource"))`, and the same scaffold shape applies to all 8 op files. The CreateResource doc comment in that file states the modeled contract that the RequestToken of the returned ProgressEvent is what GetResourceRequestStatus must resolve.

## Steps to Reproduce

1. Start the emulator and point an awsJson1_0 client at it with target prefix `CloudApiService.` (signing name cloudcontrolapi).
2. CreateResource with TypeName="AWS::S3::Bucket", DesiredState={"BucketName":"demo-bucket"}, ClientToken="tok-1" — expect 200 with ProgressEvent{Operation=CREATE, OperationStatus=SUCCESS, Identifier="demo-bucket", RequestToken=<t1>}; today: HTTP 501 NotImplemented.
3. GetResourceRequestStatus with RequestToken=<t1> — expect the same ProgressEvent (CREATE/SUCCESS, Identifier=demo-bucket); today 501.
4. Repeat the CreateResource call verbatim with ClientToken="tok-1" — expect the same RequestToken <t1> returned (idempotent replay), not a second resource.
5. CreateResource again with the same DesiredState but a new ClientToken — expect AlreadyExistsException (HTTP 400).
6. GetResource with TypeName="AWS::S3::Bucket", Identifier="demo-bucket" — expect ResourceDescription{Identifier:"demo-bucket", Properties containing BucketName:"demo-bucket"}; today 501.
7. UpdateResource with the same TypeName/Identifier and PatchDocument=[{"op":"add","path":"/VersioningConfiguration","value":{"Status":"Enabled"}}] — expect ProgressEvent{Operation=UPDATE, OperationStatus=SUCCESS}, then GetResource must show the new property.
8. ListResources with TypeName="AWS::S3::Bucket", MaxResults=1 — expect exactly one ResourceDescription plus a NextToken when more exist; re-call with that NextToken and expect the next resource and no repeats.
9. DeleteResource with TypeName="AWS::S3::Bucket", Identifier="demo-bucket" — expect ProgressEvent{Operation=DELETE, OperationStatus=SUCCESS}.
10. GetResource for "demo-bucket" again — expect ResourceNotFoundException (HTTP 404), and ListResources must no longer list it.
11. GetResourceRequestStatus with RequestToken="bogus" — expect RequestTokenNotFoundException (HTTP 404).
