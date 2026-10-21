Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the IoT Data Plane (iotmoonrakerservice) emulator

_Filed: 2026-10-21T23:17:44Z_
_Operation: service-com-amazonaws-iotdataplane-iotmoonrakerservice | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 11 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields at all (`// TODO: add service-specific state fields`). No client workflow works.

First slice: implement the **thing-shadow lifecycle plus retained-message publish/read** loop statefully (restJson1, sigv4 signing name `iotdata`). Operations, in workflow order:

1. **UpdateThingShadow** — `POST /things/{thingName}/shadow?name={shadowName}`. Input: `thingName` (required, @httpLabel), `shadowName` (optional @httpQuery(name); absent = the unnamed/classic shadow), `payload` (required @httpPayload, JSON document). Must merge the request's `state.desired` / `state.reported` into the stored shadow document (null leaf = delete that key), increment the shadow `version`, set `timestamp`, recompute `state.delta` (desired keys that differ from reported), and return the resulting accepted document as `payload` (version + timestamp + metadata), not the request echoed back. If the request carries a `version` that does not match the stored version, return `ConflictException` (HTTP 409). A payload that is not parseable JSON → `InvalidRequestException` (400).
2. **GetThingShadow** — `GET /things/{thingName}/shadow?name={shadowName}`. Returns the *persisted* document (`state`, `metadata`, `version`, `timestamp`) as `payload`. Unknown thing or unknown shadow name → `ResourceNotFoundException` (404), not an empty 200.
3. **ListNamedShadowsForThing** — `GET /api/things/shadow/ListNamedShadowsForThing/{thingName}`. Returns `results` = the named shadows stored for that thing (the classic unnamed shadow is NOT listed), plus `timestamp`, honouring `pageSize` (1..100) and round-tripping `nextToken`. Thing with no shadows → `ResourceNotFoundException` (404).
4. **DeleteThingShadow** — `DELETE /things/{thingName}/shadow?name={shadowName}`. Removes the shadow from state and returns the deletion acknowledgement document as `payload` (required member). A second delete of the same shadow → `ResourceNotFoundException` (404); after deletion GetThingShadow must 404 and the name must disappear from ListNamedShadowsForThing.
5. **Publish** — `POST /topics/{topic}?qos=&retain=`. Input: `topic` (@httpLabel), `qos` (0..1), `retain`, `payload` (@httpPayload), `userProperties` (header `x-amz-mqtt5-user-properties`), `contentType`, `responseTopic`, `messageExpiry`. Empty output body/200. When `retain=true` the message must be STORED as the retained message for that topic (payload, qos, userProperties, lastModifiedTime); publishing an empty payload with `retain=true` clears it. `qos` outside 0..1 → 400.
6. **GetRetainedMessage** — `GET /retainedMessage/{topic}`. Must return the retained message stored by step 5: `topic`, `payload` (byte-identical to what was published), `qos`, `lastModifiedTime` (millis), `userProperties`. No retained message for the topic → `ResourceNotFoundException` (404).

Every one of these must compute its response from persisted state held in the engine store — no canned responses, no echoing the request payload back, no random version/timestamp without storing it. Operations outside this slice (GetConnection, DeleteConnection, ListSubscriptions, SendDirectMessage, ListRetainedMessages) stay at 501 and will be filed as individual gap issues later.

## State Model

Add to `ComAmazonawsIotdataplaneIotmoonrakerserviceState`:

- `shadows: BTreeMap<(String /*thingName*/, String /*shadowName, "" = classic*/), Shadow>`
  - `Shadow { state_desired: serde_json::Value, state_reported: serde_json::Value, metadata: serde_json::Value, version: i64, timestamp_secs: i64 }`
  - `delta` is derived at read/update time from `state_desired` vs `state_reported`, not stored.
- `retained_messages: BTreeMap<String /*topic*/, RetainedMessage>`
  - `RetainedMessage { topic: String, payload: Vec<u8>, qos: i32, last_modified_time_millis: i64, user_properties: Option<Vec<u8>> }`

Both maps live behind the existing `crate::state::service_state(&*self.engine)` guard so state is shared across requests. Keeping the maps ordered gives ListNamedShadowsForThing / ListRetainedMessages a stable key order for offset-style `nextToken`s.

## Evidence

OPERATIONS.md (crate root) lists all 11 operations with routes, members and errors: UpdateThingShadow `POST /things/{thingName}/shadow` with required `payload` @httpPayload and `ConflictException (409)`; GetThingShadow `GET /things/{thingName}/shadow` with `ResourceNotFoundException (404)`; ListNamedShadowsForThing `GET /api/things/shadow/ListNamedShadowsForThing/{thingName}` with `results`/`nextToken`/`timestamp` and `pageSize` range 1..100; DeleteThingShadow `DELETE /things/{thingName}/shadow` with required `payload`; Publish `POST /topics/{topic}` with `retain`, `qos` range 0..1, `userProperties` header; GetRetainedMessage `GET /retainedMessage/{topic}` returning `topic`/`payload`/`qos`/`lastModifiedTime`/`userProperties` with `ResourceNotFoundException (404)`. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 11 handler files (e.g. src/ops/update_thing_shadow.rs:4, whose `handle` is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateThingShadow"))` at line 175). src/state.rs:7-9 shows the state struct is empty. Shadow versioning/delta and MQTT retained-message semantics are the documented, well-established behaviour of the AWS IoT Data Plane.

## Steps to Reproduce

1. Point an AWS SDK/CLI `iot-data` client at the emulator endpoint (restJson1, signing name `iotdata`).
2. `update-thing-shadow --thing-name myThing --payload '{"state":{"reported":{"temp":20}}}'` → 200 with a payload containing `state.reported.temp = 20`, `version = 1`, `metadata` and `timestamp`.
3. `get-thing-shadow --thing-name myThing` → 200 whose payload equals the stored document (version 1, reported temp 20).
4. `update-thing-shadow --thing-name myThing --payload '{"state":{"desired":{"temp":25}}}'` → version becomes 2 and a subsequent get shows both reported temp 20 and desired temp 25 plus a `delta` of temp 25.
5. `update-thing-shadow --thing-name myThing --shadow-name cfg --payload '{"state":{"reported":{"a":1}}}'` then `list-named-shadows-for-thing --thing-name myThing` → `results` contains `cfg` (and not the classic shadow).
6. `delete-thing-shadow --thing-name myThing --shadow-name cfg` → 200; repeat the call → 404 ResourceNotFoundException; `list-named-shadows-for-thing` no longer returns `cfg`.
7. `publish --topic my/topic --qos 1 --retain --payload $(echo -n hello | base64)` → 200 empty body.
8. `get-retained-message --topic my/topic` → 200 with payload `hello`, `qos` 1 and a non-zero `lastModifiedTime`.
9. `get-retained-message --topic other/topic` → 404 ResourceNotFoundException.
10. Every step above currently returns HTTP 501 NotImplemented.
