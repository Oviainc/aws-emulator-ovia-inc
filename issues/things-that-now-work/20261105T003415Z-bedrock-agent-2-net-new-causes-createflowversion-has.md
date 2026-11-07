Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.flow-version-creation-without-prepare-rejected bedrock-agent.validate-flow-definition-catches-cycle-before-version bedrock-agent.flow-alias-dangling-version-reference-rejected
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.flow-version-creation-without-prepare-rejected|_error#9|AssertionError|raise AssertionError(f"expected ConflictException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-agent.validate-flow-definition-catches-cycle-before-version|_error#24|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: bedrock-agent.flow-alias-dangling-version-reference-rejected|run#12|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Commit: 0a8b28aea83ccc3190789fc779439d6e199d02f9

# bedrock-agent: 2 net-new causes — CreateFlowVersion has no FlowStatus pre-condition (NotPrepared and Failed flows are versioned with 201), and CreateFlowAlias answers 501 so a dangling routingConfiguration version is never refused

> _Note: this issue was identified against the code base at commit `0a8b28aea83ccc3190789fc779439d6e199d02f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowVersion

## Description
2 causes.

Cause 1 of 2: CreateFlowVersion applies no FlowStatus pre-condition, so a flow that is NOT Prepared (NotPrepared, or Failed after a failed PrepareFlow) is versioned successfully instead of being refused. Failing checks: bedrock-agent.flow-version-creation-without-prepare-rejected, bedrock-agent.validate-flow-definition-catches-cycle-before-version.

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models CreateFlowVersion as POST /flows/{flowIdentifier}/versions with success code 201, and its modeled errors include ConflictException (@error client, @httpError 409) alongside ResourceNotFoundException (404) and ValidationException (400). Output requires status (FlowStatus enum = Failed | Prepared | Preparing | NotPrepared) and version (NumericalVersion, @pattern ^[0-9]{1,5}$). Versioning is ordered behind preparation: AWS only cuts a numbered version from a DRAFT that PrepareFlow has driven to Prepared. For any other status the call is refused with HTTP 409, error code ConflictException and (recorded behavior for this error class) the message
  "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status <status>. Retry your request after the resource changes to a valid status."
Fixed text of that template: "Couldn't perform " + <operation name> + " operation. The resource that you specified is currently in status " + <status> + ". Retry your request after the resource changes to a valid status." The ONLY variable parts are the operation name (here CreateFlowVersion) and <status>, the addressed flow's current FlowStatus token read from service state — the recordings show NotPrepared for a never-prepared DRAFT and Failed for a flow whose PrepareFlow ended Failed (a cyclic definition). Neither token is a literal to hard-code from one run. The refusal is also side-effect-free: no version number is consumed and ListFlowVersions gains no numbered entry.

Emulator: the request is accepted in both statuses. Replay of flow-version-creation-without-prepare-rejected shows HTTP 201 on a flow GetFlow reports as NotPrepared, with a CreateFlowVersionResponse carrying status NotPrepared and a numbered version, and no error code or message at all. Replay of validate-flow-definition-catches-cycle-before-version shows the same 201 on a flow whose PrepareFlow correctly ended Failed with a CyclicConnection validation: the response carries status Failed and a numbered version. The handler never inspects the flow's status before mutating state, so the modeled ConflictException is never constructed for this pre-condition.

Why: the prepare -> version -> alias -> invoke ordering that Bedrock flows depend on is unenforced. A client gets a success and a numeric version pointing at an unprepared or validation-failed, unusable definition instead of the 409 that tells it to prepare and retry; negative-path code branching on ConflictException never fires; and the leaked version is visible to ListFlowVersions/GetFlowVersion, so later version numbering and per-flow quota accounting drift by versions AWS would never have cut.

Fix: a pre-condition check in the CreateFlowVersion handler (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs) — after the flow lookup and before any state mutation, read the flow's FlowStatus from state and return the modeled 409 ConflictException with the message template above for every status other than Prepared, without consuming a version number or writing a version record.

Related open issue, which does NOT carry these behavioral checks' replay keys and covers only the NotPrepared half: 20261104T195106Z-bedrock-agent-createflowversion-has-no-prepare-ordering-guard.md (a wire finding bound to fixture bedrock-agent-flow-error-paths#task#4). The Failed-status case proven by validate-flow-definition-catches-cycle-before-version is additional to it: the guard must key off any non-Prepared FlowStatus, not NotPrepared alone. 20261031T122907Z-bedrock-agent-createflowversion-answers-501-notimplemented-so-the.md and 20261102T104226Z-bedrock-agent-createflowversion-stub.md describe the operation as a 501 stub; it now answers 201, so neither covers this.

Traces: bedrock-agent.flow-version-creation-without-prepare-rejected (NotPrepared), bedrock-agent.validate-flow-definition-catches-cycle-before-version (Failed, after a cyclic definition fails to prepare).

Cause 2 of 2: CreateFlowAlias is a 501 NotImplemented scaffold stub, so the routingConfiguration-references-an-existing-flow-version pre-condition is never applied and the modeled 404 ResourceNotFoundException never happens. Failing check: bedrock-agent.flow-alias-dangling-version-reference-rejected.

AWS: Smithy models CreateFlowAlias as POST /flows/{flowIdentifier}/aliases with success code 201; input requires name (Name, @pattern ^([0-9a-zA-Z][_-]?){1,100}$) and routingConfiguration (FlowAliasRoutingConfiguration, @length 1..1), and its modeled errors include ResourceNotFoundException (@error client, @httpError 404), ConflictException (409) and ValidationException (400). The alias's single routing entry must name a flow version that exists on the addressed flow; when it does not, AWS refuses with HTTP 404, error code ResourceNotFoundException and (recorded behavior for this error class) the message
  "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type FlowVersion because it doesn't exist. Check the resource ID and retry your request."
Fixed text of that template: "Couldn't perform " + <operation name> + " operation. Failed to retrieve resource type " + <resource type> + " because it doesn't exist. Check the resource ID and retry your request." The variable parts are the operation name (CreateFlowAlias) and the resource type that could not be resolved (FlowVersion); the rejected version token itself is not echoed. The refusal is side-effect-free — the alias name must not appear in ListFlowAliases afterwards — and the identical call with an existing version must succeed with 201 and round-trip routingConfiguration[0].flowVersion through GetFlowAlias.

Emulator: the call answers error code NotImplemented ("operation CreateFlowAlias is scaffolded but not implemented in this emulator") for every request — both the dangling-version request that must be refused 404 ResourceNotFoundException and the valid positive-control request that must succeed 201. NotImplemented is not in the operation's modeled error set, so a client cannot classify it.

Why: no client can create or address a flow alias, so the flow invocation path (alias -> routed version) is unreachable; and because the refusal class is wrong, a client probing a dangling version reference sees an unmodelled NotImplemented rather than the 404 ResourceNotFoundException it branches on, and cannot distinguish a bad version reference from an outage.

Fix: implement the CreateFlowAlias handler (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_alias.rs) — persist the alias and, in its validation, resolve every routingConfiguration[].flowVersion against the addressed flow's stored versions, returning the modeled 404 ResourceNotFoundException with the message template above when one is absent, writing nothing in that case.

Traces: bedrock-agent.flow-alias-dangling-version-reference-rejected (asserts ResourceNotFoundException, receives NotImplemented).

## Evidence
Cause 1 of 2 — CreateFlowVersion with no FlowStatus guard.

Check bedrock-agent.flow-version-creation-without-prepare-rejected [mismatch]:
  AssertionError: expected ConflictException, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'status': 'NotPrepared', 'version': '1', ...}
  Expected (AWS): HTTP 409, error code ConflictException, message "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status." — fixed text as quoted, with <operation name>=CreateFlowVersion and <status> read from the flow's state; ListFlowVersions unchanged.
  Actual (emulator): HTTP 201, CreateFlowVersionResponse with status 'NotPrepared' and a numbered version; no error code, no error message.

Check bedrock-agent.validate-flow-definition-catches-cycle-before-version [mismatch]:
  The earlier legs pass — ValidateFlowDefinition reports a CyclicConnection Error finding, and PrepareFlow on the cyclic flow correctly settles at status Failed with a CyclicConnection validation. The divergence is the next call:
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'status': 'Failed', 'version': '1', ...}
  Expected (AWS): HTTP 409, error code ConflictException, message "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status Failed. Retry your request after the resource changes to a valid status." — same template, <status>=Failed from state; afterwards ListFlowVersions holds no numbered version.
  Actual (emulator): HTTP 201 with status 'Failed' and version '1'; the version leaks into the listing.

Smithy: CreateFlowVersion http {method POST, uri /flows/{flowIdentifier}/versions, code 201}; errors include ConflictException (client, httpError 409); output.status = FlowStatus [Failed, Prepared, Preparing, NotPrepared]; output.version = NumericalVersion @pattern ^[0-9]{1,5}$.

Cause 2 of 2 — CreateFlowAlias 501 stub.

Check bedrock-agent.flow-alias-dangling-version-reference-rejected [mismatch]:
  AssertionError: NotImplemented (at `assert code == "ResourceNotFoundException", code`)
  Request: CreateFlowAlias(flowIdentifier=<prepared flow id>, name=<alias name>, routingConfiguration=[{"flowVersion": "999"}]) where only version "1" exists.
  Expected (AWS): HTTP 404, error code ResourceNotFoundException, message "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type FlowVersion because it doesn't exist. Check the resource ID and retry your request." — fixed text as quoted, with <operation name>=CreateFlowAlias and <resource type>=FlowVersion; the alias name absent from ListFlowAliases; and the same call with the existing version succeeding 201 and round-tripping routingConfiguration[0].flowVersion through GetFlowAlias.
  Actual (emulator): error code NotImplemented, message "operation CreateFlowAlias is scaffolded but not implemented in this emulator" — for the rejected request and for the valid positive control alike.

Smithy: CreateFlowAlias http {method POST, uri /flows/{flowIdentifier}/aliases, code 201}; input requires name and routingConfiguration (@length 1..1); errors include ResourceNotFoundException (client, httpError 404), ConflictException (409), ValidationException (400). NotImplemented is not a modeled error of this operation.

## Steps to Reproduce
Cause 1 of 2 (a) — never-prepared DRAFT:
1. bedrock-agent CreateFlow with a valid minimal passthrough definition and a flow execution role ARN; note the flow id. GetFlow reports status NotPrepared.
2. ListFlowVersions(flowIdentifier=<id>) and record the baseline listing.
3. CreateFlowVersion(flowIdentifier=<id>) WITHOUT calling PrepareFlow.
   Observed: HTTP 201 with status 'NotPrepared' and version '1'.
   Expected: HTTP 409, ConflictException, "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status."
4. ListFlowVersions again: the emulator shows a leaked numbered version; AWS's listing is unchanged.
5. Re-replay behavioral check bedrock-agent.flow-version-creation-without-prepare-rejected.

Cause 1 of 2 (b) — Failed flow:
1. ValidateFlowDefinition with a cyclic definition: a CyclicConnection Error finding is returned (this already matches).
2. CreateFlow with that same cyclic definition (accepted), then PrepareFlow and poll GetFlow until it settles at status Failed with a CyclicConnection validation (this already matches).
3. CreateFlowVersion(flowIdentifier=<cyclic flow id>).
   Observed: HTTP 201 with status 'Failed' and version '1'.
   Expected: HTTP 409, ConflictException, same template with <status>=Failed.
4. ListFlowVersions(flowIdentifier=<cyclic flow id>): no numbered version may exist.
5. Re-replay behavioral check bedrock-agent.validate-flow-definition-catches-cycle-before-version.

Cause 2 of 2:
1. CreateFlow with a valid passthrough definition and a flow execution role ARN; PrepareFlow and poll GetFlow to Prepared; CreateFlowVersion to mint version "1".
2. CreateFlowAlias(flowIdentifier=<id>, name=<alias>, routingConfiguration=[{"flowVersion": "999"}]).
   Observed: error code NotImplemented, "operation CreateFlowAlias is scaffolded but not implemented in this emulator".
   Expected: HTTP 404, ResourceNotFoundException, "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type FlowVersion because it doesn't exist. Check the resource ID and retry your request."
3. ListFlowAliases(flowIdentifier=<id>): the rejected alias name must not appear.
4. CreateFlowAlias with routingConfiguration=[{"flowVersion": "1"}] (positive control): expected HTTP 201, and GetFlowAlias must return routingConfiguration[0].flowVersion == "1".
5. Re-replay behavioral check bedrock-agent.flow-alias-dangling-version-reference-rejected.
