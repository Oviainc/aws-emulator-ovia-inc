Issue-Severity: HIGH
Issue-Gap-Class: stub
Issue-Commit: aa65c5ca57f194f253214a65a9ade62a5cf00b08

# Implement iot UpdateThing: the thing-registry lifecycle's only unwritten handler (answers 501)

_Filed: 2026-11-02T00:02:17Z_
_Operation: service-com-amazonaws-iot-awsiotservice | Class: stub_

> _Note: this issue was identified against the code base at commit `aa65c5ca57f194f253214a65a9ade62a5cf00b08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service is NOT an all-stub scaffold: the thing registry — the service's primary resource — is already implemented and stateful. CreateThing persists a `RegisteredThing` into `store.things`, DescribeThing reads it back (attributes, version, thingId, thingArn), and DeleteThing/ListThings are implemented too. The one hole in that lifecycle is UpdateThing, whose handler is still the unmodified scaffold: it carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented("UpdateThing")`, i.e. HTTP 501.

Consequences for the happy path create -> describe -> update -> describe -> delete: a client can create a thing and read it, but can never change its `attributePayload`, attach or `removeThingType`, or exercise optimistic concurrency with `expectedVersion`. The persisted record already has the field an update must move — `RegisteredThing.version` ("The registry version of this record", state.rs:142) — and every thing created stays at `FIRST_THING_VERSION` forever, because nothing in the crate ever increments it. DescribeThing's own modelled doc points callers at the operation that does not exist: "pass the version information in the expectedVersion parameter of the UpdateThing and DeleteThing calls".

What the handler must do, from OPERATIONS.md:1963-1968 (`PATCH /things/{thingName}`): resolve `thingName` in `store.things` (404 `ResourceNotFoundException` when absent); if `expectedVersion` is supplied and does not equal the stored `version`, refuse with the modelled `VersionConflictException` (HTTP 409); apply `attributePayload` (honouring its merge semantics) and `thingTypeName`, or clear the type when `removeThingType` is true (resolving a named type against `store.thing_types` exactly as CreateThing does at create_thing.rs:183-208, including the deprecated-type refusal); bump the stored `version`; persist and answer the empty output. A subsequent DescribeThing must report the updated attributes/type and the bumped version, and the SCAFFOLDED marker line at update_thing.rs:4 must be deleted so the stub census stops counting it.

No bootstrap issue is warranted for this service: the emulator is alive and stateful (see the many already-filed per-operation iot gap issues), so this is the lifecycle-completing gap, not a bootstrap.

## Evidence

STUB SIDE — services/service-com-amazonaws-iot-awsiotservice/src/ops/update_thing.rs:4: "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and"; :172: "Err(crate::scaffolded::Scaffolded::not_implemented(\"UpdateThing\"))".

IMPLEMENTED SIBLINGS (same crate, same resource) — create_thing.rs:227 "if let Some(existing) = store.things.get(&input.thing_name) {" and :267 "store.things.insert(thing.thing_name.clone(), thing);" (no SCAFFOLDED marker in the file); describe_thing.rs:172 "let thing = store.things.get(&input.thing_name).ok_or_else(|| {", :190 "attributes: Some(thing.attributes()),", :191 "version: thing.version,". `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` over src/ops/ lists update_thing.rs but NOT create_thing.rs / describe_thing.rs / delete_thing.rs / list_things.rs.

STATE THE UPDATE MUST MOVE — src/state.rs:141-142: "/// The registry version of this record." / "pub version: i64,"; :145 "/// The `version` a freshly registered thing carries." (FIRST_THING_VERSION), set by RegisteredThing::new (state.rs:153-156) and never incremented anywhere in the crate.

MODEL/DOC SIDE — src/ops/describe_thing.rs:68-70 (rendered from the Smithy model): "* `version` (Version, default 0) **required** — The current version of the thing record in ... pass the version information in the expectedVersion parameter of the UpdateThing and" ...; OPERATIONS.md:1963-1968: "## UpdateThing" / "- **Route:** `PATCH /things/{thingName} -> 200`" / "- **Input:** `thingName` (... @httpLabel, required), `thingTypeName` (...), `attributePayload` (AttributePayload), `expectedVersion` (long), `removeThingType` (boolean, default false, required)" / "- **Errors:** ... `ResourceNotFoundException (HTTP 404)` ... `VersionConflictException (HTTP 409)`".

No existing issue covers UpdateThing (list_existing_issues(query="UpdateThing") returns only iot-data UpdateThingShadow and CFN issues).

## Steps to Reproduce

1. Start the emulator with the com-amazonaws-iot-awsiotservice crate mounted.
2. aws iot create-thing --thing-name demo-thing --attribute-payload '{"attributes":{"color":"red"}}' -> 200, returns thingName/thingId/thingArn (implemented).
3. aws iot describe-thing --thing-name demo-thing -> 200, attributes {"color":"red"}, version 1 (implemented, read from persisted state).
4. aws iot update-thing --thing-name demo-thing --attribute-payload '{"attributes":{"color":"blue"},"merge":true}' -> HTTP 501 NotImplemented from update_thing.rs:172 instead of applying the change.
5. aws iot describe-thing --thing-name demo-thing -> still attributes {"color":"red"} and version 1: the thing record can never be modified and its stored `version` (state.rs:142) can never advance, so `expectedVersion` on DeleteThing/UpdateThing is untestable.
