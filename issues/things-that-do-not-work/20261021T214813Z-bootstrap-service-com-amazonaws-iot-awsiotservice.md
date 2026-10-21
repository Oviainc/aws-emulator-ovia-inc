Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the com-amazonaws-iot-awsiotservice (AWS IoT) emulator

_Filed: 2026-10-21T21:48:13Z_
_Operation: service-com-amazonaws-iot-awsiotservice | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 272 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. The first buildable slice is the Thing registry lifecycle — AWS IoT's primary resource — implemented statefully over the restJson1 protocol (SDK id `IoT`, signing name `iot`).

### Happy path to implement (in order)

1. **CreateThing** — `POST /things/{thingName}`. Input: `thingName` (@httpLabel, required, len 1..128, pattern `^[a-zA-Z0-9:_-]+$`), `thingTypeName`, `attributePayload` (AttributePayload: `attributes` map<string,string>, `merge` bool), `billingGroupName`. Must insert a new Thing into the registry and return `thingName`, a freshly generated `thingId` (uuid) and a real `thingArn` (`arn:aws:iot:<region>:<account>:thing/<thingName>`) derived from the stored record. Re-creating an existing name must fail with `ResourceAlreadyExistsException` (HTTP 409); referencing an unknown `thingTypeName` must fail with `ResourceNotFoundException` (404).
2. **DescribeThing** — `GET /things/{thingName}`. Must read the stored record and return `thingName`, `thingId`, `thingArn`, `thingTypeName`, `attributes` (exactly what CreateThing/UpdateThing persisted), `version` (required, starts at 1 and increments on each update), and `defaultClientId` (= thingName). Unknown name → `ResourceNotFoundException` (404), never a hollow 200.
3. **UpdateThing** — `PATCH /things/{thingName}`. Input: `thingTypeName`, `attributePayload`, `expectedVersion`, `removeThingType` (default false). Must mutate the stored record: `merge=true` merges attributes into the existing map, `merge=false` replaces it, an attribute with an empty value removes that key, `removeThingType=true` clears `thingTypeName`. Bump `version` on success. If `expectedVersion` is supplied and does not match the stored version → `VersionConflictException` (409). Unknown thing → 404. Output has no members.
4. **ListThings** — `GET /things`. Input: `maxResults` (1..250), `nextToken`, `attributeName`, `attributeValue`, `usePrefixAttributeValue` (default false), `thingTypeName`. Must enumerate the stored registry, returning `things` as `ThingAttribute{thingName, thingTypeName, thingArn, attributes, version}` built from state, honour the `thingTypeName`/`attributeName`+`attributeValue` filters (prefix-match when `usePrefixAttributeValue=true`), truncate at `maxResults` and return an opaque `nextToken` that resumes exactly where the page ended (absent on the last page).
5. **DeleteThing** — `DELETE /things/{thingName}`. Input: `expectedVersion` (@httpQuery). Must remove the record from the registry; mismatched `expectedVersion` → `VersionConflictException` (409); unknown thing → `ResourceNotFoundException` (404). After a successful delete, DescribeThing must 404 and ListThings must no longer include the thing.

Every one of these must compute its response from persisted state reached through `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no generating an id without storing it. A create followed by a describe must return what was created; a delete must be observable by the next reader.

## State Model

In `ComAmazonawsIotAwsiotserviceState`:

- `things: BTreeMap<String /*thingName*/, Thing>` — BTreeMap so ListThings has a stable, resumable order.
  - `Thing { thing_name: String, thing_id: String /*uuid*/, thing_arn: String, thing_type_name: Option<String>, attributes: BTreeMap<String,String>, billing_group_name: Option<String>, version: i64 /*starts at 1*/, created_at: SystemTime }`
- `thing_types: BTreeMap<String /*thingTypeName*/, ThingType>` — needed only so CreateThing/UpdateThing can validate `thingTypeName` and so CreateThingType can be added next.
  - `ThingType { thing_type_name: String, thing_type_id: String, thing_type_arn: String, properties: Option<ThingTypeProperties>, deprecated: bool, created_at: SystemTime }`
- `list_tokens` (or a deterministic token encoding of the last-returned `thingName`) backing ListThings pagination so `nextToken` is opaque to the client but resolvable by the server.

All other 267 operations (jobs, certificates, policies, topic rules, security profiles, packages, audits, …) stay at 501 and will be filed as individual gap issues later; keep this first slice small.

## Evidence

`services/service-com-amazonaws-iot-awsiotservice/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-iot-awsiotservice.json`) documents: CreateThing line 338-343 (`POST /things/{thingName}`, output `thingName`/`thingArn`/`thingId`, errors incl. ResourceAlreadyExistsException 409), DescribeThing line 851-856 (`GET /things/{thingName}`, output incl. `attributes`, `version` required default 0, `defaultClientId`, error ResourceNotFoundException 404), UpdateThing line 1963-1968 (`PATCH /things/{thingName}`, `expectedVersion`, `removeThingType`, error VersionConflictException 409), ListThings line 1539-1545 (`GET /things`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=things)`, `maxResults` range 1..250, attribute filters), DeleteThing line 585-590 (`DELETE /things/{thingName}`, `expectedVersion` @httpQuery, VersionConflictException 409). Protocol block lines 8-14: restJson1, sdkId `IoT`, signing name `iot`. Stub proof: `services/service-com-amazonaws-iot-awsiotservice/src/ops/create_thing.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 178 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateThing"))`; the same pattern holds for every file in `src/ops/`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it with `--endpoint-url`, service `iot`.
2. `aws iot create-thing --thing-name demo-thing --attribute-payload 'attributes={color=red}'` → expect 200 with `thingName=demo-thing`, a non-empty `thingId` and `thingArn` ending in `thing/demo-thing` (today: HTTP 501 NotImplemented).
3. `aws iot describe-thing --thing-name demo-thing` → expect the same `thingId`/`thingArn`, `attributes={color:red}`, `version=1`, `defaultClientId=demo-thing`.
4. `aws iot update-thing --thing-name demo-thing --attribute-payload 'attributes={size=large},merge=true'` then `describe-thing` → expect `attributes={color:red,size:large}` and `version=2`.
5. `aws iot update-thing --thing-name demo-thing --expected-version 1 --attribute-payload 'attributes={x=y}'` → expect HTTP 409 VersionConflictException.
6. `aws iot list-things --max-results 1` with two things created → expect exactly one `things` entry carrying stored `thingName`/`attributes`/`version` plus a `nextToken`; passing that token returns the second thing and no token.
7. `aws iot delete-thing --thing-name demo-thing` → 200; then `describe-thing --thing-name demo-thing` → HTTP 404 ResourceNotFoundException and `list-things` no longer lists it.
