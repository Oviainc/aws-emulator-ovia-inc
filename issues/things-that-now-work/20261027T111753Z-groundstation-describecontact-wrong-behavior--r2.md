Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 861e7723cbd520f91d3c8b29f6571221cf90c671

# A contact's dataflowList reports a dataflow endpoint whose clock-owed creating -> created move was never materialised

_Filed: 2026-10-27T11:17:53Z_
_Operation: DescribeContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `861e7723cbd520f91d3c8b29f6571221cf90c671`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two operations that report a contact's `dataflowList` (`DescribeContact` and `DescribeContactVersion`) build it from `state::contact_dataflow_list`, which for a `dataflowEndpointConfig` end reaches `state::dataflow_endpoint(..)` -> `state::reported_endpoint(..)` and so embeds a stored `EndpointDetails` — including the dataflow endpoint's `endpoint.status`, `healthStatus` and `healthReasons` — into the response.

Both handlers call `guard.settle_due_contacts()` before reading, but NEITHER calls `guard.settle_due_endpoints()`, the sibling settler whose own doc says a reporting operation calls it before reading. `GetDataflowEndpointGroup` — the other read of the same stored endpoint — does call it.

Consequence: a dataflow endpoint created by `CreateDataflowEndpointGroup` is stored `creating` / `UNHEALTHY` / `INITIALIZING_DATAPLANE` with `endpoints_settle_at` set, and its move to `created` / `HEALTHY` / `HEALTHY` is only materialised by whoever reads next. If the next read is `DescribeContact` (or `DescribeContactVersion`), the move is NOT applied and the contact's `dataflowList` keeps describing the endpoint as still being attached, arbitrarily long after the move fell due. The same `DescribeContact` call then changes its answer — with nothing about the contact or the group having changed — as soon as anybody happens to call `GetDataflowEndpointGroup`, because that call performs the settle the contact read skipped. That contradicts the crate's own rule that one stored endpoint cannot be described differently by two readers.

(The earlier, now-fixed DescribeContact/wrong-behavior issue was about `dataflowList` being hardcoded absent and `contactStatus` frozen at SCHEDULED; it did not cover the endpoint settle, which only became reachable once `dataflowList` started being derived.)

## Evidence

Crate rule the code breaks — `src/state.rs:4004-4007` (doc of `settle_due_endpoints`):
  "/// Materialise every `creating` -> `created` endpoint move the clock has already made
   /// due. A reporting operation calls this before reading: the move was owed at this
   /// instant whether or not anybody called, so applying it changes nothing a later
   /// caller could otherwise have seen."

and `src/state.rs:3744-3745` (doc of `reported_endpoint_details`):
  "/// Shared by every read that reports an endpoint, so two readers of one stored endpoint
   /// cannot describe its health differently."

The reader that honours it — `src/ops/get_dataflow_endpoint_group.rs:159-162`:
  "// A `creating` endpoint becomes `created` at an instant the create recorded, so
   // the move was owed at this instant whether or not anybody read: materialising it
   // before reading changes nothing a later caller could otherwise have seen.
   guard.settle_due_endpoints();"

The readers that skip it — `src/ops/describe_contact.rs:183` is only `guard.settle_due_contacts();`, and `:197` then builds the endpoint-bearing response: `let dataflow_list = guard.contact_dataflow_list(&contact.mission_profile_arn, &contact.region);`. Same in `src/ops/describe_contact_version.rs:216` (`guard.settle_due_contacts();`) and `:255-256` (`dataflow_list: guard.contact_dataflow_list(..)`). Neither file contains `settle_due_endpoints` (grep for `settle_due_` over `src/ops` matches it only in `get_dataflow_endpoint_group.rs:162`).

That the embedded value is the un-settled stored one — `src/state.rs:3438-3440`: `Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) => state.dataflow_endpoint(&endpoint_config.dataflow_endpoint_name).map(ConfigDetails::endpointDetails)`, and `src/state.rs:3731`: `.map(|details| self.reported_endpoint(details, group))` — `reported_endpoint` only overlays AGENT health (`apply_agent_endpoint_health`), it does not apply the `creating` -> `created` move, which lives solely in `settle_due_endpoints` (`src/state.rs:4022-4032`).

Test coverage: the one test that compares a contact's reported endpoint against the group's is written to read the GROUP FIRST, which is what performs the settle — `tests/all/contact_dataflows_and_pass_stages.rs:221-224`:
  "// The dataflow endpoint's details as the group itself reports them — what the contact
   // must report for the end of a dataflow wired to that endpoint. Read first, so both
   // reads are taken after the endpoint's `creating` -> `created` move is due.
   std::thread::sleep(AFTER_SETTLE);"
So no test exercises a contact read taken after the settle span WITHOUT a prior `GetDataflowEndpointGroup`, and the defect is masked by that ordering.

## Steps to Reproduce

1. CreateDataflowEndpointGroup with one `endpointDetails[0].endpoint` (a plain dataflow endpoint, name "Downlink-1", with securityDetails) — it is stored `creating` / `UNHEALTHY` / `INITIALIZING_DATAPLANE` and `endpoints_settle_at = now + ENDPOINT_CREATION_SETTLES_AFTER`.
2. CreateConfig a `dataflowEndpointConfig` naming `dataflowEndpointName: "Downlink-1"`, plus an `antennaDownlinkConfig` and a `trackingConfig`.
3. CreateMissionProfile with `dataflowEdges=[[antennaDownlinkConfigArn, dataflowEndpointConfigArn]]`.
4. ReserveContact against that profile.
5. Sleep well past `state::ENDPOINT_CREATION_SETTLES_AFTER` (300 ms) so the endpoint's `creating` -> `created` move is due, and do NOT call GetDataflowEndpointGroup.
6. GET /contact/{contactId} (DescribeContact): `dataflowList[0].destination.configDetails.endpointDetails.endpoint.status` is still `creating`, with `healthStatus: UNHEALTHY` / `healthReasons: [INITIALIZING_DATAPLANE]`, even though the move fell due long ago.
7. Now GET /dataflowEndpointGroup/{groupId} once (it calls `settle_due_endpoints`), then repeat DescribeContact: the same request now answers `created` / `HEALTHY` — the contact read's answer depended on an unrelated call having been made.
8. Repeat steps 5-7 with GET /contact/{contactId}/version/{versionId} (DescribeContactVersion) for the identical behaviour.
