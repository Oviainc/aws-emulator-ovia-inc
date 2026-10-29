Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# ConfigureAgent drops fleetInstanceId and metadata: the agent state the operation exists to report is never stored

_Filed: 2026-10-25T21:25:21Z_
_Operation: ConfigureAgent | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ConfigureAgent` is documented as the call "Used by profiler agents to report their current state and to receive remote configuration updates". The handler implements only the second half: it records the ping instant (`latest_agent_orchestrated_at`) and answers with the group's orchestration decision, while the two input members that carry the reported state — `fleetInstanceId` and `metadata` — are deserialized into `ConfigureAgentInput` and then never read. `input.fleet_instance_id` and `input.metadata` appear nowhere in the handler body, and `crate::state::ProfilingGroup` has no field for either, so nothing in the store distinguishes one fleet instance's ping from another's and the reported compute-platform metadata is discarded.

Unlike every other input this crate declines to honour (GetProfile's `maxDepth`/`accept`, which are refused explicitly with a comment saying why), these two are dropped silently: no refusal, no comment in `configure_agent.rs` acknowledging it, and the handler's own header describes the operation as "Records that the agent of a profiling group pinged back" without mentioning the members it discards.

## Evidence

Model (smithy_query query="operation", operation="ConfigureAgent", and the member docs reproduced in the handler header at services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/configure_agent.rs:46-64):
```
//! Used by profiler agents to report their current state and to receive remote
//! configuration updates. ...
//! * `fleetInstanceId` (FleetInstanceId, len 1..255) — A universally unique identifier (UUID)
//!   for a profiling instance. For example, if the profiling instance is an Amazon EC2
//!   instance, it is the instance ID. ...
//! * `metadata` (Metadata) — Metadata captured about the compute platform the agent is
//!   running on. It includes information about sampling and reporting. ...
```

The whole handler body — src/ops/configure_agent.rs:194-233 — reads `input.profiling_group_name` only:
```
        group.latest_agent_orchestrated_at = Some(crate::state::now());

        Ok(ConfigureAgentOutput {
            configuration: AgentConfiguration {
                should_profile: group.profiling_enabled,
                period_in_seconds: PERIOD_IN_SECONDS,
                agent_parameters: Some(agent_parameters()),
            },
        })
```

The members exist on the built input — src/ops/types/configure_agent_types.rs:18-19:
```
    pub fleet_instance_id: Option<String>,
    pub metadata: Option<std::collections::HashMap<String, String>>,
```
and the store has nowhere to put them — src/state.rs:686-738 (`pub struct ProfilingGroup`) declares `name, arn, compute_platform, profiling_enabled, created_at, updated_at, tags, latest_agent_orchestrated_at, latest_agent_profile_reported_at, profiles, seen_profile_tokens, notification_channels, policy, findings_reports, anomaly_feedback` — no fleet-instance or agent-metadata field.

Contrast with the inputs this crate declines DELIBERATELY, which it refuses rather than drops — src/ops/get_profile.rs:262-273 ("maxDepth is not supported: this emulator returns the stored agent profile unmodified ...").

Test coverage: no test exercises either member's effect — the only mention anywhere in tests/ is tests/all/refusal_message_rules.rs:214-266, which asserts the `@length 1..255` refusal message for `fleetInstanceId`, i.e. a boundary refusal, not the handler's treatment of an accepted value.

## Steps to Reproduce

1. CreateProfilingGroup N.
2. POST /profilingGroups/N/configureAgent with body {"fleetInstanceId": "i-0123456789abcdef0", "metadata": {"COMPUTE_PLATFORM": "AWSLambda", "AGENT_ID": "agent-1"}} — 200.
3. Repeat with a different fleetInstanceId and different metadata — the stored record is identical except for the ping instant.
4. Read src/ops/configure_agent.rs:194-233: `input.fleet_instance_id` and `input.metadata` are never referenced, and src/state.rs:686-738 shows `ProfilingGroup` holds no field either could be stored in.
