Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CodeGuruProfiler emulator

_Filed: 2026-10-21T21:38:11Z_
_Operation: service-com-amazonaws-codeguruprofiler-codeguruprofiler | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 operations in `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and the service holds no state. Implement a FIRST, SMALL, stateful slice: the profiling-group lifecycle plus the agent profile round-trip (restJson1, signing name `codeguru-profiler`).

### Happy path to implement (in order)

1. **CreateProfilingGroup** — `POST /profilingGroups?clientToken=...` -> **201**. Input: `profilingGroupName` (required, len 1..255, `^[\w-]+$`), `clientToken` (required, idempotency token), `computePlatform` (`AWSLambda|Default`, default `Default`), `agentOrchestrationConfig{profilingEnabled: bool}`, `tags` map. Output: `profilingGroup` (`ProfilingGroupDescription`, @httpPayload) built from what was stored: `name`, `arn` (`arn:aws:codeguru-profiler:<region>:<account>:profilingGroup/<name>`), `computePlatform`, `agentOrchestrationConfig`, `createdAt`/`updatedAt` (real clock, ISO-8601), `profilingStatus`, `tags`. Creating a name that already exists must return `ConflictException` (409); a repeat with the same `clientToken` must be idempotent rather than conflict.
2. **DescribeProfilingGroup** — `GET /profilingGroups/{profilingGroupName}` -> **200**, returns the SAME `ProfilingGroupDescription` that was stored by Create/Update (not an echo of the request). Unknown name -> `ResourceNotFoundException` (404).
3. **ListProfilingGroups** — `GET /profilingGroups` -> **200**. Must return `profilingGroupNames` for every stored group, and populate `profilingGroups` (full descriptions) only when `includeDescription=true`. Honour `maxResults` (1..1000) and round-trip `nextToken` (len 1..64, `^[\w-]+$`) so paging over >maxResults groups terminates.
4. **ConfigureAgent** — `POST /profilingGroups/{profilingGroupName}/configureAgent` -> **200**. Input `fleetInstanceId`, `metadata`. Output `configuration` (`AgentConfiguration`, @httpPayload) with `shouldProfile` derived from the stored `agentOrchestrationConfig.profilingEnabled` and `periodInSeconds` (e.g. 300) plus `agentParameters`. The call must record `profilingStatus.latestAgentOrchestratedAt` so a later DescribeProfilingGroup reflects it. Unknown group -> 404.
5. **PostAgentProfile** — `POST /profilingGroups/{profilingGroupName}/agentProfile` -> **204**. Required `agentProfile` blob (@httpPayload) and `Content-Type` header; optional `profileToken` (idempotency). Store the blob + content type + receipt time against the group and set `profilingStatus.latestAgentProfileReportedAt`. Unknown group -> 404.
6. **GetProfile** — `GET /profilingGroups/{profilingGroupName}/profile` -> **200**. Returns the stored `profile` blob as @httpPayload with the `Content-Type` header it was stored under; must return bytes byte-identical to what PostAgentProfile uploaded. Group with no posted profile / unknown group -> `ResourceNotFoundException` (404).
7. **DeleteProfilingGroup** — `DELETE /profilingGroups/{profilingGroupName}` -> **204**. Removes the group and its profiles; a subsequent DescribeProfilingGroup/GetProfile must return 404, and the name must disappear from ListProfilingGroups.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no generating an ARN/timestamp that is never stored. `profilingStatus` timestamps in particular must come from recorded ConfigureAgent/PostAgentProfile calls, not be fabricated per response.

## State Model

```
CodeGuruProfilerState {
    // profilingGroupName -> group
    groups: Map<String, ProfilingGroup>,
    // clientToken -> profilingGroupName, for CreateProfilingGroup idempotency
    create_tokens: Map<String, String>,
}

ProfilingGroup {
    name: String,
    arn: String,                      // arn:aws:codeguru-profiler:<region>:<account>:profilingGroup/<name>
    compute_platform: String,         // "Default" | "AWSLambda"
    profiling_enabled: bool,          // agentOrchestrationConfig.profilingEnabled
    created_at: Timestamp,
    updated_at: Timestamp,
    tags: Map<String, String>,
    latest_agent_orchestrated_at: Option<Timestamp>,     // set by ConfigureAgent
    latest_agent_profile_reported_at: Option<Timestamp>, // set by PostAgentProfile
    profiles: Vec<AgentProfile>,      // newest last
    seen_profile_tokens: Set<String>, // PostAgentProfile idempotency
}

AgentProfile {
    bytes: Vec<u8>,
    content_type: String,
    received_at: Timestamp,
}
```

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): UpdateProfilingGroup, notification channels (Add/Get/RemoveNotificationChannel), policy ops (PutPermission/GetPolicy/RemovePermission), tagging ops (TagResource/UntagResource/ListTagsForResource), ListProfileTimes, BatchGetFrameMetricData, and the `/internal/*` findings/recommendations/feedback operations.

## Evidence

`services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/OPERATIONS.md` lists all 23 operations with routes, success statuses, input/output members and errors — CreateProfilingGroup (`POST /profilingGroups` -> 201, required `profilingGroupName`+`clientToken`, output `profilingGroup` as @httpPayload, ConflictException 409), DescribeProfilingGroup (`GET /profilingGroups/{profilingGroupName}` -> 200, ResourceNotFoundException 404), ListProfilingGroups (`GET /profilingGroups` -> 200, @paginated nextToken/maxResults, required `profilingGroupNames`, optional `profilingGroups`, `includeDescription`), ConfigureAgent (`POST .../configureAgent` -> 200, output `configuration` AgentConfiguration @httpPayload), PostAgentProfile (`POST .../agentProfile` -> 204, required `agentProfile` blob + `Content-Type`), GetProfile (`GET .../profile` -> 200, required `profile` blob @httpPayload + `contentType` header), DeleteProfilingGroup (`DELETE /profilingGroups/{profilingGroupName}` -> 204). Shape detail from smithy_query: `ProfilingGroupDescription{name, arn, computePlatform, agentOrchestrationConfig, createdAt, updatedAt, profilingStatus, tags}`, `AgentOrchestrationConfig{profilingEnabled required}`, `AgentConfiguration{shouldProfile required, periodInSeconds required, agentParameters}`, `ProfilingStatus{latestAgentProfileReportedAt, latestAggregatedProfile, latestAgentOrchestratedAt}`. `src/ops/create_profiling_group.rs:4` shows the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and line 176 the `Scaffolded::not_implemented("CreateProfilingGroup")` return — representative of all 23 op files. No tests exist for any of these.

## Steps to Reproduce

1. CreateProfilingGroup(profilingGroupName="demo-app", clientToken="tok-1", computePlatform="Default", agentOrchestrationConfig={profilingEnabled:true}, tags={env:"dev"}) -> expect 201 with profilingGroup.name="demo-app", a codeguru-profiler ARN, createdAt set, tags echoed from storage; today: 501.
2. CreateProfilingGroup again with the same name and a NEW clientToken -> expect 409 ConflictException.
3. DescribeProfilingGroup(profilingGroupName="demo-app") -> expect 200 with the exact description stored at create time (same arn, createdAt, computePlatform, tags).
4. ListProfilingGroups(includeDescription=true) -> expect "demo-app" in profilingGroupNames and its full description in profilingGroups; with maxResults=1 over two groups, expect a nextToken that pages to the second group and then no token.
5. ConfigureAgent(profilingGroupName="demo-app") -> expect 200 with configuration.shouldProfile=true (because profilingEnabled=true) and periodInSeconds set; then DescribeProfilingGroup shows profilingStatus.latestAgentOrchestratedAt.
6. PostAgentProfile(profilingGroupName="demo-app", agentProfile=<bytes>, Content-Type="application/json") -> expect 204; DescribeProfilingGroup then shows profilingStatus.latestAgentProfileReportedAt.
7. GetProfile(profilingGroupName="demo-app") -> expect 200 with the body byte-identical to the uploaded bytes and Content-Type matching what was posted.
8. DeleteProfilingGroup(profilingGroupName="demo-app") -> expect 204; DescribeProfilingGroup and GetProfile then return 404 ResourceNotFoundException and ListProfilingGroups no longer lists "demo-app".
