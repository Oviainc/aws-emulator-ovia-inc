Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# Registering an agent never reaches the dataflow endpoint waiting for it: the endpoint stays INACTIVE/NO_REGISTERED_AGENT forever

_Filed: 2026-10-26T06:53:03Z_
_Operation: RegisterAgent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDataflowEndpointGroup` stamps an AWS Ground Station Agent endpoint with `agentStatus = INACTIVE` and `healthReasons = [NO_REGISTERED_AGENT]`, and its own doc comment explains that this is the state the endpoint holds *until an agent has registered*. `RegisterAgent` then registers an agent, and `UpdateAgentStatus` reports that agent `ACTIVE`, but neither ever revisits the endpoint: the group's stored `EndpointDetails` keeps the create-time values, so `GetDataflowEndpointGroup` reports `NO_REGISTERED_AGENT` and an `INACTIVE` agent for the rest of the store's life, contradicting the "until one has registered" the create wrote down. The clock-settled move in `settle_due_endpoints` only touches plain dataflow endpoints (`creating` -> `created`, `HEALTHY`); it has no branch for the agent kind, so nothing in the crate can ever move an agent endpoint off its initial health.

## Evidence

The rule the create states — `src/ops/create_dataflow_endpoint_group.rs:338-340`:
```
/// * an AWS Ground Station Agent endpoint's stream is carried by an agent the customer
///   registers with `RegisterAgent`, so until one has registered the endpoint reports
///   `NO_REGISTERED_AGENT`, an `INACTIVE` agent and no audit result, none having run.
```
and what it writes — `src/ops/create_dataflow_endpoint_group.rs:355-370`:
```
        crate::state::EndpointKind::Agent => {
            if let Some(endpoint) = stored.aws_ground_station_agent_endpoint.as_mut() {
                endpoint.agent_status = Some(crate::state::AGENT_INACTIVE.to_string());
                endpoint.audit_results = None;
            }
            ...
            stored.health_reasons =
                Some(vec![crate::state::REASON_NO_REGISTERED_AGENT.to_string()]);
```
Nothing else ever writes those fields: a grep over `src/` for `agent_status|REASON_NO_REGISTERED_AGENT|HEALTH_HEALTHY` matches only the three assignments above, the constants in `state.rs:2056-2070`, and `state.rs:3540` (`details.health_status = Some(HEALTH_HEALTHY.to_string());`) inside `settle_due_endpoints`, which is guarded on the plain-dataflow endpoint. `src/ops/register_agent.rs:226-249` builds the `Agent` record and inserts it plus its ARN index entry — it never looks at `guard.dataflow_endpoint_groups`; `src/ops/update_agent_status.rs:167-170` writes only `agent.aggregate_status`, `agent.component_statuses`, `agent.task_id`, `agent.last_updated`.

`src/ops/get_dataflow_endpoint_group.rs:182` then hands the frozen record straight back: `endpoints_details: Some(stored.endpoint_details.clone()),`.

Test coverage: `tests/all/dataflow_endpoint_groups.rs:756-770` only pins the case where no agent has registered ("An agent endpoint has no agent registered for it, and no clock makes one register"); no test registers an agent and re-reads the group, so the post-registration behaviour is unpinned.

## Steps to Reproduce

1. `CreateDataflowEndpointGroup` with one `endpointDetails[0].awsGroundStationAgentEndpoint` (name + egressAddress + ingressAddress) → 200.
2. `GetDataflowEndpointGroup` → `healthReasons: ["NO_REGISTERED_AGENT"]`, `awsGroundStationAgentEndpoint.agentStatus: "INACTIVE"` (correct so far: no agent has registered).
3. `RegisterAgent` with `agentDetails` + `discoveryData`, then `UpdateAgentStatus` for the returned `agentId` with `aggregateStatus.status = "ACTIVE"` → 200.
4. `GetDataflowEndpointGroup` again → still `healthReasons: ["NO_REGISTERED_AGENT"]` and `agentStatus: "INACTIVE"`, although an agent has registered and reported itself active.
