Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# A flow's NdiConfig.NdiDiscoveryServers is stored with any number of servers, although the model states the list holds at most three

_Filed: 2026-10-29T12:14:02Z_
_Operation: CreateFlow | Class: validation_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateFlowRequest$NdiConfig` is stored verbatim on the flow record (`ndi_config: input.ndi_config.clone()`, src/ops/create_flow.rs:667) and reported back by the flow projection. The only content check CreateFlow makes on it is the flow-size pairing (`ndi_is_enabled` + `ndi_flow_size_rejection`, src/ops/create_flow.rs:375-379); the nested `NdiDiscoveryServers` list is never examined.

The model states a maximum for that list twice — once on the member and once on the member's target shape — so a flow can be created (and, through UpdateFlow, re-stored) declaring ten, a hundred or an unbounded number of NDI discovery servers, in a record nothing evicts and that every later DescribeFlow reports.

This is the one request-sized collection of a flow that the crate stores without a count rule. Every sibling collection is bounded before it is stored — gateway networks against the documented "at least one network (up to four)" (src/ops/create_gateway.rs:192-201), media streams against `MAX_FLOW_MEDIA_STREAMS`, VPC interfaces against `MAX_FLOW_VPC_INTERFACES`, outputs against "you can create up to 50 outputs per flow", entitlements against `MAX_FLOW_ENTITLEMENTS`, and even a listing's `Filters` values, for the reason the crate states as "stored verbatim in a record nothing evicts, so the pairs and their count are bounded before they are stored".

(The separate open issue 20261028T203711Z covers a different member of the same structure — the service-generated `MachineName` — and is about a value the service must MINT, not about a bound on a request list.)

## Evidence

Model, the member (smithy_query(query="doc",shape="NdiConfig",member="NdiDiscoveryServers")):
  "A list of up to three NDI discovery server configurations. While not required by the API, this configuration is necessary for NDI functionality to work properly."
Model, the target shape (smithy_query(query="shape",shape="NdiDiscoveryServerConfig")):
  "Specifies the configuration settings for individual NDI® discovery servers. A maximum of 3 servers is allowed."

The handler stores the structure as it arrived — src/ops/create_flow.rs:666-668:
```
            source_monitoring_config: input.source_monitoring_config.clone(),
            ndi_config: input.ndi_config.clone(),
            encoding_config: input.encoding_config.clone(),
```
and UpdateFlow re-stores a replacement the same way — src/ops/update_flow.rs:294-297 and :396:
```
        let ndi_config = input
            .ndi_config
            .clone()
            .or_else(|| flow.ndi_config.clone());
...
        flow.ndi_config = ndi_config;
```
Nothing in the crate ever looks at the list: `grep -rn 'ndi_discovery|NdiDiscovery|MAX_NDI' src/` matches only the field declaration, src/ops/types/shared_types.rs:485:
```
    pub ndi_discovery_servers: Option<Vec<NdiDiscoveryServerConfig>>,
```
Contrast the sibling rule this crate applies when a model SENTENCE states a count — src/ops/create_gateway.rs:192-201:
```
        // "Creates a new gateway. The request must include at least one network (up to
        // four)."
        if input.networks.is_empty() || input.networks.len() > crate::state::MAX_GATEWAY_NETWORKS {
            return Err(bad_request(format!(
                "A create gateway request must include at least one network and at most {}; \
                 this one has {}.",
```
and the crate's stated reason for bounding a stored collection — src/ops/create_router_input.rs:250-252:
```
        // Tags are stored verbatim in a record nothing evicts, so the pairs and their count
        // are bounded before they are stored, through the same functions every other tag
        // write in this crate uses.
```
Test coverage: the NDI tests supply at most ONE discovery server (tests/all/update_flow.rs:245-255 — `"ndiDiscoveryServers": [ { "discoveryServerAddress": "198.51.100.40", "discoveryServerPort": 5959, "vpcInterfaceAdapter": "vpc-1" } ]`) or none at all (tests/all/flow_ndi_sizing.rs:119 — `"ndiConfig": { "ndiState": "ENABLED", "machineName": "studio-a" }`), so no test states that a longer list is acceptable and a count check leaves every existing test passing.

## Steps to Reproduce

1. POST /v1/flows with {"name":"ndi-flow","flowSize":"LARGE","source":{"name":"s1","protocol":"rtp"},"ndiConfig":{"ndiState":"ENABLED","ndiDiscoveryServers":[{"discoveryServerAddress":"198.51.100.1","vpcInterfaceAdapter":"a"},{"discoveryServerAddress":"198.51.100.2","vpcInterfaceAdapter":"b"},{"discoveryServerAddress":"198.51.100.3","vpcInterfaceAdapter":"c"},{"discoveryServerAddress":"198.51.100.4","vpcInterfaceAdapter":"d"}]}} — four servers, one more than the documented maximum.
2. Observe 201 and a response whose `flow.ndiConfig.ndiDiscoveryServers` carries all four entries.
3. GET /v1/flows/{FlowArn}: the stored flow still reports four discovery servers, where the model states "a list of up to three" / "a maximum of 3 servers is allowed".
4. Repeat with twenty entries, and with PUT /v1/flows/{FlowArn} carrying the same `ndiConfig`: both are accepted, so the stored list is bounded by nothing.
