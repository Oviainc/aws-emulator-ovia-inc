Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f75cbb9a461527dc7539f53b0560573b007d83b1

# DeleteDataflowEndpointGroup has no in-use guard, so a non-terminal contact's dataflow silently loses the endpoint it is wired to

_Filed: 2026-10-29T03:28:36Z_
_Operation: DeleteDataflowEndpointGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `f75cbb9a461527dc7539f53b0560573b007d83b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other delete in this crate refuses to drop a resource that something this account still holds depends on, and says so in a comment on the guard. `DeleteDataflowEndpointGroup` has no guard at all: its only failure path is the not-found lookup, after which `remove_dataflow_endpoint_group` drops the group and its ARN index entry unconditionally.

The dependency it ignores is real and the crate already resolves it on every read of a contact. A mission profile's `dataflowEndpointConfig` selects an endpoint of a group BY NAME, and `ComAmazonawsGroundstationGroundstationState::dataflow_endpoint(name)` walks every group to find it. Once the group holding that endpoint is deleted, the lookup answers `None`, so `dataflow_end` builds the edge end with `config_details` absent — i.e. a contact that is still to be flown reports a `dataflow-endpoint` end with no endpoint behind it, which is exactly the "holder pointing at a reference that no longer resolves" state `DeleteConfig`, `DeleteMissionProfile` and `DeleteEphemeris` each exist to prevent.

The refusal is expressible with the errors this operation models: `DeleteConfig` and `DeleteMissionProfile` answer their in-use refusals with `InvalidParameterException` (HTTP 431), which `DeleteDataflowEndpointGroup` also declares. The crate also already has the pieces the guard composes from — `endpoint_name(details)` reads an endpoint's name whichever member describes it, and `contacts_using_mission_profile` is the non-terminal-contact rule the sibling deletes use.

## Evidence

The unguarded delete — `services/service-com-amazonaws-groundstation-groundstation/src/ops/delete_dataflow_endpoint_group.rs:143-159`:
```
        // The group and the ARN index entry that resolved to it go together, so a
        // reference to a deleted group resolves to nothing — exactly as one to a group
        // that never existed does. `@idempotent` is about repeating the SAME delete, which
        // this service answers the way it answers every other delete of a resource that is
        // already gone: `ResourceNotFoundException`.
        let removed = state
            .lock()
            .remove_dataflow_endpoint_group(&input.dataflow_endpoint_group_id)
            .ok_or_else(|| { ... ResourceNotFoundException ... })?;
```
and `src/state.rs:4874-4885`, which only drops the record and the index entry:
```
    /// Remove the dataflow endpoint group `dataflow_endpoint_group_id` addresses, along
    /// with its ARN index entry, and answer it; `None` when no group has that id.
    pub fn remove_dataflow_endpoint_group(
```

The rule the three sibling deletes of THIS crate state and apply:
* `src/ops/delete_config.rs:168-175` — "A config another resource this account still holds depends on is NOT deleted: a holder names the config by ARN, and dropping it out from under the holder would leave that holder pointing at a reference that no longer resolves" (then `mission_profiles_referencing` / `configs_referencing`, refused with `InvalidParameterException`).
* `src/ops/delete_mission_profile.rs:165-182` — "A mission profile a contact still to be flown depends on is NOT deleted: a contact names its profile by ARN and reads the wiring of its pass back out of it, so dropping the profile out from under it would leave a live reservation pointing at a reference that no longer resolves." (`contacts_using_mission_profile`, `InvalidParameterException`).
* `src/ops/delete_ephemeris.rs:185-199` — "An ephemeris a contact still to be flown depends on is not removed: dropping it would take the trajectory out from under a pass that is still to happen." (`contacts_using_ephemeris`, `ResourceInUseException`).

The reference the delete breaks, resolved by this crate on every contact read — `src/state.rs:4667-4689`:
```
    /// The dataflow endpoint of this account named `name`, as a read of the group that
    /// holds it reports it ([`Self::reported_endpoint`]); `None` when no group of this
    /// account holds one by that name.
    pub fn dataflow_endpoint(&self, name: &str) -> Option<EndpointDetails> {
```
and the response member that then goes missing — `src/state.rs:4199-4201` and `src/state.rs:4220-4223`:
```
/// * `dataflow-endpoint`: the endpoint of this account the config names, as the dataflow
///   endpoint group that holds it stored it; absent when no group holds one by that name,
///   since there is then no endpoint to describe.
...
        Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) => state
            .dataflow_endpoint(&endpoint_config.dataflow_endpoint_name)
            .map(ConfigDetails::endpointDetails),
```
This is what `DescribeContact` (`src/ops/describe_contact.rs:203`) and `DescribeContactVersion` (`src/ops/describe_contact_version.rs:262-263`) report through `contact_dataflow_list`.

The operation models the error the siblings use: `smithy_query(query="operation", operation="DeleteDataflowEndpointGroup")` -> errors `DependencyException (531)`, `InvalidParameterException (431)`, `ResourceNotFoundException (434)`.

Test coverage: nothing pins the current acceptance. `tests/all/dataflow_endpoint_group_deletion.rs:12-14` states the suite's scope explicitly — "The group's contents are not part of the rule, so the table spans what a group can hold" — and no row of it, nor any test in `tests/all/dataflow_endpoint_groups.rs` or `tests/all/contact_endpoint_settle_consistency.rs`, deletes a group a live contact's dataflow is wired to.

## Steps to Reproduce

1. CreateDataflowEndpointGroup with one `endpointDetails[0].endpoint` named "downlink-1" (plus `securityDetails`); note the returned `dataflowEndpointGroupId`.
2. CreateConfig an `antennaDownlinkConfig`, and CreateConfig a `dataflowEndpointConfig` with `dataflowEndpointName: "downlink-1"`.
3. CreateConfig a `trackingConfig`, then CreateMissionProfile with `dataflowEdges: [[<antenna-downlink ARN>, <dataflow-endpoint ARN>]]`.
4. ReserveContact a future window against that profile; GET /contact/{contactId} — the `dataflowList[0].destination.configDetails.endpointDetails` carries the endpoint.
5. DELETE /dataflowEndpointGroup/{dataflowEndpointGroupId} — answers 200 even though the contact is still non-terminal.
6. GET /contact/{contactId} again — the same edge end now reports no `configDetails` at all: the dataflow of a pass still to be flown has lost its endpoint. Compare DELETE /missionprofile/{id} or DELETE /ephemeris/{id} for the same contact, both of which refuse while the contact is non-terminal.
