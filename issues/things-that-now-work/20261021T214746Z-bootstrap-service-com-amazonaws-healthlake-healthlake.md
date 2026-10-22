Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the HealthLake emulator

_Filed: 2026-10-21T21:47:46Z_
_Operation: service-com-amazonaws-healthlake-healthlake | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-healthlake-healthlake/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, with zero state held. Implement a FIRST, SMALL, stateful slice around the service's primary resource — the FHIR data store.

Wire protocol (from OPERATIONS.md): `awsJson1_0`, `POST /` with `X-Amz-Target: HealthLake.<OperationName>`, JSON body using modeled member names; sigv4 signing name `healthlake`.

## Happy path to implement (in order)

1. **CreateFHIRDatastore** — input: `DatastoreTypeVersion` (FHIRVersion, only `R4`, required), `DatastoreName`, `SseConfiguration`, `PreloadDataConfig`, `IdentityProviderConfiguration`, `Tags` (list<Tag>), `ClientToken` (@idempotencyToken). Must mint and PERSIST a new data store: generate a 32-char `DatastoreId`, build `DatastoreArn` as `arn:aws:healthlake:<region>:<account>:datastore/fhir/<DatastoreId>` (must satisfy the modeled ARN pattern), build `DatastoreEndpoint` (e.g. `https://healthlake.<region>.amazonaws.com/datastore/<DatastoreId>/r4/`), record `CreatedAt`, and return `DatastoreId`/`DatastoreArn`/`DatastoreStatus`/`DatastoreEndpoint`. Status should start `CREATING` and become `ACTIVE` on a subsequent describe (a simple deterministic transition is fine, but it must be stored, not recomputed per call from nothing). Same `ClientToken` + same request ⇒ return the SAME `DatastoreId` instead of creating a second store.
2. **DescribeFHIRDatastore** — input `DatastoreId` (required); output `DatastoreProperties` (required) populated FROM the stored record: `DatastoreId`, `DatastoreArn`, `DatastoreName`, `DatastoreStatus`, `CreatedAt`, `DatastoreTypeVersion`, `DatastoreEndpoint`, plus `SseConfiguration`/`PreloadDataConfig`/`IdentityProviderConfiguration` exactly as supplied at create. Unknown id ⇒ `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListFHIRDatastores** — input `Filter` (DatastoreFilter: `DatastoreName`, `DatastoreStatus`, `CreatedBefore`, `CreatedAfter`), `MaxResults` (1..500), `NextToken` (<=8192, ASCII). Output `DatastorePropertiesList` (required) containing every stored store, each the same `DatastoreProperties` shape Describe returns; `Filter` must actually filter and `MaxResults`/`NextToken` must actually page (emit `NextToken` only when more remain, and resume from it).
4. **TagResource** / **ListTagsForResource** — `ResourceARN` (must match the modeled healthlake datastore ARN pattern) + `Tags`. TagResource stores the tags on the referenced data store (merging by key) and returns an empty body; ListTagsForResource returns exactly the stored tags, including those passed to CreateFHIRDatastore. Unknown/unparseable ARN ⇒ `ResourceNotFoundException` (404) / `ValidationException` (400).
5. **DeleteFHIRDatastore** — input `DatastoreId` (required); mark the stored record `DELETING` (then `DELETED`) and return `DatastoreId`/`DatastoreArn`/`DatastoreStatus`/`DatastoreEndpoint` from the stored record. A following DescribeFHIRDatastore must show the deleted/deleting status (or 404 once reaped), and ListFHIRDatastores must no longer list it as `ACTIVE`. Deleting an unknown id ⇒ `ResourceNotFoundException` (404); deleting one already `DELETING` ⇒ `ConflictException` (409).

Every one of these MUST compute its response from persisted state held via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the input back, no random id that is never stored. A create followed by a describe/list must return what was created.

## State Model

```
HealthLakeState {
    datastores: Map<DatastoreId /* 32-char string */, Datastore>,
    client_tokens: Map<ClientToken, DatastoreId>,   // CreateFHIRDatastore idempotency
}

Datastore {
    id: String,                        // DatastoreId (32 chars)
    arn: String,                       // arn:aws:healthlake:<region>:<account>:datastore/fhir/<id>
    name: Option<String>,              // DatastoreName
    status: DatastoreStatus,           // CREATING -> ACTIVE -> DELETING -> DELETED
    type_version: FHIRVersion,         // R4
    endpoint: String,                  // DatastoreEndpoint
    created_at: Timestamp,             // CreatedAt
    sse_configuration: Option<SseConfiguration>,
    preload_data_config: Option<PreloadDataConfig>,
    identity_provider_configuration: Option<IdentityProviderConfiguration>,
    error_cause: Option<ErrorCause>,
    tags: Map<String, String>,         // from Create Tags + TagResource
}
```

The remaining 21 operations (import/export jobs, data transformation profiles/jobs, RestoreFHIRDatastore, UpdateFHIRDatastore, UpdateProfileWithAgent, UntagResource, ...) stay 501 for this slice and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md at the crate root (`services/service-com-amazonaws-healthlake-healthlake/OPERATIONS.md`) lists 26 operations with their input/output members and errors: CreateFHIRDatastore (lines 34-38), DeleteFHIRDatastore (48-52), DescribeFHIRDatastore (62-66), ListFHIRDatastores (112-117), ListTagsForResource (133-137), TagResource (172-176). Field shapes `DatastoreProperties` and `DatastoreFilter` were confirmed via smithy_query (DatastoreProperties requires DatastoreId, DatastoreArn, DatastoreStatus, DatastoreTypeVersion, DatastoreEndpoint; DatastoreFilter filters by name/status/CreatedBefore/CreatedAfter). The all-stub state is proven by `services/service-com-amazonaws-healthlake-healthlake/src/ops/create_fhir_datastore.rs` lines 3-6 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 187 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateFHIRDatastore"))`); all 25 sibling op files follow the identical generated template. No tests assert any HealthLake behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI healthlake client at it.
2. CreateFHIRDatastore with DatastoreTypeVersion=R4, DatastoreName="demo-store", Tags=[{Key:env,Value:test}], ClientToken="tok-1" — expect 200 with a DatastoreId, a well-formed DatastoreArn, DatastoreEndpoint and DatastoreStatus=CREATING (today: HTTP 501 NotImplemented).
3. Repeat the same CreateFHIRDatastore call with ClientToken="tok-1" — expect the SAME DatastoreId, not a second data store.
4. DescribeFHIRDatastore with that DatastoreId — expect DatastoreProperties echoing the stored name, type version, CreatedAt, endpoint and ARN, with DatastoreStatus=ACTIVE.
5. ListFHIRDatastores with no filter — expect the store in DatastorePropertiesList; repeat with Filter={DatastoreStatus:ACTIVE} and with Filter={DatastoreName:"nope"} and expect filtering to apply; call with MaxResults=1 after creating two stores and expect a NextToken that resumes.
6. ListTagsForResource with the store's ARN — expect [{env,test}]; TagResource adding {owner,alice} then ListTagsForResource again — expect both tags.
7. DeleteFHIRDatastore with that DatastoreId — expect 200 with DatastoreStatus=DELETING; DescribeFHIRDatastore again shows DELETING/DELETED (or 404 once reaped) and ListFHIRDatastores no longer reports it ACTIVE.
8. DescribeFHIRDatastore / DeleteFHIRDatastore with a bogus DatastoreId — expect ResourceNotFoundException with HTTP 404.
