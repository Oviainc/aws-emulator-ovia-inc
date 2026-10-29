Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-agent-workflow-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-agent-workflow-error-paths groundstation-reference-data-ground-stations-and-satellites
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|RegisterAgent|4xx->2xx|$.message
Issue-Key: groundstation|RegisterAgent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: groundstation|RegisterAgent|4xx->2xx|status
Issue-Key: groundstation|GetAgentConfiguration|4xx->4xx|$.message
Issue-Key: groundstation|GetAgentConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: groundstation|GetAgentConfiguration|4xx->4xx|status
Issue-Key: groundstation|GetAgentTaskResponseUrl|4xx->4xx|$.message
Issue-Key: groundstation|GetAgentTaskResponseUrl|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: groundstation|GetAgentTaskResponseUrl|4xx->4xx|status
Issue-Key: groundstation|UpdateAgentStatus|4xx->4xx|$.message
Issue-Key: groundstation|UpdateAgentStatus|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: groundstation|UpdateAgentStatus|4xx->4xx|status
Issue-Key: groundstation|GetSatellite|4xx->4xx|$.message
Issue-Key: groundstation|GetSatellite|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: groundstation|GetSatellite|4xx->4xx|status
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Unknown-identifier and missing-parameter refusals answer ResourceNotFoundException or succeed, where Ground Station answers InvalidParameterException

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterAgent

## Description
Five net-new wire causes, all in groundstation, all one fault: a request Ground Station refuses with InvalidParameterException (HTTP 431) is either accepted outright or refused with the wrong error class (ResourceNotFoundException, HTTP 434). The error code and the HTTP status are part of the contract, so each is a divergence; they are bundled because they share one service, and each must be closed separately.

Cause 1 of 5: RegisterAgent with an agentDetails that names neither reservedCpuCores nor agentCpuCores.
AWS: Smithy: RegisterAgent input agentDetails (AgentDetails) is @required; AgentDetails.reservedCpuCores and AgentDetails.agentCpuCores are both OPTIONAL AgentCpuCoresList members (reservedCpuCores carries the note 'This field should not be used. Use agentCpuCores instead.'), so neither is individually required but the service requires one of the pair. Modeled errors: InvalidParameterException (431), ResourceNotFoundException (434), DependencyException (531). Recorded behavior: the call is refused with HTTP 431, x-amzn-errortype InvalidParameterException and the message 'agentDetails.reservedCpuCores or agentDetails.agentCpuCores must be specified' — every word is fixed text naming the two member paths; nothing is interpolated from the request.
Emulator: accepts the registration and answers HTTP 200 with an agentId; no refusal at all, no message, no x-amzn-errortype header.
Why: an agent that registers with no CPU-core assignment is accepted into the emulator's state, so the whole downstream agent workflow proceeds on a configuration AWS never admits. A client (the Ground Station Agent itself, or a test of it) that expects its own malformed registration to be rejected sees success, and an operator never learns which of the two members they omitted.
Fix: the either-or presence check over agentDetails.reservedCpuCores / agentDetails.agentCpuCores belongs in the RegisterAgent handler's validation (services/service-com-amazonaws-groundstation-groundstation/src/ops/register_agent.rs), raising InvalidParameterException with that fixed message.
Traces: groundstation-agent-workflow-error-paths#task#1.

Cause 2 of 5: GetAgentConfiguration for an agentId that names no registered agent.
AWS: Smithy: GetAgentConfiguration.agentId is an @required @httpLabel Uuid (length 36..36, pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$); errors InvalidParameterException (431), ResourceNotFoundException (434). Recorded behavior: a well-formed uuid that names no agent is refused with HTTP 431, x-amzn-errortype InvalidParameterException and the fixed message 'Invalid AgentID.' — Ground Station treats an unknown agent id as an invalid PARAMETER, not as a missing resource, and the message interpolates nothing (the offending id does not appear in it).
Emulator: answers HTTP 434 with x-amzn-errortype ResourceNotFoundException and a message of its own, 'No agent found with id <agentId>', interpolating the requested uuid.
Why: the error class decides client behavior. SDK retry/classification layers and callers that branch on ResourceNotFoundException vs InvalidParameterException take the wrong branch — a caller probing for an agent's existence by catching ResourceNotFoundException sees a not-found where AWS raises a parameter error, and the HTTP status differs too, so even status-based handling diverges.
Fix: the agent-lookup miss path in the GetAgentConfiguration handler (src/ops/get_agent_configuration.rs) must raise InvalidParameterException with the fixed message 'Invalid AgentID.' instead of the shared ResourceNotFoundException helper.
Traces: groundstation-agent-workflow-error-paths#task#2.

Cause 3 of 5: GetAgentTaskResponseUrl for an agentId that names no registered agent.
AWS: Smithy: same agent-scoped shape and the same modeled error set as cause 2. Recorded behavior: HTTP 431, x-amzn-errortype InvalidParameterException, fixed message 'Invalid AgentID.' — identical treatment of an unknown agent id.
Emulator: HTTP 434, ResourceNotFoundException, 'No agent found with id <agentId>'.
Why: as in cause 2 — wrong error class and wrong HTTP status for the whole agent id-resolution family, so a client cannot distinguish a bad id from a missing resource the way AWS does.
Fix: the agent-lookup miss path in the GetAgentTaskResponseUrl handler (src/ops/get_agent_task_response_url.rs).
Traces: groundstation-agent-workflow-error-paths#task#3.

Cause 4 of 5: UpdateAgentStatus for an agentId that names no registered agent.
AWS: Smithy: UpdateAgentStatus.agentId is an @required @httpLabel Uuid with the same pattern; taskId, aggregateStatus and componentStatuses are @required; errors InvalidParameterException (431), ResourceNotFoundException (434). Recorded behavior: HTTP 431, x-amzn-errortype InvalidParameterException, fixed message 'Invalid AgentID.'.
Emulator: HTTP 434, ResourceNotFoundException, 'No agent found with id <agentId>'.
Why: an agent-status reporter that treats InvalidParameterException as fatal-and-re-register but ResourceNotFoundException as retryable (or the reverse) loops or gives up wrongly; the status code it sees is also wrong.
Fix: the agent-lookup miss path in the UpdateAgentStatus handler (src/ops/update_agent_status.rs).
Traces: groundstation-agent-workflow-error-paths#task#4.

Cause 5 of 5: GetSatellite for a satelliteId that names no satellite.
AWS: Smithy: GetSatellite.satelliteId is an @required @httpLabel Uuid (length 36..36, uuid pattern); errors InvalidParameterException (431), ResourceNotFoundException (434), DependencyException (531). Recorded behavior: a well-formed uuid naming no satellite is refused with HTTP 431, x-amzn-errortype InvalidParameterException and the fixed message 'Satellite does not exist.' — fixed text only, with no id interpolated.
Emulator: HTTP 434, x-amzn-errortype ResourceNotFoundException, message 'Satellite not found'.
Why: the reference-data read path disagrees with AWS on both the error class and the status for the commonest client mistake (a stale or mistyped satellite id); code that catches the modeled InvalidParameterException to report 'no such satellite' never fires.
Fix: the satellite-lookup miss path in the GetSatellite handler (src/ops/get_satellite.rs) must raise InvalidParameterException with the fixed message 'Satellite does not exist.'.
Traces: groundstation-reference-data-ground-stations-and-satellites#task#4.

## Evidence
Cause 1 — fixture groundstation-agent-workflow-error-paths step task#1, RegisterAgent:
  expected: status 431, x-amzn-errortype 'InvalidParameterException', message 'agentDetails.reservedCpuCores or agentDetails.agentCpuCores must be specified'
  actual:   status 200, no x-amzn-errortype header, no message member (request accepted)

Cause 2 — fixture groundstation-agent-workflow-error-paths step task#2, GetAgentConfiguration:
  expected: status 431, x-amzn-errortype 'InvalidParameterException', message 'Invalid AgentID.'
  actual:   status 434, x-amzn-errortype 'ResourceNotFoundException', message 'No agent found with id 998359be-517e-44a3-bbb8-dd8d78d08141'

Cause 3 — fixture groundstation-agent-workflow-error-paths step task#3, GetAgentTaskResponseUrl:
  expected: status 431, x-amzn-errortype 'InvalidParameterException', message 'Invalid AgentID.'
  actual:   status 434, x-amzn-errortype 'ResourceNotFoundException', message 'No agent found with id 998359be-517e-44a3-bbb8-dd8d78d08141'

Cause 4 — fixture groundstation-agent-workflow-error-paths step task#4, UpdateAgentStatus:
  expected: status 431, x-amzn-errortype 'InvalidParameterException', message 'Invalid AgentID.'
  actual:   status 434, x-amzn-errortype 'ResourceNotFoundException', message 'No agent found with id 998359be-517e-44a3-bbb8-dd8d78d08141'

Cause 5 — fixture groundstation-reference-data-ground-stations-and-satellites step task#4, GetSatellite:
  expected: status 431, x-amzn-errortype 'InvalidParameterException', message 'Satellite does not exist.'
  actual:   status 434, x-amzn-errortype 'ResourceNotFoundException', message 'Satellite not found'

In causes 2-5 the expected messages have NO variable parts: 'Invalid AgentID.' and 'Satellite does not exist.' are fixed text in full. The uuid 998359be-517e-44a3-bbb8-dd8d78d08141 appears only in the emulator's own message, where it is the agentId the request supplied — it is a per-run value, not part of any required template. Cause 1's expected message is likewise entirely fixed text, naming the two member paths agentDetails.reservedCpuCores and agentDetails.agentCpuCores.

## Steps to Reproduce
Cause 1: call RegisterAgent with discoveryData set and agentDetails={agentVersion, instanceId, instanceType, componentVersions} but with BOTH reservedCpuCores and agentCpuCores omitted; read the HTTP status, the x-amzn-errortype header and Error.Message. AWS refuses with 431/InvalidParameterException; the emulator returns 200 and an agentId.

Causes 2-4: without registering any agent, call GetAgentConfiguration(agentId=<any well-formed uuid, e.g. 998359be-517e-44a3-bbb8-dd8d78d08141>), then GetAgentTaskResponseUrl(agentId=<same>), then UpdateAgentStatus(agentId=<same>, taskId=<uuid>, aggregateStatus=..., componentStatuses=[...]); read status, x-amzn-errortype and Error.Message on each.

Cause 5: call GetSatellite(satelliteId=<a well-formed uuid absent from ListSatellites>) and read status, x-amzn-errortype and Error.Message.
