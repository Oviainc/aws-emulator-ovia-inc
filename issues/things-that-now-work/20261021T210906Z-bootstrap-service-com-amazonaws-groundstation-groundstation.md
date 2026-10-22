Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the groundstation emulator

_Filed: 2026-10-21T21:09:06Z_
_Operation: service-com-amazonaws-groundstation-groundstation | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 40 handlers under `services/service-com-amazonaws-groundstation-groundstation/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no AWS Ground Station client workflow can run.

Implement a FIRST, SMALL slice: the core Ground Station scheduling loop — define a tracking Config, define a Mission Profile that references it, reserve a contact against it, read the contact back, and cancel it. Protocol is `restJson1`, signing name `groundstation`.

Happy path (in order, with the key fields each must honour, per OPERATIONS.md):

1. **CreateConfig** — `POST /config`. Input: `name` (1..256, `^[ a-zA-Z0-9_:-]{1,256}$`, required), `configData` (ConfigTypeData union — at minimum support the `trackingConfig` and `antennaDownlinkConfig` members, required), `tags`. Output: `configId` (uuid), `configType` (from which union member was set, e.g. `tracking`), `configArn` of the form `arn:aws:groundstation:<region>:<account>:config/<config-type>/<configId>`. Store the config; do not fabricate an id without persisting it.
2. **GetConfig** — `GET /config/{configType}/{configId}` (@readonly). Must return the stored `configId`, `configArn`, `name`, `configType` and the exact `configData` and `tags` that were created. Unknown id, or an id whose stored type does not match the `{configType}` label, must return `ResourceNotFoundException` (HTTP 434 per the model), not 200 and not 501.
3. **CreateMissionProfile** — `POST /missionprofile`. Input: `name` (required), `minimumViableContactDurationSeconds` (1..21600, required), `dataflowEdges` (list<list<string>>, required), `trackingConfigArn` (required), optional `contactPrePassDurationSeconds`/`contactPostPassDurationSeconds` (0..21600), `telemetrySinkConfigArn`, `tags`. Output: `missionProfileId`. Must validate that `trackingConfigArn` (and each ARN appearing in `dataflowEdges`) resolves to a config created in step 1, else `ResourceNotFoundException`. Persist all inputs plus a derived `missionProfileArn` (`arn:aws:groundstation:<region>:<account>:mission-profile/<missionProfileId>`).
4. **GetMissionProfile** / **ListMissionProfiles** — `GET /missionprofile/{missionProfileId}` must echo back from state: `missionProfileId`, `missionProfileArn`, `name`, `region`, `contactPrePassDurationSeconds`, `contactPostPassDurationSeconds`, `minimumViableContactDurationSeconds`, `dataflowEdges`, `trackingConfigArn`, `telemetrySinkConfigArn`, `tags`. `GET /missionprofile` must list every created profile as `MissionProfileListItem`s and honour `maxResults`/`nextToken` (paginated: inputToken=nextToken, outputToken=nextToken, items=missionProfileList).
5. **ReserveContact** — `POST /contact`. Input: `missionProfileArn` (required), `startTime` (required), `endTime` (required), `groundStation` (4..97, required), optional `satelliteArn`, `tags`, `trackingOverrides`. Output: `contactId` (uuid) and `versionId` (1..128, starts at 1). Must reject an unknown `missionProfileArn` with `ResourceNotFoundException`, reject `endTime <= startTime` and a duration shorter than the profile's `minimumViableContactDurationSeconds` with `InvalidParameterException` (HTTP 431). Persist a contact in state with `contactStatus = SCHEDULED` and `prePassStartTime`/`postPassEndTime` computed from the profile's pre/post-pass durations.
6. **DescribeContact** — `GET /contact/{contactId}` (@readonly). Must return the persisted contact: `contactId`, `missionProfileArn`, `satelliteArn`, `startTime`, `endTime`, `prePassStartTime`, `postPassEndTime`, `groundStation`, `contactStatus`, `tags`, `region`, `trackingOverrides`, `version`. Unknown id ⇒ `ResourceNotFoundException`.
7. **ListContacts** — `POST /contacts` (paginated, items=contactList). Input: `statusList` (required), `startTime` (required), `endTime` (required), optional `groundStation`, `satelliteArn`, `missionProfileArn`, `maxResults`, `nextToken`. Must return only contacts from state whose status is in `statusList` and whose window overlaps [`startTime`,`endTime`], filtered by the optional ground station / satellite / mission-profile arguments, and must honour `maxResults` + round-trip `nextToken`.
8. **CancelContact** — `DELETE /contact/{contactId}` (@idempotent). Must flip the stored contact's `contactStatus` to `CANCELLED` and return `contactId` + `versionId`; a following `DescribeContact` must report `CANCELLED`, and `ListContacts` with `statusList=[SCHEDULED]` must no longer return it. Unknown id ⇒ `ResourceNotFoundException`.

Every response must be computed from persisted state — NO canned bodies, NO echoing the request back, NO generating an id that is never stored. A create followed by a get/list must return exactly what was created, and a cancel must be observable by the next reader.

## State Model

In `src/state.rs` (`ComAmazonawsGroundstationGroundstationState`, reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `configs: HashMap<(ConfigCapabilityType, String /*configId*/), Config>` where
  `Config { config_id: String, config_arn: String, name: String, config_type: ConfigCapabilityType, config_data: ConfigTypeData, tags: HashMap<String,String> }`
- `mission_profiles: HashMap<String /*missionProfileId*/, MissionProfile>` where
  `MissionProfile { mission_profile_id: String, mission_profile_arn: String, name: String, region: String, contact_pre_pass_duration_seconds: Option<i32>, contact_post_pass_duration_seconds: Option<i32>, minimum_viable_contact_duration_seconds: i32, dataflow_edges: Vec<Vec<String>>, tracking_config_arn: String, telemetry_sink_config_arn: Option<String>, tags: HashMap<String,String> }`
- `contacts: HashMap<String /*contactId*/, Contact>` where
  `Contact { contact_id: String, mission_profile_arn: String, satellite_arn: Option<String>, start_time: DateTime, end_time: DateTime, pre_pass_start_time: DateTime, post_pass_end_time: DateTime, ground_station: String, contact_status: ContactStatus, version_id: i32, tracking_overrides: Option<TrackingOverrides>, region: String, tags: HashMap<String,String> }`
- `arn_index: HashMap<String /*arn*/, ResourceRef>` so `missionProfileArn`/`trackingConfigArn` lookups (and later TagResource/ListTagsForResource) resolve without scanning.
- An `account_id`/`region` pair used to mint ARNs consistently so the generated ARNs satisfy the model's ARN patterns (config 82..424, mission-profile 89..138).

Out of scope for this first slice (stay 501, filed separately later): the agent operations (RegisterAgent, UpdateAgentStatus, GetAgentConfiguration, GetAgentTaskResponseUrl), ephemerides (Create/Describe/Update/Delete/ListEphemerides), dataflow endpoint groups (incl. V2), satellites and ground-station catalogue reads (GetSatellite, ListSatellites, ListGroundStations, ListAntennas, ListGroundStationReservations), contact versioning (UpdateContact, DescribeContactVersion, ListContactVersions), GetMinuteUsage, and tagging (TagResource/UntagResource/ListTagsForResource).

## Evidence

`services/service-com-amazonaws-groundstation-groundstation/OPERATIONS.md` documents all 40 operations, the `restJson1` protocol, sigv4 name `groundstation`, and the exact routes/members/errors quoted above (CreateConfig `POST /config`; GetConfig `GET /config/{configType}/{configId}`; CreateMissionProfile `POST /missionprofile`; GetMissionProfile `GET /missionprofile/{missionProfileId}`; ListMissionProfiles `GET /missionprofile` @paginated items=missionProfileList; ReserveContact `POST /contact` -> contactId+versionId; DescribeContact `GET /contact/{contactId}`; ListContacts `POST /contacts` @paginated items=contactList; CancelContact `DELETE /contact/{contactId}`), with `ResourceNotFoundException (HTTP 434)` and `InvalidParameterException (HTTP 431)` declared on each of them. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under `src/ops/` matches all 40 handler files, including `create_config.rs`, `get_config.rs`, `create_mission_profile.rs`, `get_mission_profile.rs`, `list_mission_profiles.rs`, `reserve_contact.rs`, `describe_contact.rs`, `list_contacts.rs` and `cancel_contact.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and touches no state. No integration test exercises this workflow.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI groundstation client at it.
2. `aws groundstation create-config --name demo-tracking --config-data '{"trackingConfig":{"autotrack":"PREFERRED"}}'` — expect 200 with a `configId`, `configType=tracking` and a well-formed `configArn`; today it returns HTTP 501 NotImplemented.
3. `aws groundstation get-config --config-id <configId> --config-type tracking` — expect the same name/configData/tags that were just created, read from state.
4. `aws groundstation create-mission-profile --name demo-profile --minimum-viable-contact-duration-seconds 180 --dataflow-edges '[]' --tracking-config-arn <configArn>` — expect 200 with a `missionProfileId`; passing a bogus tracking-config ARN must fail with ResourceNotFoundException (434).
5. `aws groundstation get-mission-profile --mission-profile-id <missionProfileId>` and `aws groundstation list-mission-profiles` — expect the created profile with all its fields, and the list to contain exactly it.
6. `aws groundstation reserve-contact --mission-profile-arn <missionProfileArn> --ground-station Ohio-1 --start-time <t0> --end-time <t0+600s>` — expect 200 with `contactId` and `versionId=1`; an `endTime` before `startTime` or a window shorter than 180s must fail with InvalidParameterException (431).
7. `aws groundstation describe-contact --contact-id <contactId>` — expect the reserved window, groundStation, missionProfileArn and `contactStatus=SCHEDULED`, all from state.
8. `aws groundstation list-contacts --status-list SCHEDULED --start-time <t0-1h> --end-time <t0+2h>` — expect the contact in `contactList`; repeat with `--max-results 1` across two pages via `nextToken`.
9. `aws groundstation cancel-contact --contact-id <contactId>`, then `describe-contact` again — expect `contactStatus=CANCELLED`, and `list-contacts --status-list SCHEDULED` to no longer return it.
10. `aws groundstation describe-contact --contact-id 00000000-0000-0000-0000-000000000000` — expect ResourceNotFoundException (HTTP 434), not 501 and not an empty 200.
