Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 44485854c238b5a692f0cba9ea9d9988a6365132

# A mission profile's dataflowEndpointConfigs may resolve into DIFFERENT dataflow endpoint groups, which the model says a contact must not do

_Filed: 2026-10-29T00:49:27Z_
_Operation: CreateMissionProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `44485854c238b5a692f0cba9ea9d9988a6365132`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateMissionProfile` (and `UpdateMissionProfile`) validate a `dataflowEdges` graph two ways: every `ConfigArn` must resolve to a config that exists (`resolves_config`), and the source/destination capability pair must be one that can form a dataflow (`dataflow_edge_fault`). Neither of them — and nothing in `ReserveContact` either — asks which DATAFLOW ENDPOINT GROUP each `dataflowEndpointConfig` end resolves into.

The model states that as a requirement, not a preference: `CreateDataflowEndpointGroup`'s own documentation says "When a contact uses multiple `DataflowEndpointConfig` objects, each `Config` must match a `DataflowEndpoint` in the same group." So a profile wiring two `dataflowEndpointConfig`s whose `dataflowEndpointName`s live in two different groups describes a contact AWS would refuse, and this crate accepts it, reserves a contact against it, and then reports it: `contact_dataflow_list` -> `dataflow_detail` -> `dataflow_end` resolves each name through `ComAmazonawsGroundstationGroundstationState::dataflow_endpoint`, whose own doc states the lookup is "over every group's endpoints" with no notion of which group the other end of the same contact landed in. `DescribeContact` / `DescribeContactVersion` then report a `dataflowList` whose two dataflow-endpoint ends carry `endpointDetails` taken from two separate groups — including two separate groups' agent carriers and health, since `reported_endpoint` resolves the carrying agent from the group that holds the endpoint.

The crate already quotes this exact sentence and implements only half of it (name uniqueness WITHIN one group), so the remaining half is the gap: the group a contact's dataflow endpoints are drawn from is never made to agree. `CreateMissionProfile` is where this crate already type-checks the graph, so it is the place the gate belongs (with the same gate on `UpdateMissionProfile`, which re-runs both existing checks on whichever members it carries).

NOTE this is NOT the already-fixed `CreateMissionProfile` validation gap (20261024T012157Z, "only checks that a dataflow edge's configs exist, never that the pair can form a dataflow"): that one is about the capability PAIR of one edge, this one is about which GROUP the endpoints of the whole graph come from.

## Evidence

Model — `smithy_query(query="doc", shape="CreateDataflowEndpointGroup")`:
```
"Creates a DataflowEndpoint group containing the specified list of  DataflowEndpoint objects."
"The name field in each endpoint is used in your mission profile  DataflowEndpointConfig to specify which endpoints to use during a contact."
"When a contact uses multiple DataflowEndpointConfig objects, each  Config must match a DataflowEndpoint in the same group."
```

The crate quotes that very sentence and derives only the within-group half from it — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:2617-2627`:
```
/// Refuse an endpoint of a group that carries no name, or one an endpoint already accepted
/// into the SAME group took.
///
/// "The `name` field in each endpoint is used in your mission profile
/// `DataflowEndpointConfig` to specify which endpoints to use during a contact. When a
/// contact uses multiple `DataflowEndpointConfig` objects, each `Config` must match a
/// `DataflowEndpoint` in the same group" (`smithy_query(query="operation",
/// operation="CreateDataflowEndpointGroup")`). That match is by name, so an endpoint
/// carrying none, and two endpoints of one group carrying the same one, describe nothing a
/// `dataflowEndpointConfig` can single out
```

The resolution that ignores the group — `src/state.rs:4631-4649`:
```
    /// A `dataflowEndpointConfig` names an endpoint by `dataflowEndpointName` rather than
    /// by the group it belongs to, so the lookup is over every group's endpoints
    /// ([`endpoint_name`] reads the name whichever member describes the endpoint). Groups
    /// are taken in id order, so two endpoints sharing a name resolve to the same one on
    /// every call whatever order the map iterates in.
    pub fn dataflow_endpoint(&self, name: &str) -> Option<EndpointDetails> {
        let mut group_ids: Vec<&String> = self.dataflow_endpoint_groups.keys().collect();
        group_ids.sort();
        group_ids
            .into_iter()
            .filter_map(|id| self.dataflow_endpoint_groups.get(id))
            .find_map(|group| {
                group
                    .endpoint_details
                    .iter()
                    .find(|details| endpoint_name(details) == Some(name))
                    .map(|details| self.reported_endpoint(details, group))
            })
    }
```

The two graph checks the create DOES make, with no third — `src/ops/create_mission_profile.rs:200-238`:
```
        // Every config a profile wires up must exist, and a reference whose member names
        // ONE config type must name a config of that type: ...
        for (arn, required_type) in referenced {
            if !guard.resolves_config(arn, required_type) {
...
        // An edge is one dataflow of a contact, not any two configs that happen to exist:
        // its source's capability must originate a dataflow and its destination's must
        // receive data from that source, or the graph describes no dataflow the service
        // could run and the profile is refused rather than stored (see
        // `state::dataflow_edge_fault`).
        for edge in &input.dataflow_edges {
            if let Some(refusal) = guard.dataflow_edge_fault(edge) {
```
and the same two, and only those two, on the update — `src/ops/update_mission_profile.rs:190-229`.

The reader that surfaces the cross-group wiring — `src/state.rs:4181-4183` (`dataflow_end`):
```
        Some(ConfigTypeData::dataflowEndpointConfig(endpoint_config)) => state
            .dataflow_endpoint(&endpoint_config.dataflow_endpoint_name)
            .map(ConfigDetails::endpointDetails),
```
reached from `src/ops/describe_contact.rs:203` and `src/ops/describe_contact_version.rs:262-263` through `contact_dataflow_list`.

Test coverage: the suite reads the same sentence as a NAMING rule only and pins the half it implements — `tests/all/dataflow_endpoint_addressing.rs:405-408`:
```
    // What the rule does NOT forbid: names that differ, however slightly — the selection
    // is by the name itself, so two names that are not equal are two endpoints — and the
    // SAME name in another group, the doc's rule being about one group ("each `Config`
    // must match a `DataflowEndpoint` in the same group").
```
That test is about two GROUPS reusing one endpoint NAME (which this finding does not touch); no test anywhere builds a mission profile whose two `dataflowEndpointConfig`s resolve into different groups, and no comment states a reason for accepting it. The existing graph-validation suites to extend are `tests/all/dataflow_edge_rules.rs` and `tests/all/config_reference_integrity.rs`.

## Steps to Reproduce

1. POST /dataflowEndpointGroup (CreateDataflowEndpointGroup) with one `endpointDetails[0].endpoint` named "alpha" (plus securityDetails) — group A.
2. POST /dataflowEndpointGroup again with one endpoint named "beta" — group B, a DIFFERENT group.
3. POST /config twice (CreateConfig) with `configData.dataflowEndpointConfig.dataflowEndpointName` = "alpha" and = "beta" respectively; plus an `antennaDownlinkConfig` and an `antennaUplinkConfig`.
4. POST /missionprofile (CreateMissionProfile) with `dataflowEdges` = [[antennaDownlinkArn, dfeAlphaArn], [dfeBetaArn, antennaUplinkArn]] — two `DataflowEndpointConfig`s, one matching an endpoint of group A and one of group B.
5. The create answers 200. Per the model ("each `Config` must match a `DataflowEndpoint` in the same group") this profile describes a contact that cannot exist.
6. POST /contact (ReserveContact) against that profile, then GET /contact/{contactId} (DescribeContact): `dataflowList` reports one dataflow whose destination `configDetails.endpointDetails` is group A's endpoint and another whose source `configDetails.endpointDetails` is group B's — two groups serving one contact, each with its own agent/health.
