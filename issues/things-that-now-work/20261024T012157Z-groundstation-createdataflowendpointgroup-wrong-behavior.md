Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# CreateDataflowEndpointGroup refuses an AWS Ground Station Agent endpoint that carries no securityDetails

_Filed: 2026-10-24T01:21:57Z_
_Operation: CreateDataflowEndpointGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler requires `securityDetails` on EVERY item of `endpointDetails`, whatever kind of endpoint the item describes, and otherwise answers `InvalidParameterException` (HTTP 431) naming `securityDetails`.

Expected: `securityDetails` is the subnet/security-group/role triple Ground Station needs to place elastic network interfaces "to send streams to your instances" — that is the VPC plumbing of a classic `endpoint` (`DataflowEndpoint`) only. An AWS Ground Station Agent endpoint (`awsGroundStationAgentEndpoint`, `uplinkAwsGroundStationAgentEndpoint`, `downlinkAwsGroundStationAgentEndpoint`) carries its stream over the agent running on the customer's own instance and is created from its `name` + `egressAddress` + `ingressAddress` alone; the model marks `securityDetails` optional for exactly that reason. Such a request is accepted with HTTP 200 and the group is created.

Actual: HTTP 431 for every agent-endpoint group that omits `securityDetails`, so the whole documented AWS Ground Station Agent workflow (create agent dataflow endpoint group -> RegisterAgent -> contact) cannot be started against the emulator unless the caller invents VPC details the real API does not ask for.

## Evidence

Spec: `smithy_query(query="shape", shape="EndpointDetails")` — `securityDetails` is NOT `@required` (only `endpoint`-style VPC placement needs it), while `AwsGroundStationAgentEndpoint` marks `name`, `egressAddress` and `ingressAddress` `@required`, i.e. an agent endpoint is fully described without `securityDetails`; `smithy_query(query="doc", shape="SecurityDetails")` — "Endpoint security details including a list of subnets, a list of security groups and a role to connect streams to instances", which is the ENI placement Ground Station performs for a dataflow endpoint, not for an agent endpoint.
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_dataflow_endpoint_group.rs` lines ~204-213 — `if details.security_details.is_none() { return Err(invalid_parameter("securityDetails", ...)) }`, applied before the kind is used for anything else, so it fires for all four endpoint members.
Tests: `tests/all/dataflow_endpoint_groups.rs::an_endpoint_without_security_details_is_refused_whatever_kind_it_is` (lines ~425-467) asserts the current refusal for every kind, including the three agent members — i.e. the divergent behavior is pinned by a test that must be narrowed to the `endpoint` (dataflow) kind when this is fixed.

## Steps to Reproduce

1. POST /dataflowEndpointGroup with endpointDetails=[{"awsGroundStationAgentEndpoint": {"name": "agent-1", "egressAddress": {"socketAddress": {"name": "10.0.0.1", "port": 55888}, "mtu": 1500}, "ingressAddress": {"socketAddress": {"name": "10.0.0.2", "portRange": {"minimum": 55000, "maximum": 55099}}, "mtu": 1500}}}] and no securityDetails.
2. Observe HTTP 431 InvalidParameterException with parameterName "securityDetails", where AWS creates the group and returns 200 with a dataflowEndpointGroupId.
3. Add a securityDetails block to the same agent item and resend: 200 — confirming the refusal is driven solely by the missing member.
4. Repeat step 1 with "uplinkAwsGroundStationAgentEndpoint" and "downlinkAwsGroundStationAgentEndpoint": same 431.
