Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-create-flow-nonexistent-connector-profile-rejected
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-create-flow-nonexistent-connector-profile-rejected
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|CreateFlow|4xx->2xx|$.message
Issue-Key: appflow|CreateFlow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appflow|CreateFlow|4xx->2xx|status
Issue-Key: appflow|DescribeFlow|4xx->2xx|$.message
Issue-Key: appflow|DescribeFlow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appflow|DescribeFlow|4xx->2xx|status
Issue-Commit: bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c

# AppFlow missing-resource lookups answer 200: CreateFlow accepts a nonexistent connector profile and DescribeFlow never 404s

> _Note: this issue was identified against the code base at commit `bccd4676d2ae0a6cc01fa45a04e1fd18e4c75f4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlow

## Description
Two independent places in the appflow control plane where a reference to a resource that does not exist is answered with HTTP 200 instead of the modeled 404 / ResourceNotFoundException. 2 causes.

Cause 1 of 2: CreateFlow does not resolve the connectorProfileName its flow config references, so a flow naming a connector profile that was never created is accepted and persisted. (fixture appflow-create-flow-nonexistent-connector-profile-rejected, step task#7)
AWS: Smithy: CreateFlow (http POST /create-flow, success code 200) takes flowName (@required, @length 0..256, @pattern ^[a-zA-Z0-9][\\w!@#.-]+$), triggerConfig, sourceFlowConfig and destinationFlowConfigList (all @required) and models the error ResourceNotFoundException (errorKind client, httpError 404, doc "The resource specified in the request (such as the source or destination connector profile) is not found."), whose single member is message (@length 0..2048, @pattern ^[\\s\\w/!@#+=.-]*$). Recorded behavior for the text: when sourceFlowConfig (or a destinationFlowConfigList entry) names a connectorProfileName that does not exist in the account, AWS refuses the call with 404 / ResourceNotFoundException and a message whose FIXED text is "Connector Profile with name " + " does not exist." wrapped around the one VARIABLE part, the connectorProfileName taken verbatim from the request, and it creates no flow. The x-amzn-errortype header carries the error shape name.
Emulator: the call returns HTTP 200 with a normal CreateFlowResponse body and no error at all: no $.message, no x-amzn-errortype, and the flow record is written to state.
Why: the misconfiguration is accepted at definition time instead of being refused. A client that relies on the 404 to detect a mistyped or not-yet-created connector profile gets no signal, believes it holds a working flow, and the account accumulates flows that can never run; error-handling branches coded against ResourceNotFoundException are dead against the emulator.
Fix: the appflow CreateFlow handler's validation stage — every connectorProfileName referenced by sourceFlowConfig and destinationFlowConfigList must be resolved against the connector-profile store and rejected with ResourceNotFoundException(404) carrying the message above BEFORE any flow record is written to state.
Traces: appflow-create-flow-nonexistent-connector-profile-rejected#task#7.
Related but NOT covering this: open issue 20261029T194452Z-appflow-flow-lifecycle-wrong-initial-flowstatus-executions-always.md describes the same validation gap as its Cause 5 from the BEHAVIORAL corpus; it does not carry this wire fixture's replay key, so this wire cause is still unexercised by it.

Cause 2 of 2: DescribeFlow answers 200 for a flow name that does not exist instead of raising ResourceNotFoundException. (fixture appflow-create-flow-nonexistent-connector-profile-rejected, step task#8)
AWS: Smithy: DescribeFlow (http POST /describe-flow, success code 200) takes only flowName (@required) and models exactly two errors — ResourceNotFoundException (client, httpError 404) and InternalServerException (server, 500). Recorded behavior for the text: a DescribeFlow naming a flow that is not present in the account answers 404 / ResourceNotFoundException with a message whose FIXED text is "There is no flow with the name " followed by the one VARIABLE part, the flowName taken verbatim from the request, plus the x-amzn-errortype header naming the error shape. Because the preceding CreateFlow is itself refused (Cause 1), the name is never a flow, so this lookup must miss.
Emulator: the request answers HTTP 200 with a flow description — $.message absent, x-amzn-errortype absent — because the flow Cause 1 should have refused is present in state. The lookup-miss branch is therefore never reached on this path; where it is reached, the open issue noted below records that its sentence is also wrong.
Why: a client cannot distinguish "this flow exists" from "this name was never created". A reconciler or test that treats a successful DescribeFlow as proof of existence concludes a rejected CreateFlow actually created a flow, and any code branching on ResourceNotFoundException to create-on-miss never fires, so absent resources read as present.
Fix: the appflow DescribeFlow handler's lookup path and the flow state it reads — a name with no flow record must produce ResourceNotFoundException(404) with the message above and the x-amzn-errortype header; and, upstream, Cause 1's validation must stop writing the flow record that makes this lookup spuriously hit.
Traces: appflow-create-flow-nonexistent-connector-profile-rejected#task#8.
Related but NOT covering this: open issue 20261105T202318Z-appflow-flow-deactivation-deletion-semantics-deleteflow-ignores-the.md (Cause 3) and 20261029T192701Z-appflow-error-message-text-diverges-from-aws-wording.md describe the WORDING of the DescribeFlow not-found sentence from other corpora; neither reports DescribeFlow returning 200 on this path, and neither carries this fixture's replay key.

## Evidence
Cause 1 — fixture appflow-create-flow-nonexistent-connector-profile-rejected, step task#7, operation CreateFlow: expected HTTP 404 with $.message = "Connector Profile with name <connectorProfileName> does not exist." (the recording's value was 'Connector Profile with name sim2real-af-missing-e2ac3e62 does not exist.' — the generated profile name is this run's value, only the surrounding sentence is the requirement) and a shaped x-amzn-errortype header. Actual: HTTP 200, $.message missing (None), x-amzn-errortype missing (None). Replay divergence_key service=appflow|CreateFlow|status=404->200|keys=$.message,$headers.x-amzn-errortype,status.

Cause 2 — same fixture, step task#8, operation DescribeFlow: expected HTTP 404 with $.message = "There is no flow with the name <flowName>" (recording's value 'There is no flow with the name sim2real-af-badprofile-7c1e94ab'; the flow name is the only variable part, taken from the request) and a shaped x-amzn-errortype header. Actual: HTTP 200, $.message missing (None), x-amzn-errortype missing (None). Replay divergence_key service=appflow|DescribeFlow|status=404->200|keys=$.message,$headers.x-amzn-errortype,status.

Model basis (Smithy, com.amazonaws.appflow#SandstoneConfigurationServiceLambda): CreateFlow errors AccessDeniedException(403), ConflictException(409), ConnectorAuthenticationException(401), ConnectorServerException(400), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ValidationException(400); DescribeFlow errors InternalServerException(500), ResourceNotFoundException(404); ResourceNotFoundException = structure{message: ErrorMessage @length 0..2048 @pattern ^[\\s\\w/!@#+=.-]*$}, errorKind client, httpError 404.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus), fixture appflow-create-flow-nonexistent-connector-profile-rejected.
2. Cause 1: POST /create-flow with a valid flowName, triggerConfig {triggerType: OnDemand}, and a sourceFlowConfig whose connectorProfileName was never created (no CreateConnectorProfile for it). Observe HTTP 200 and a CreateFlowResponse; AWS answers 404 ResourceNotFoundException with message "Connector Profile with name <connectorProfileName> does not exist." and creates no flow. Confirm with ListFlows that no flow was persisted.
3. Cause 2: POST /describe-flow with the flowName used in step 2. Observe HTTP 200 and a flow description; AWS answers 404 ResourceNotFoundException with message "There is no flow with the name <flowName>" and the x-amzn-errortype header. Repeat with any never-created flowName to exercise the lookup-miss branch independently of Cause 1.
