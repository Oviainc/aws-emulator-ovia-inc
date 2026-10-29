Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: agent-registry-control.approval-configuration-update-affects-only-future-submissions agent-registry-control.auto-approval-rule-skips-manual-review agent-registry-control.delete-registry-blocked-while-records-exist agent-registry-control.list-registry-records-filter-by-status-excludes-other-statuses agent-registry-control.update-registry-record-status-enforces-workflow-transitions
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|CreateRegistry|AmbiguousService
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# Every AgentRegistryControl request is rejected with AmbiguousService: two services claim the shared signing name 'agent-registry'

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRegistry

## Description
Cause 1 of 1: the whole Agent Registry Control surface is unreachable because its request-claim predicate keys on the signing name, which it shares with the Agent Registry data plane, so the router sees two claimants and refuses every request rather than dispatching it.

AWS: Agent Registry Control and Agent Registry are two distinct services that deliberately share a SigV4 signing scope and ARN namespace but are addressed at different endpoints. Smithy (services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/smithy-model/com-amazonaws-agentregistrycontrol-agentregistrycontrol.json): service AgentRegistryControl, version 2025-12-01, protocol restJson1, aws.api#service { sdkId: "Agent Registry Control", endpointPrefix: "agent-registry-control", arnNamespace: "agent-registry", cloudTrailEventSource: "agent-registry.amazonaws.com" }, sigv4 name "agent-registry". Smithy (services/service-com-amazonaws-agentregistry-agentregistry/smithy-model/com-amazonaws-agentregistry-agentregistry.json): service AgentRegistry, version 2025-12-01, protocol restJson1, aws.api#service { sdkId: "Agent Registry", endpointPrefix: "agent-registry", arnNamespace: "agent-registry" }, sigv4 name "agent-registry". The signing name and ARN namespace are therefore NOT a unique service key for this pair; the endpointPrefix is. On AWS, a signed CreateRegistry request addressed to the control-plane endpoint is served by AgentRegistryControl and returns 200 with the modeled CreateRegistry output (registryArn, status); there is no AmbiguousService condition and no such error is modeled by either service (neither lists it; the control plane's common errors are AccessDeniedException, InternalServerException, ThrottlingException, ValidationException).

Emulator: every call into the service is refused before any handler runs. CreateRegistry returns the non-modeled error code `AmbiguousService` with message `2 emulated services all claim requests for 'agent-registry': agent-registry, agent-registry; the request is rejected rather than served by an arbitrary one`, and botocore retries it to exhaustion (`reached max retries: 3`). In that message the fixed text is `N emulated services all claim requests for '<key>': <names>; the request is rejected rather than served by an arbitrary one`; the variable parts are the resolved route key and the claimants' names — here both render as the shared signing name `agent-registry`, which is itself the symptom: the two claimants are indistinguishable under the key being matched. Because the refusal happens at claim time, no operation of the service is reachable — CreateRegistry, GetRegistry, UpdateRegistry, DeleteRegistry, CreateRegistryRecord, GetRegistryRecord, ListRegistryRecords, SubmitRegistryRecordForApproval, UpdateRegistryRecordStatus and DeleteRegistryRecord all fail identically, which is why all five behavioral checks die on their first call.

Why: a client gets an unmodeled error code its SDK cannot classify, on a request AWS answers 200. `AmbiguousService` is in no error union, so no catch of the service's modeled errors matches it; worse, it is retried as a transient fault, so each call costs the full retry budget before surfacing. No control-plane workflow is exercisable at all: registries cannot be created, records cannot be submitted or approved, and nothing downstream of them can be tested.

Fix: the claim seam of these two services — the `can_handle` predicate each service crate owns. Both crates still carry the generator's unedited default, which resolves a route key and compares it to `self.entry.identity().signing_name()`; for a signing name shared by several services that default is documented as the case to edit, and neither was. The control plane must claim only requests addressed to its own `endpointPrefix` (`agent-registry-control`) and the data plane only those addressed to `agent-registry`, so exactly one service claims any given request and the router never reaches its ambiguity refusal. Resolving it in only one of the two crates is not enough: both predicates currently answer true for both endpoints.

Traces: all five agent-registry-control checks below, each failing on its first CreateRegistry call with the AmbiguousService refusal — approval-configuration-update-affects-only-future-submissions, auto-approval-rule-skips-manual-review, delete-registry-blocked-while-records-exist, list-registry-records-filter-by-status-excludes-other-statuses, update-registry-record-status-enforces-workflow-transitions. File citations: services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/src/service.rs lines 76-84 and services/service-com-amazonaws-agentregistry-agentregistry/src/service.rs lines 76-84.

## Evidence
Expected (AWS): CreateRegistry against the Agent Registry Control endpoint is routed to AgentRegistryControl and answers 200 with the modeled output (registryArn, status), after which GetRegistry reaches READY. No AmbiguousService error exists in either service's model.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateRegistry operation (reached max retries: 3): 2 emulated services all claim requests for 'agent-registry': agent-registry, agent-registry; the request is rejected rather than served by an arbitrary one

Root cause in the checkout: both service crates keep the generated default claim predicate
    match smithy_server_runtime::resolve_route_key(parts) {
        Some(key) => key == self.entry.identity().signing_name(),
        None => false,
    }
and both identities carry sigv4 name "agent-registry" (shared), while their endpointPrefix values differ ("agent-registry-control" vs "agent-registry"). Matching on the shared signing name makes both services claim every request to either endpoint, so the router's two-claimant refusal fires on every operation of the service.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Build a botocore client for the agent-registry-control service against the emulator endpoint.
3. Call CreateRegistry(name="<any-unique-name>").
4. Observe the call is refused with error code AmbiguousService and the two-claimant message naming 'agent-registry' twice, after exhausting the retry budget, instead of returning 200 with registryArn/status.
5. Any other control-plane call (GetRegistry, CreateRegistryRecord, ListRegistryRecords, SubmitRegistryRecordForApproval, UpdateRegistryRecordStatus, DeleteRegistry) fails identically, confirming the refusal is at service-claim time and not in any one handler.
