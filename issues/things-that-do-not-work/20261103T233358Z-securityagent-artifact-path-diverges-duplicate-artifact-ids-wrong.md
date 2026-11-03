Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-artifact-batch-and-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-artifact-batch-and-error-paths securityagent-artifact-lifecycle
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|AddArtifact|2xx->2xx|$.artifactId
Issue-Key: securityagent|GetArtifact|4xx->4xx|$.message
Issue-Key: securityagent|AddArtifact|4xx->4xx|$.message
Issue-Key: securityagent|GetArtifact|4xx->2xx|$.message
Issue-Key: securityagent|GetArtifact|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: securityagent|GetArtifact|4xx->2xx|status
Issue-Commit: 0f7523550f25498fe15e51e7c9685e91bd6d79be

# securityagent artifact path diverges: duplicate artifact ids, wrong not-found messages, deleted artifact still readable

> _Note: this issue was identified against the code base at commit `0f7523550f25498fe15e51e7c9685e91bd6d79be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddArtifact

## Description
FOUR net-new causes, one shared area: the securityagent artifact sub-API (AddArtifact / GetArtifact and the artifact record's lifecycle) answers with the right HTTP status and shape in three of them, but the artifact IDENTITY it mints is a non-unique constant, both artifact not-found messages carry the wrong text (and the two paths' texts are effectively swapped against AWS's), and a deleted artifact remains readable. All four must be closed before submitting.

Cause 1 of 4: AddArtifact returns a fixed, non-unique artifactId (fixture securityagent-artifact-batch-and-error-paths, step task#2; cluster also covers securityagent-artifact-batch-and-error-paths#task#3 and securityagent-artifact-lifecycle#task#2).
AWS: Smithy: AddArtifact(agentSpaceId, artifactContent, artifactType, fileName all @required) -> AddArtifactOutput.artifactId @required, target ArtifactId; http POST /AddArtifact, code 201. AWS mints a fresh, opaque artifact identifier for each artifact added, so two AddArtifact calls - in the same agent space or in different ones - never return the same artifactId, and the id is the handle GetArtifact/DeleteArtifact use to address that one artifact. (Any particular id format is recorded behavior only; the requirement is per-artifact uniqueness.)
Emulator: returns the correct 201 but with $.artifactId = the constant '00000000000000000000000000000001' for EVERY artifact; the replay expected the id captured from this call ({{artifact_id_1}}) and the same literal came back for three separate AddArtifact steps across two fixtures (task#2 and task#3 of the error-paths fixture, and the lifecycle fixture's task#2), i.e. even two artifacts added back-to-back in one workflow share one id.
Why: a client that adds two artifacts gets one identifier for both, so the second add silently aliases the first: GetArtifact/DeleteArtifact address the wrong artifact, any map keyed by artifactId collapses the set, and a workflow that adds-then-deletes one artifact affects another. It also collides with the agent-space / pack / target-domain ids the emulator mints, so ids are not even distinct across resource types.
Fix: the AddArtifact handler's identifier allocation in the securityagent state layer - allocate the artifact id per stored artifact record from the shared unique-id generator and echo that value into $.artifactId, instead of serializing a constant. Related open issue 20261030T004326Z-securityagent-created-resource-identifiers-and-verification-tokens-are-fixed.md describes the same defect class for CreateAgentSpace / CreateSecurityRequirementPack / CreateTargetDomain; it does NOT cover AddArtifact (its Issue-Key list has no AddArtifact|artifactId entry and the replay did not mark this cluster as owned by it), so the artifact allocator must be fixed here.
Traces: securityagent-artifact-batch-and-error-paths#task#2, securityagent-artifact-batch-and-error-paths#task#3, securityagent-artifact-lifecycle#task#2.

Cause 2 of 4: GetArtifact's ResourceNotFoundException message is a generic sentence instead of AWS's id-bearing text (fixture securityagent-artifact-batch-and-error-paths, step task#5).
AWS: Smithy: GetArtifact(agentSpaceId, artifactId @required) models ResourceNotFoundException (client, HTTP 404). Recorded behavior for an artifactId that names no artifact: HTTP 404, x-amzn-errortype ResourceNotFoundException, message 'No artifact was found for id: <artifactId>'. Fixed text = 'No artifact was found for id: '; the only variable part is the artifactId copied verbatim from the request (in this recording 'a-00000000-0000-0000-0000-000000000000', which fills the template and is not part of it).
Emulator: status 404 and the error type are right, but the message is the constant 'The specified artifact does not exist.' - AWS's fixed text is absent and the requested artifactId is never echoed.
Why: a client cannot tell WHICH artifact id was rejected from the emulator's message, so batch/multi-artifact callers that log or parse the id out of the 404 lose it, and any assertion on AWS's documented wording fails even though the status and code are correct.
Fix: the GetArtifact handler's not-found error construction in the securityagent error/message layer - build the message from the operation's own fixed text with the request's artifactId interpolated.
Traces: securityagent-artifact-batch-and-error-paths#task#5.

Cause 3 of 4: AddArtifact's agent-space-not-found message uses the agent-instance wording and omits the id (fixture securityagent-artifact-batch-and-error-paths, step task#6).
AWS: Smithy: AddArtifact models ResourceNotFoundException (client, HTTP 404) and agentSpaceId is @required. Recorded behavior when agentSpaceId names no agent space: HTTP 404, ResourceNotFoundException, message 'AgentSpace under the provided id: <agentSpaceId> does not exist'. Fixed text = 'AgentSpace under the provided id: ' + ' does not exist'; the variable part is the agentSpaceId taken from the request (here 'as-00000000-0000-0000-0000-000000000000').
Emulator: status 404 and error type are right, but the message is the constant 'The specified agent instance does not exist.' - different fixed text, a different resource noun ('agent instance' rather than 'AgentSpace'), and the offending agentSpaceId is dropped. Note the emulator uses this same generic sentence on the agent-space path while using the generic artifact sentence on the artifact path (Cause 2), so both artifact-API not-found messages are emulator-local constants rather than per-operation AWS text.
Why: callers that distinguish 'the agent space does not exist' from 'the artifact does not exist' by message text, or that recover the rejected id from the message, get neither; the 404 is trivially distinguishable from real AWS. Related open issue 20261030T135909Z-securityagent-updateagentspace-responses-omit-agent-instance-identity-fields-and.md owns the UpdateAgentSpace not-found text and 20261029T204635Z-securityagent-error-message-text-diverges-from-aws-for.md owns AddArtifact's enum ValidationException text and CreatePrivateConnection's 404 text; neither covers AddArtifact's ResourceNotFoundException for a missing agent space, and the replay did not mark this cluster as owned.
Fix: the AddArtifact handler's agent-space lookup failure path - raise ResourceNotFoundException with AWS's AgentSpace fixed text and the request's agentSpaceId interpolated, rather than a shared generic sentence.
Traces: securityagent-artifact-batch-and-error-paths#task#6.

Cause 4 of 4: an artifact that has been deleted is still returned by GetArtifact with 200 instead of 404 (fixture securityagent-artifact-batch-and-error-paths, step teardown#9; cluster also covers teardown#11).
AWS: Smithy: DeleteArtifact(agentSpaceId, artifactId @required) -> empty output, http POST /DeleteArtifact code 200; GetArtifact models ResourceNotFoundException (client, HTTP 404). After an artifact is deleted, AWS no longer serves it: GetArtifact for that artifactId returns HTTP 404 with x-amzn-errortype ResourceNotFoundException and message 'No artifact was found for id: <artifactId>' (same template as Cause 2 - fixed text plus the artifactId from the request).
Emulator: GetArtifact for the deleted artifact answers HTTP 200 with a full artifact body: status expected 404 got 200, $.message missing, and the $headers.x-amzn-errortype: ResourceNotFoundException header absent. The deletion left no trace in state, so the read path still resolves the record. Hypothesis: this is downstream of DeleteArtifact being an unimplemented scaffold (open issue 20261103T202512Z-securityagent-deleteartifact-stub.md owns implementing the operation itself); that issue covers the DeleteArtifact call, not GetArtifact's post-delete read, which the replay reports as an unowned mismatch - once deletion is wired, the artifact store must drop the record so GetArtifact 404s.
Why: a client cannot observe deletion at all - cleanup appears to succeed while the artifact stays readable, so idempotency checks, 'deleted means gone' assertions and any consumer that treats a 200 as proof of existence all operate on resurrected state.
Fix: the artifact state layer plus the GetArtifact read path - deleting an artifact must remove (or tombstone) the record for its agent space, and GetArtifact must raise ResourceNotFoundException with the id-bearing message of Cause 2 when the record is absent.
Traces: securityagent-artifact-batch-and-error-paths#teardown#9, securityagent-artifact-batch-and-error-paths#teardown#11.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 105 steps, 43 passed, 62 mismatched, score 41%). Four net-new divergence clusters, all service=securityagent, none carrying stub_path or owned_by:

1) securityagent-artifact-batch-and-error-paths#task#2 AddArtifact, 201 -> 201 (cluster also covers securityagent-artifact-batch-and-error-paths#task#3 and securityagent-artifact-lifecycle#task#2):
   $.artifactId: value_mismatch (expected '{{artifact_id_1}}', got '00000000000000000000000000000001')
   The same literal came back for three distinct AddArtifact calls in two fixtures, which is what shows the value is a constant rather than a per-run id.

2) securityagent-artifact-batch-and-error-paths#task#5 GetArtifact, 404 -> 404:
   $.message: value_mismatch (expected 'No artifact was found for id: a-00000000-0000-0000-0000-000000000000', got 'The specified artifact does not exist.')
   Status and x-amzn-errortype match; only the message text diverges. Fixed text = 'No artifact was found for id: '; variable = the request's artifactId.

3) securityagent-artifact-batch-and-error-paths#task#6 AddArtifact, 404 -> 404:
   $.message: value_mismatch (expected 'AgentSpace under the provided id: as-00000000-0000-0000-0000-000000000000 does not exist', got 'The specified agent instance does not exist.')
   Status and error code match; fixed text = 'AgentSpace under the provided id: ' ... ' does not exist'; variable = the request's agentSpaceId.

4) securityagent-artifact-batch-and-error-paths#teardown#9 GetArtifact, expected 404 -> got 200 (cluster also covers teardown#11):
   status: expected 404, got 200
   $.message: missing (expected 'No artifact was found for id: 00000000000000000000000000000001', got None)
   $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)

Model basis (smithy_query on services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json, restJson1): AddArtifact -> AddArtifactOutput.artifactId @required (ArtifactId), http POST /AddArtifact code 201, errors include ResourceNotFoundException (client, 404); GetArtifact(agentSpaceId, artifactId both @required) -> output with agentSpaceId/artifactId/artifact/fileName/updatedAt all @required, errors include ResourceNotFoundException (client, 404); DeleteArtifact(agentSpaceId, artifactId @required) -> empty output, code 200, errors include ResourceNotFoundException (client, 404). The id and message texts quoted above are from the recording: the fixed portions are the requirement, the specific ids and the '00000000000000000000000000000001' literal are per-run values filling the templates.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1) and inspect these steps of fixture securityagent-artifact-batch-and-error-paths: task#2 (and task#3), task#5, task#6, teardown#9 (and teardown#11); plus securityagent-artifact-lifecycle#task#2 for cause 1.
3. Equivalently by hand against the emulator's data listener (restJson1, POST with sigv4):
   a. Cause 1: POST /CreateAgentSpace, then POST /AddArtifact {agentSpaceId, artifactContent, artifactType:'TXT', fileName:'a.txt'} twice with different file names -> observe both responses return the identical artifactId ('00000000000000000000000000000001'); AWS returns two distinct ids.
   b. Cause 2: POST /GetArtifact {agentSpaceId:<existing>, artifactId:'a-00000000-0000-0000-0000-000000000000'} -> 404 ResourceNotFoundException with message 'The specified artifact does not exist.'; expected 'No artifact was found for id: a-00000000-0000-0000-0000-000000000000'.
   c. Cause 3: POST /AddArtifact {agentSpaceId:'as-00000000-0000-0000-0000-000000000000', ...} -> 404 ResourceNotFoundException with message 'The specified agent instance does not exist.'; expected 'AgentSpace under the provided id: as-00000000-0000-0000-0000-000000000000 does not exist'.
   d. Cause 4: add an artifact, DELETE it via POST /DeleteArtifact {agentSpaceId, artifactId}, then POST /GetArtifact for that same artifactId -> the emulator answers 200 with the artifact body; expected 404, x-amzn-errortype ResourceNotFoundException and message 'No artifact was found for id: <artifactId>'.
4. Expected overall: a unique artifactId per added artifact, AWS's id-bearing not-found text on both the GetArtifact and AddArtifact rejection paths, and a deleted artifact that is no longer readable.
