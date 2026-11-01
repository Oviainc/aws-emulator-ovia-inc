Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# Updating a Config retroactively changes the execution parameters reported for contacts already scheduled with it, which the operation's own model doc forbids

_Filed: 2026-10-28T12:16:40Z_
_Operation: UpdateConfig | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states, for this operation, that updating a `Config` must NOT update the execution parameters of contacts already scheduled with that config. The handler replaces `stored.config_data` in place, and a contact's dataflow ends are resolved from the config store as it stands NOW: `dataflow_end` looks the edge's `ConfigArn` up live (`state.config_by_arn(arn)`) and builds `configDetails` out of whatever `configData` the config currently holds — the `dataflowEndpointName` it resolves an endpoint by, the `s3RecordingConfig`'s `bucketArn`/`prefix`, and so on.

So for a contact that is still SCHEDULING/SCHEDULED — an "existing future contact scheduled with this Config" — a single `UpdateConfig` call changes what `DescribeContact`/`DescribeContactVersion` report for it: e.g. repointing an `s3RecordingConfig` at another bucket/prefix, or renaming the `dataflowEndpointName`, changes that contact's reported `dataflowList.destination.configDetails` after the fact. The contact is thereby flown with parameters it was not reserved with.

The same inconsistency the sibling `UpdateMissionProfile` issue describes applies here: `ReserveContact` freezes the profile's pre/post-pass durations onto the contact at reserve time, but the config-derived wiring stays live. The defect is the retroactive reach; the pre-update `configData` seen by every non-terminal contact that reaches the config has to survive the update.

## Evidence

Model (smithy_query(query="doc", shape="UpdateConfig")): "Updates the <code>Config</code> used when scheduling contacts." / "Updating a <code>Config</code> will not update the execution parameters for existing future contacts scheduled with this <code>Config</code>."

Handler replaces the parameters in place — services/service-com-amazonaws-groundstation-groundstation/src/ops/update_config.rs:272-275:
```
        // Both members are required, so an update REPLACES the config's name and its
        // parameters; its id, type and ARN are its address and do not move.
        stored.name = input.name;
        stored.config_data = config_data;
```

A contact's dataflow ends are read LIVE from that same store — services/service-com-amazonaws-groundstation-groundstation/src/state.rs:3982-3999:
```
    let (config_type, config_id) = config_arn_parts(arn)?;
    let config_data = state.config_by_arn(arn).map(|config| &config.config_data);

    let config_details = match config_data {
        Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) => state
            .dataflow_endpoint(&endpoint_config.dataflow_endpoint_name)
            .map(ConfigDetails::endpointDetails),
...
        Some(ConfigTypeData::s3RecordingConfig(recording)) => {
            Some(ConfigDetails::s3RecordingDetails(S3RecordingDetails {
                bucket_arn: Some(recording.bucket_arn.clone()),
                key_template: recording.prefix.clone(),
```
reached per contact through services/service-com-amazonaws-groundstation-groundstation/src/state.rs:4444-4456 (`contact_dataflow_list`) and services/service-com-amazonaws-groundstation-groundstation/src/ops/describe_contact.rs:203:
```
        let dataflow_list = guard.contact_dataflow_list(&contact.mission_profile_arn, &contact.region);
```

The crate already proves it knows how to freeze a referenced resource's parameters onto a contact — services/service-com-amazonaws-groundstation-groundstation/src/ops/reserve_contact.rs:288-297 stores `pre_pass_start_time`/`post_pass_end_time` computed from the profile at reserve time, so a later change to those durations does not move an existing contact's window.

Test coverage: tests/all/contact_dataflows_and_pass_stages.rs:326 asserts the `dataflowList` entries and their `configDetails` for a freshly reserved contact; tests/all/config_defaults_and_references.rs and tests/all/s3_recording_buckets.rs cover UpdateConfig's own gates. No test reads an already-reserved contact after its configs were updated, so nothing pins today's retroactive behaviour.

## Steps to Reproduce

1. CreateConfig an `s3RecordingConfig` (bucketArn B1, prefix P1) and the configs a dataflow edge needs; CreateMissionProfile wiring an edge whose destination is that s3-recording config.
2. ReserveContact against the profile for a future window; GET /contact/{contactId} and record `dataflowList[0].destination.configDetails.s3RecordingDetails` (bucketArn B1, keyTemplate P1).
3. Call UpdateConfig on the same `(configType, configId)` with `configData.s3RecordingConfig` naming prefix P2 (same type, as the handler requires).
4. GET /contact/{contactId} again for the SAME, still-future contact.
5. Observed: `s3RecordingDetails.keyTemplate` is now P2 — the execution parameters of a contact already scheduled with this Config changed. Expected per the operation's own documentation: the contact keeps the parameters it was scheduled with.
