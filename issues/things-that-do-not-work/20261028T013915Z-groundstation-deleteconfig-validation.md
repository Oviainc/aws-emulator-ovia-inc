Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# DeleteConfig's in-use guard only counts mission profiles, so deleting the antenna-uplink config an uplinkEchoConfig echoes leaves a dangling reference

_Filed: 2026-10-28T01:39:15Z_
_Operation: DeleteConfig | Class: validation_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteConfig` refuses to remove a config that a mission profile still references, and its own comment states the rule it is enforcing: a reference must not be left pointing at something that no longer resolves. But the only inbound references it counts are the mission profile's (`mission_profiles_referencing`). A config-to-config reference — `uplinkEchoConfig$antennaUplinkConfigArn`, which this very crate refuses on both `CreateConfig` and `UpdateConfig` when the ARN resolves to no config — is not counted at all.

So an `antenna-uplink` config that an existing `uplink-echo` config echoes can be deleted with 200. Afterwards the stored `uplinkEchoConfig` carries an `antennaUplinkConfigArn` that resolves to nothing: `GetConfig`/`ListConfigs` keep reporting it, while re-creating or updating that same echo config with the identical body is refused 434. The crate reaches a state its own create/update gate exists to prevent, and it reaches it through a path that has the guard concept already (`DeleteConfig` for mission profiles, `DeleteMissionProfile` for contacts, `DeleteEphemeris` for contacts) and simply omits this reference kind.

## Evidence

The rule DeleteConfig says it enforces, and the only references it actually counts — services/service-com-amazonaws-groundstation-groundstation/src/ops/delete_config.rs:168-184:
```
// A config a live mission profile still depends on is NOT deleted: a mission
// profile names its configs by ARN, and dropping one out from under it would
// leave it pointing at a reference that no longer resolves. So the inbound
// references are counted first and, when there are any, the delete is refused as
// a bad parameter naming the profiles that hold them
...
let referencing = guard.mission_profiles_referencing(&config_arn);
```
That helper is documented as covering mission-profile members only — state.rs:3988-3997:
```
/// The ARNs of the mission profiles that reference the config `config_arn` names, in
/// a stable order; empty when nothing references it.
...
/// shape="CreateMissionProfileRequest")`) — so a config is in use when ANY of them
/// names it, not only the tracking one.
pub fn mission_profiles_referencing(&self, config_arn: &str) -> Vec<String> {
```
The other inbound reference the crate knows about and gates on write — src/ops/create_config.rs:214-244:
```
// An `uplinkEchoConfig` echoes a specific ANTENNA UPLINK config, named by ARN:
// like every ARN-valued reference it must resolve to a config that exists, and
// the config it names must be of the type the member is for
...
Some(crate::state::TypedReferenceFault::Absent) => {
    return Err(CreateConfigErrorModeled::ResourceNotFoundException(
```
and the same gate on the update path — src/ops/update_config.rs:222-243:
```
// An `uplinkEchoConfig` echoes a specific ANTENNA UPLINK config, named by ARN:
// the update is held to the same reference rule, and answers the same two texts,
// as the create, so an echo config cannot be pointed at a config that does not
// exist (a missing resource) or at one of another type (a bad parameter).
```
And state.rs states explicitly that a deleted config is the same as one that never existed for every reference holder — state.rs:3897-3902:
```
/// Whether `arn` resolves to a config this service still holds, of `required_type`
/// ...
/// Shared by every operation that takes a config reference, so a reference to a
/// config that was never created and one to a config that has since been DELETED
/// are refused the same way.
```
Test coverage: no existing test deletes an echoed antenna-uplink config. `tests/all/config_defaults_and_references.rs:573-747` creates the echo source at line 581 and never deletes it; `tests/all/lifecycle_writes.rs:74-169` deletes each config the table creates, with the echo-source uplink config (line 79) left alive and the `uplink-echo` entry itself deleted inside the loop; `tests/all/config_listing.rs:235-246`, `tests/all/resource_tagging.rs:636-647` and `tests/all/tag_removal.rs:320-331` delete only `dataflow-endpoint`/`tracking` configs. So adding the guard breaks no pinned behaviour.

## Steps to Reproduce

1. POST /config with {"name":"echo source","configData":{"antennaUplinkConfig": <valid uplink body>}} — note the returned configId A and configArn.
2. POST /config with {"name":"echo","configData":{"uplinkEchoConfig":{"enabled":true,"antennaUplinkConfigArn":"<A's configArn>"}}} — 200, note configId E.
3. DELETE /config/antenna-uplink/{A} — observe 200; expected the in-use refusal (InvalidParameterException, HTTP 431) the sibling mission-profile reference earns, since an inbound reference still names it.
4. GET /config/uplink-echo/{E} — observe 200 with configData.uplinkEchoConfig.antennaUplinkConfigArn still naming A, an ARN that now resolves to no config.
5. POST /config with the identical echo body from step 2 — observe 434 ResourceNotFoundException, proving the stored echo config now holds a reference the service itself refuses to accept.
