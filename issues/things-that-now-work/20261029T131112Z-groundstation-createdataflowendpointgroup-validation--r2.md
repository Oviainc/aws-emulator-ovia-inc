Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# An agent endpoint's egress MTU is stored unbounded, where the ingress MTU beside it in the same connection pair is held to 1400..1500

_Filed: 2026-10-29T13:11:12Z_
_Operation: CreateDataflowEndpointGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `mtu` member of this service's endpoint shapes is "Maximum transmission unit (MTU) size in bytes of a dataflow endpoint", and the model bounds TWO of the three to `range 1400..1500` — `DataflowEndpoint$mtu` and `RangedConnectionDetails$mtu` — so the boundary validator refuses an out-of-range value there before a handler runs. The third, `ConnectionDetails$mtu`, carries the identical documentation sentence and NO `range`, so nothing upstream bounds it.

`ConnectionDetails` is the EGRESS half of every agent connection pair this service stores: `AwsGroundStationAgentEndpoint$egressAddress`, `UplinkAgentEndpoint...agentConnectionDetails$ingressAddressAndPort` and the downlink `egressAddressAndPort`. `crate::state::check_endpoint_ports` already walks every one of those structs — it is handed the whole `ConnectionDetails` — and `check_connection` reads only `connection.socket_address.port`, never the `mtu` sitting beside it. So `CreateDataflowEndpointGroup` (and `CreateDataflowEndpointGroupV2`, which shares the helper) accepts and stores `egressAddress.mtu = 9000`, `0` or `-1` with HTTP 200, and `GetDataflowEndpointGroup` reports it back — while the IDENTICAL value on `ingressAddress.mtu` of the SAME endpoint is refused, because that member's shape carries the range. One endpoint, one documented member, two different answers.

This is exactly the condition the crate's own port rule was written for, stated on `PORT_MIN`: a plain modelled `Integer` with no `range` that must still be bounded, so the handler bounds it. The MTU bound is not invented here — the model states it (1400..1500) on the two sibling members of the same closure. One fix in `check_connection` covers both creates, as the sibling port check already does.

## Evidence

The model, `smithy_query(query="shape", shape="ConnectionDetails,RangedConnectionDetails,DataflowEndpoint")` — the same sentence, bounded on two members and unbounded on the third:
```
"ConnectionDetails": { ... "mtu": {"target": "Integer", "doc": "<p>Maximum transmission unit (MTU) size in bytes of a dataflow endpoint.</p>"} }
"RangedConnectionDetails": { ... "mtu": {"target": "Integer", "range": {"min": 1400, "max": 1500}, "doc": "<p>Maximum transmission unit (MTU) size in bytes of a dataflow endpoint.</p>"} }
"DataflowEndpoint": { ... "mtu": {"target": "Integer", "range": {"min": 1400, "max": 1500}, "doc": "<p>Maximum transmission unit (MTU) size in bytes of a dataflow endpoint.</p>"} }
```

Both halves of the pair carry an `mtu` in this crate's own stored types — `src/ops/types/shared_types.rs:301-313`:
```
#[smithy_macros::smithy(shape = "com.amazonaws.groundstation#ConnectionDetails")]
pub struct ConnectionDetails {
    pub socket_address: SocketAddress,
    pub mtu: Option<i32>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.groundstation#RangedConnectionDetails")]
pub struct RangedConnectionDetails {
    pub socket_address: RangedSocketAddress,
    pub mtu: Option<i32>,
}
```

The crate's STATED reason for bounding an unbounded modelled Integer in a handler — `src/state.rs:2741-2752`:
```
/// A dataflow endpoint's address is the TCP/UDP socket Ground Station connects a contact's
/// stream to, so the only values it can carry are the transport's own port numbers: ...
/// `SocketAddress$port` and the two bounds of `IntegerRange` are plain modelled `Integer`s
/// with no `range` of their own (...), so nothing upstream of a handler bounds them.
pub const PORT_MIN: i32 = 1;
pub const PORT_MAX: i32 = 65535;
```

The walker that is handed the struct and skips the member — `src/state.rs:2754-2761` and `src/state.rs:2813-2822`:
```
/// Refuse an endpoint any of whose addresses names a port no socket can be addressed by.
///
/// Walks every port-valued member reachable from one `EndpointDetails`, whichever endpoint
/// member describes it: a `DataflowEndpoint`'s own `address`, an agent endpoint's
/// `egressAddress`/`ingressAddress`, and the connection pair an uplink or downlink agent
/// endpoint's `dataflowDetails` carries
...
/// Refuse a `ConnectionDetails` whose socket address names an unusable port.
fn check_connection(
    path: &str,
    connection: &ConnectionDetails,
) -> Result<(), InvalidParameterException> {
    check_port(
        &format!("{path}.socketAddress.port"),
        connection.socket_address.port,
    )
}
```
(`connection.mtu` is never read — `grep -n mtu src/` matches only the three type declarations.)

The handler's whole per-endpoint gate — `src/ops/create_dataflow_endpoint_group.rs:253-262`:
```
        // Every address the endpoint carries is a socket Ground Station connects a
        // stream to, so each port it names has to be one a transport can address, and
        // the name it is selected by has to be one no other endpoint of this group
        // took: both rules belong to every create of a dataflow endpoint group, so both
        // are held in `crate::state`.
        let path = format!("endpointDetails[{index}]");
        crate::state::check_endpoint_ports(&path, details)
            .map_err(CreateDataflowEndpointGroupErrorModeled::InvalidParameterException)?;
        crate::state::claim_endpoint_name(&path, member, details, &mut names)
```

Test coverage: every MTU any test sends is already inside the bound — `tests/all/dataflow_endpoint_groups.rs:106,121,128,143,150,169,173` and `tests/all/dataflow_endpoint_groups_v2.rs:76,83,101,105` use only `"mtu": 1500` and `"mtu": 1400`, and `tests/all/dataflow_endpoint_group_listing.rs:90`, `tests/all/resource_tagging.rs:229`, `tests/all/tag_rules.rs:120` use `1500`. No test sends an out-of-range MTU, so nothing pins the acceptance and adding the rule breaks no existing assertion. This is distinct from the FIXED `CreateDataflowEndpointGroup/validation` issue, which covered `SocketAddress.port` and endpoint-name presence/uniqueness only.

## Steps to Reproduce

1. POST /dataflowEndpointGroup with endpointDetails=[{"securityDetails": {...}, "endpoint": null, "awsGroundStationAgentEndpoint": {"name": "agent-1", "egressAddress": {"socketAddress": {"name": "10.0.0.1", "port": 55000}, "mtu": 9000}, "ingressAddress": {"socketAddress": {"name": "10.0.0.2", "portRange": {"minimum": 42000, "maximum": 42010}}, "mtu": 1500}}] — observe HTTP 200.
2. GET /dataflowEndpointGroup/{id} — the endpoint reports `egressAddress.mtu = 9000` back; repeat step 1 with `0` and `-1`, same 200.
3. Send the same request with the 9000 moved to `ingressAddress.mtu` (and 1500 on the egress) — refused before the handler runs, because `RangedConnectionDetails$mtu` carries `range 1400..1500`.
4. POST /dataflowEndpointGroupV2 with endpoints=[{"downlinkAwsGroundStationAgentEndpoint": {"name": "dl-1", "dataflowDetails": {"agentConnectionDetails": {"egressAddressAndPort": {"socketAddress": {"name": "10.0.0.1", "port": 55000}, "mtu": 9000}, "agentIpAndPortAddress": {"socketAddress": {"name": "10.0.0.2", "portRange": {"minimum": 42000, "maximum": 42010}}, "mtu": 1400}}}}] — same 200 through the shared helper.
