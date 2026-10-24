Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# CreateMissionProfile only checks that a dataflow edge's configs exist, never that the pair can form a dataflow

_Filed: 2026-10-24T01:21:57Z_
_Operation: CreateMissionProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A mission profile's `dataflowEdges` is a list of `[source, destination]` config-ARN pairs that describe the dataflow graph a contact runs. The handler resolves each edge member with `required_type = None`, i.e. it verifies only that the ARN names a config that still exists; the two configs' capability types are never looked at.

Expected: AWS type-checks the edge, because only certain capability pairings form a legal dataflow (downlink side: `antenna-downlink` / `antenna-downlink-demod-decode` -> `dataflow-endpoint` / `s3-recording`; uplink side: `dataflow-endpoint` -> `antenna-uplink`). An edge that cannot be a dataflow — both elements naming the SAME config, an edge whose source and destination are both `antenna-downlink` configs, or an edge naming the `tracking` config (which is wired through `trackingConfigArn`, never as a dataflow node) — is refused with the operation's modeled `InvalidParameterException` (HTTP 431) and nothing is stored.

Actual: HTTP 200 and a `missionProfileId`. The profile is stored with a nonsensical graph, `GetMissionProfile` reports it back, and `ReserveContact` happily accepts a contact against it, so a client that depends on the refusal (or on the profile being schedulable) takes the wrong branch.

The same hole exists on the update path: `update_mission_profile.rs` reuses the identical existence-only loop for the edges it carries, so an update can also leave a profile with an illegal graph.

## Evidence

Spec: `smithy_query(query="shape", shape="DataflowEdge")` — a 2..2 list of `ConfigArn` (source then destination); `smithy_query(query="operation", operation="CreateMissionProfile")` declares `InvalidParameterException` (client, HTTP 431) exactly for a parameter AWS will not accept. That AWS type-checks config references (rather than only resolving them) is already established for this service by the recorded trace behind issue `20261023T171627Z-groundstation-ground-station-config-missionprofile-fidelity-...` cause 4, where `uplinkEchoConfig.antennaUplinkConfigArn` pointing at an existing but wrong-type config is refused with `InvalidParameterException` and the message 'Invalid ARN provided. Config Type must be of type "antenna-uplink".'
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_mission_profile.rs` lines ~199-219 — `for edge in &input.dataflow_edges { referenced.extend(edge.iter().map(|arn| (arn, None))); }`, with the comment "so an edge endpoint is only required to exist"; `src/ops/update_mission_profile.rs` lines ~194-196 does the same. `state::resolves_config(arn, None)` then skips the type comparison.
Tests: `tests/all/scheduling_loop.rs::mission_profile_round_trips_every_member_it_was_created_with` and `::a_mission_profile_refuses_a_config_reference_that_resolves_to_nothing` only cover a legal edge and a nonexistent ARN; `tests/all/not_found_messages.rs::create_mission_profile_names_the_config_its_lookup_searched_for` covers the not-found text. No test asserts anything about an edge whose configs are of incompatible types, so adding the rule breaks no existing assertion.

## Steps to Reproduce

1. CreateConfig a `trackingConfig` (autotrack=PREFERRED) and keep its configArn.
2. CreateConfig an `antennaDownlinkConfig` and keep its configArn.
3. CreateMissionProfile(name=..., minimumViableContactDurationSeconds=60, trackingConfigArn=<tracking arn>, dataflowEdges=[[<tracking arn>, <tracking arn>]]) — the emulator answers 200 with a missionProfileId where AWS refuses the edge with InvalidParameterException (431).
4. CreateMissionProfile again with dataflowEdges=[[<antenna-downlink arn>, <antenna-downlink arn>]] (source and destination of the same capability type, no endpoint) — again 200.
5. GetMissionProfile on either id: the illegal `dataflowEdges` are reported back as stored; ReserveContact against the profile also succeeds.
6. Repeat step 3's body through PUT /missionprofile/{missionProfileId} (UpdateMissionProfile) on a legal profile — the illegal edge list replaces the legal one with 200.
