Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# An agent endpoint's IPv4 socket address is stored unparsed, so 256.0.0.1 is accepted where RegisterAgent refuses the identical value

_Filed: 2026-10-28T18:06:50Z_
_Operation: CreateDataflowEndpointGroupV2 | Class: validation_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RangedSocketAddress$name` targets the model's `IpV4Address`, whose pattern only holds a value to four dot-separated runs of up to three digits — so `256.0.0.1` and `52.0.0.999` clear the boundary validator. `RegisterAgent` knows this and parses the value itself, refusing such an address with `InvalidParameterException`. The dataflow endpoint creates do not: `crate::state::check_endpoint_ports` walks exactly the same socket addresses but only inspects their PORTS, and `check_ranged_connection` reads `socket_address.port_range` while never looking at `socket_address.name`. Nothing else in the crate parses an `IpV4Address` (`is_ipv4_address` exists only in `register_agent.rs`).

So `CreateDataflowEndpointGroupV2` — every one of whose endpoints is agent-carried, so every request of it carries an `agentIpAndPortAddress` — stores an ingress address no stream can be sent to, and `GetDataflowEndpointGroup` reports it back as the endpoint's address. The crate's own reason for the port check ("a dataflow endpoint's address is the TCP/UDP socket Ground Station connects a contact's stream to, so the only values it can carry are the transport's own port numbers") applies verbatim to the address beside the port. Both creates share the helper, so `CreateDataflowEndpointGroup` (`src/ops/create_dataflow_endpoint_group.rs:258-262`) accepts it too and one fix in `check_endpoint_ports` covers both.

## Evidence

The model shape the value is held to — `smithy_query(query="shape", shape="RangedSocketAddress")`:
```
"name": {"target": "IpV4Address", "required": true, "length": {"min": 7, "max": 16},
 "pattern": "^\\d{1,3}\\.\\d{1,3}\\.\\d{1,3}\\.\\d{1,3}$", "doc": "<p>IPv4 socket address.</p>"}
```

The sibling handler that parses it — `src/ops/register_agent.rs:169-193`:
```
        // The discovery data's addresses are IPv4 addresses (`IpAddressList`'s member is
        // `IpV4Address` — `smithy_query(query="shape", shape="IpAddressList")`), and that
        // shape's pattern only holds them to four dot-separated runs of up to three
        // digits: `256.0.0.1` satisfies the pattern the boundary validator applies and is
        // still not an address the agent can be reached on, so the value is parsed here.
...
            if let Some(address) = addresses.iter().find(|address| !is_ipv4_address(address)) {
                return Err(invalid_parameter(
```
with the helper at `src/ops/register_agent.rs:278-280`:
```
fn is_ipv4_address(value: &str) -> bool {
    value.parse::<::std::net::Ipv4Addr>().is_ok()
}
```

The shared gate this create uses instead, which stops at the port — `src/state.rs:2645-2652`:
```
/// Refuse an endpoint any of whose addresses names a port no socket can be addressed by.
///
/// Walks every port-valued member reachable from one `EndpointDetails`, whichever endpoint
/// member describes it: a `DataflowEndpoint`'s own `address`, an agent endpoint's
/// `egressAddress`/`ingressAddress`, and the connection pair an uplink or downlink agent
/// endpoint's `dataflowDetails` carries
```
and `src/state.rs:2718-2736`:
```
fn check_ranged_connection(
    path: &str,
    connection: &RangedConnectionDetails,
) -> Result<(), InvalidParameterException> {
    let range_path = format!("{path}.socketAddress.portRange");
    let range = &connection.socket_address.port_range;
    check_port(&format!("{range_path}.minimum"), range.minimum)?;
    check_port(&format!("{range_path}.maximum"), range.maximum)?;
```
(`connection.socket_address.name` is never read.)

The V2 handler's whole per-endpoint gate — `src/ops/create_dataflow_endpoint_group_v2.rs:184-192`:
```
            let path = format!("endpoints[{index}]");
            crate::state::check_endpoint_ports(&path, details)
                .map_err(CreateDataflowEndpointGroupV2ErrorModeled::InvalidParameterException)?;
...
            crate::state::claim_endpoint_name(&path, member, details, &mut names)
```

Test coverage: the sibling rule is pinned with its reason in `tests/all/agent_lifecycle.rs:661-684` ("Each row satisfies `IpV4Address`'s pattern … and is still not an address: the octet one past the published 0..255 bound", rows `256.0.0.1`, `10.0.0.256`, `52.0.0.999`, each expecting 431). No test in `tests/all/dataflow_endpoint_groups_v2.rs`, `dataflow_endpoint_addressing.rs` or `dataflow_endpoint_groups.rs` sends a non-address IP, so nothing pins the acceptance.

## Steps to Reproduce

1. `POST /dataflowEndpointGroupV2` with `endpoints[0].downlinkAwsGroundStationAgentEndpoint` = `{"name":"downlink-1","dataflowDetails":{"agentConnectionDetails":{"egressAddressAndPort":{"socketAddress":{"name":"10.0.0.1","port":55000}},"agentIpAndPortAddress":{"socketAddress":{"name":"256.0.0.1","portRange":{"minimum":42000,"maximum":42010}}}}}}`.
2. The call answers 200 and `GET /dataflowEndpointGroup/{id}` reports `agentIpAndPortAddress.socketAddress.name = "256.0.0.1"`.
3. `POST /agent` (RegisterAgent) with `discoveryData.publicIpAddresses = ["256.0.0.1"]` → 431 `InvalidParameterException`, `parameterName = discoveryData.publicIpAddresses`.
4. The same string is refused as an agent's own address and accepted as the address Ground Station is told to connect that agent's stream to.
