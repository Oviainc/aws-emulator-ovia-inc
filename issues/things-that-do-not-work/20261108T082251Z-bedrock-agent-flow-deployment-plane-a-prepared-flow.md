Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-flow-alias-lifecycle
Issue-Step: setup#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-flow-alias-lifecycle bedrock-agent-flow-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|CreateFlowVersion|2xx->4xx|status
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.arn
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.createdAt
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.description
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.flowId
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.id
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.name
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.routingConfiguration
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|$.updatedAt
Issue-Key: bedrock-agent|CreateFlowAlias|2xx->4xx|status
Issue-Key: bedrock-agent|ListFlowAliases|2xx->2xx|$.flowAliasSummaries[*]
Issue-Key: bedrock-agent|CreateFlowAlias|4xx->4xx|$.message
Issue-Commit: 824a021fbc81c5b46e0dca2fae74f8c18911ab8d

# bedrock-agent flow deployment plane: a prepared flow is refused 409 on CreateFlowVersion, no TSTALIASID test alias exists, and CreateFlowAlias names the wrong missing resource type

> _Note: this issue was identified against the code base at commit `824a021fbc81c5b46e0dca2fae74f8c18911ab8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowVersion

## Description
4 causes. All four sit in the bedrock-agent flow deployment plane (prepare -> version -> alias -> list): the emulator interposes an artificial preparation span that turns a legal CreateFlowVersion into a 409, which strands alias creation, it never mints the test alias AWS gives every flow, and its CreateFlowAlias not-found refusal names the wrong resource type.

Cause 1 of 4: CreateFlowVersion on a flow whose preparation has just been accepted is refused 409 ConflictException instead of cutting version 1 (fixture bedrock-agent-flow-alias-lifecycle, step setup#4).
AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models CreateFlowVersion as POST /flows/{flowIdentifier}/versions with success status 201 and output requiring name, executionRoleArn, id, arn, status (FlowStatus = Failed | Prepared | Preparing | NotPrepared), createdAt and version (NumericalVersion @pattern ^[0-9]{1,5}$); ConflictException (@error client, @httpError 409) is modelled but is owed only when the flow genuinely cannot be versioned. PrepareFlow is POST /flows/{flowIdentifier}/ with success 202 and output {id, status}. Recorded behavior: in this fixture the sequence CreateFlow -> PrepareFlow -> CreateFlowVersion answered 201 and cut a numbered version, i.e. AWS had the flow versionable by the time the very next call arrived; preparation of a small DRAFT definition is not a multi-second wait a caller must sit out. The requirement is that a flow AWS has accepted a preparation for, and whose definition carries no error finding, can be versioned by the immediately following CreateFlowVersion, answering 201 with version '1'.
Emulator: replay observed HTTP 409 where 201 was required, with no CreateFlowVersionResponse body. The handler's status pre-condition (services/.../src/ops/create_flow_version.rs lines 243-262) refuses any flow whose stored status is not FLOW_STATUS_VERSIONS_ARE_CUT_FROM (= Prepared) with resource_status_conflict_message, whose fixed text is "Couldn't perform <operation> operation. The resource that you specified is currently in status <status>. Retry your request after the resource changes to a valid status." — its only variable parts being the operation name and the status token read from state. PrepareFlow decides the outcome at accept time but defers publishing it for a chosen wall-clock span (services/.../src/ops/prepare_flow.rs lines 224-239 plus FLOW_PREPARATION_SPAN = 3s in src/state.rs line 324), so for those 3 seconds the flow reports Preparing and every CreateFlowVersion in that window is refused. Hypothesis: it is that emulator-chosen span, not any property of the request, that produces the 409 — the definition in this fixture yields no error finding, so the deferred outcome is Prepared.
Why: the whole deploy path is gated on versioning. A client that prepares a flow and versions it in sequence — exactly what the recording does — gets a conflict for a flow AWS considers ready, so no version is cut, no alias can route to one, and the client cannot tell this emulator-authored timing refusal from a real "prepare first" conflict. The refusal is not even stable: the same call succeeds if the caller happens to arrive late enough.
Fix: the flow preparation state machine and the CreateFlowVersion pre-condition that reads it — services/.../src/ops/prepare_flow.rs together with src/state.rs's PendingPreparation/materialise_due_transitions and FLOW_PREPARATION_SPAN, so a prepared flow is versionable by the next call rather than after a fixed span, and services/.../src/ops/create_flow_version.rs lines 243-262, which must refuse only a flow that is genuinely NotPrepared/Failed.
Traces: bedrock-agent-flow-alias-lifecycle#setup#4.

Cause 2 of 4: CreateFlowAlias routing to version '1' of that flow answers 404 ResourceNotFoundException instead of 201 with the created alias (fixture bedrock-agent-flow-alias-lifecycle, step task#5).
AWS: Smithy models CreateFlowAlias as POST /flows/{flowIdentifier}/aliases with success status 201, input requiring name and routingConfiguration (FlowAliasRoutingConfiguration @length 1..1) and output requiring name, routingConfiguration, flowId, id (FlowAliasId @pattern ^(TSTALIASID|[0-9a-zA-Z]{10})$), arn (FlowAliasArn @pattern ^arn:aws:bedrock:<region>:<account>:flow/<flowId>/alias/(TSTALIASID|[0-9a-zA-Z]{10})$), createdAt and updatedAt. The requirement: an alias whose single routing entry names a version the flow holds is created and reported back — arn is the flow's own ARN with /alias/<aliasId> appended, flowId is the addressed flow, description and routingConfiguration are echoed from the request, and id is a freshly minted alias id (the recording's concrete id, flow id and timestamps are per-run values, not part of the contract).
Emulator: replay observed HTTP 404 where 201 was required; every asserted output member was absent ($.arn, $.createdAt, $.description, $.flowId, $.id, $.name, $.routingConfiguration, $.updatedAt all missing). The routing-target check in services/.../src/ops/create_flow_alias.rs lines 240-264 resolves the named flowVersion through state.flow_version_exists and refuses with flow_version_not_found_message when the flow holds no such version. Because cause 1 stopped version 1 from ever being cut, the version the alias routes to does not exist in emulator state, so the alias plane is unreachable even though CreateFlowAlias itself is implemented.
Why: alias creation is how a flow is deployed and how bedrock-agent-runtime addresses it. A client that versions and then aliases a flow gets a not-found for a version AWS had already cut, so no alias, no deployment target, and every downstream read/update/delete of that alias fails too.
Fix: primarily the preparation/versioning sequencing named in cause 1 (so version 1 exists); re-verify the CreateFlowAlias handler's routing-target resolution in services/.../src/ops/create_flow_alias.rs lines 240-264 once a version can be cut, so a routing entry naming an existing numbered version is accepted and the alias is written to state.
Traces: bedrock-agent-flow-alias-lifecycle#task#5.

Cause 3 of 4: ListFlowAliases on a flow reports none of its aliases — in particular the flow carries no TSTALIASID test alias at all (fixture bedrock-agent-flow-alias-lifecycle, step task#8).
AWS: Smithy models ListFlowAliases as GET /flows/{flowIdentifier}/aliases with success 200 and output requiring flowAliasSummaries (FlowAliasSummaries @length ..10, paginated with nextToken/maxResults); FlowAliasId is @pattern ^(TSTALIASID|[0-9a-zA-Z]{10})$ and FlowAliasArn admits the TSTALIASID segment, i.e. the reserved test alias is a first-class alias of every flow. Recorded behavior: the listing carries TWO summaries — the caller's own alias (its current name/description and routingConfiguration as last written) and the service-created test alias, whose id, name and description are all the literal token 'TSTALIASID' and whose routingConfiguration is [{flowVersion: 'DRAFT'}], with an arn of <flowArn>/alias/TSTALIASID. The requirement is that every flow carries the DRAFT-routing TSTALIASID alias from creation, with no caller action, and that ListFlowAliases enumerates it alongside caller-created aliases.
Emulator: replay observed HTTP 200 with neither summary present ($.flowAliasSummaries[0] and $.flowAliasSummaries[1] both missing). The caller's alias is absent downstream of cause 2, but TSTALIASID is absent unconditionally: nothing in the store ever creates it — src/state/flow_aliases.rs lines 132-136 document TEST_ALIAS_ID as naming an alias "this store holds only once something has created one under it", and mint_flow_alias_id (lines 231-243) deliberately skips that token, so no flow ever holds it. ListFlowAliases (src/ops/list_flow_aliases.rs lines 225-243) faithfully pages whatever flow_aliases holds, which for a fresh flow is nothing.
Why: TSTALIASID is the alias every Bedrock client uses to invoke the DRAFT version for testing before cutting a version. A client that lists aliases to find the test alias, or that invokes/reads/updates it by name, is told the flow has no aliases, so the test-invoke path is unreachable and alias-count-based logic (quotas, idempotent provisioning) sees the wrong state.
Fix: the flow-creation state transition — mint the reserved TSTALIASID FlowAliasRecord routing to DRAFT when a flow is stored (services/.../src/ops/create_flow.rs together with the flow-alias slice in services/.../src/state/flow_aliases.rs), and keep it out of the caller-settable alias namespace so ListFlowAliases/GetFlowAlias report it like any other alias.
Traces: bedrock-agent-flow-alias-lifecycle#task#8.

Cause 4 of 4: CreateFlowAlias's 404 refusal names resource type 'Flow' where AWS names 'FlowVersion' (fixture bedrock-agent-flow-error-paths, step task#5).
AWS: Smithy models ResourceNotFoundException (@error client, @httpError 404, doc "The specified resource Amazon Resource Name (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.") on CreateFlowAlias. Recorded behavior for this error class: HTTP 404 with the message "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type FlowVersion because it doesn't exist. Check the resource ID and retry your request." Fixed text of the template: "Couldn't perform " + <operation name> + " operation. Failed to retrieve resource type " + <resource type> + " because it doesn't exist. Check the resource ID and retry your request." The only variable parts are the invoked operation name (here CreateFlowAlias) and the type of the resource that failed to resolve; no identifier appears in it. The requirement is the value of that second variable: when a CreateFlowAlias cannot resolve the version its routingConfiguration names, AWS reports FlowVersion.
Emulator: replay observed HTTP 404 — the right status and error class — but the message read "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type Flow because it doesn't exist. Check the resource ID and retry your request.", i.e. resource type 'Flow' instead of 'FlowVersion'. Both strings come from the same template in src/state.rs lines 551-570 (resource_not_found_message, flow_not_found_message -> "Flow") and src/state/flow_aliases.rs lines 179-184 (flow_version_not_found_message -> "FlowVersion"), so only the resource-type argument differs. Hypothesis: the handler's ordering is what picks the wrong one — services/.../src/ops/create_flow_alias.rs looks the flow itself up first (lines 230-237, flow_not_found_message) and only then resolves the routing target (lines 240-264, flow_version_not_found_message), so whenever neither resolves the emulator reports Flow while AWS reports the unresolvable FlowVersion.
Why: the resource type is the only part of this message that tells a client WHAT was missing. A client (or an error-matching script) that distinguishes "the flow is gone" from "the version I am pinning does not exist" draws the wrong conclusion and retries the wrong thing; the two cases demand different remedies (re-create the flow vs cut/choose a version).
Fix: the CreateFlowAlias handler's not-found resolution order and the resource type it passes to the message helper — services/.../src/ops/create_flow_alias.rs (the flow lookup at lines 230-237 versus the routing-target resolution at lines 240-264), so the refusal names FlowVersion for an unresolvable routing target.
Traces: bedrock-agent-flow-error-paths#task#5.

Related open issues that do NOT cover these four causes (none carries these fixture steps' replay keys): 20261104T195106Z-bedrock-agent-createflowversion-has-no-prepare-ordering-guard.md asked for the 409 guard that cause 1 now over-applies (it covers the NotPrepared case, not a prepared flow being refused); 20261031T122907Z-bedrock-agent-createflowversion-answers-501-notimplemented-so-the.md, 20261105T003415Z-bedrock-agent-createflowalias-stub.md and 20261102T104226Z-bedrock-agent-listflowaliases-stub.md all describe these operations as unimplemented 501 scaffold stubs, whereas all three are now implemented and answer 409/404/200; 20261029T172151Z-bedrock-agent-prepareflow-leaves-the-flow-in-notprepared.md covers PrepareFlow's status transition and GetFlow's validations member, not the versioning window or the alias plane; 20261107T230358Z-bedrock-agent-4-net-new-causes-createflowalias-has.md covers alias name-uniqueness and delete-time in-use guards, not the test alias, the listing, or this not-found wording.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 195 steps, score 62.1%).

Cause 1 — bedrock-agent-flow-alias-lifecycle#setup#4, CreateFlowVersion:
  status: expected 201, got 409
  Expected (AWS): HTTP 201 with a CreateFlowVersionResponse carrying version '1' and the flow's name/arn/executionRoleArn/status/createdAt.
  Actual (emulator): HTTP 409 ConflictException, message template "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status <status>. Retry your request after the resource changes to a valid status." — fixed text as quoted, <status> read from state (Preparing during the emulator's 3s FLOW_PREPARATION_SPAN).

Cause 2 — bedrock-agent-flow-alias-lifecycle#task#5, CreateFlowAlias:
  status: expected 201, got 404
  $.arn: missing (expected <flowArn>/alias/<aliasId>, got None)
  $.createdAt: missing; $.updatedAt: missing (shaped, not asserted literally)
  $.description: missing (expected the description sent in the request, got None)
  $.flowId: missing (expected the addressed flow id, got None)
  $.id: missing (shaped alias id, got None)
  $.name: missing (expected the alias name sent in the request, got None)
  $.routingConfiguration: missing (expected [{flowVersion: '1'}], got None)

Cause 3 — bedrock-agent-flow-alias-lifecycle#task#8, ListFlowAliases:
  status 200 == 200, but flowAliasSummaries carried neither expected entry:
  $.flowAliasSummaries[0]: missing (expected the caller's alias: arn <flowArn>/alias/<aliasId>, flowId <flowId>, name/description as last written, routingConfiguration [{flowVersion: '1'}], got None)
  $.flowAliasSummaries[1]: missing (expected the service-created test alias: id 'TSTALIASID', name 'TSTALIASID', description 'TSTALIASID', arn <flowArn>/alias/TSTALIASID, routingConfiguration [{flowVersion: 'DRAFT'}], got None)

Cause 4 — bedrock-agent-flow-error-paths#task#5, CreateFlowAlias (status 404 == 404):
  $.message: value_mismatch
    expected "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type FlowVersion because it doesn't exist. Check the resource ID and retry your request."
    got      "Couldn't perform CreateFlowAlias operation. Failed to retrieve resource type Flow because it doesn't exist. Check the resource ID and retry your request."
  Fixed text: "Couldn't perform <operation> operation. Failed to retrieve resource type <resourceType> because it doesn't exist. Check the resource ID and retry your request."; variable parts: <operation> = CreateFlowAlias, <resourceType> = the type that failed to resolve (FlowVersion).

Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json):
  CreateFlowVersion http {POST /flows/{flowIdentifier}/versions, code 201}; errors ConflictException(409), ResourceNotFoundException(404); output.version NumericalVersion @pattern ^[0-9]{1,5}$; FlowStatus = [Failed, Prepared, Preparing, NotPrepared].
  PrepareFlow http {POST /flows/{flowIdentifier}/, code 202}; output {id, status} required.
  CreateFlowAlias http {POST /flows/{flowIdentifier}/aliases, code 201}; routingConfiguration @required @length 1..1; id FlowAliasId @pattern ^(TSTALIASID|[0-9a-zA-Z]{10})$; arn FlowAliasArn admits the /alias/TSTALIASID segment; errors ResourceNotFoundException(404), ConflictException(409).
  ListFlowAliases http {GET /flows/{flowIdentifier}/aliases, code 200}; output.flowAliasSummaries @required @length ..10; paginated.

Source: services/.../src/ops/create_flow_version.rs lines 243-262 (status pre-condition); src/ops/prepare_flow.rs lines 224-239 and src/state.rs line 324 (FLOW_PREPARATION_SPAN = 3s, explicitly "CHOSEN, not documented"); src/state.rs line 367 (FLOW_STATUS_VERSIONS_ARE_CUT_FROM = Prepared); src/state.rs lines 551-585 (resource_not_found_message / flow_not_found_message / resource_status_conflict_message); src/state/flow_aliases.rs lines 132-136, 179-184, 231-243 (TEST_ALIAS_ID never created, flow_version_not_found_message); src/ops/create_flow_alias.rs lines 230-264; src/ops/list_flow_aliases.rs lines 225-243.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a bedrock-agent client at it.
2. Cause 1: CreateFlow with a valid minimal definition and a flow execution role ARN; note the flow id. Call PrepareFlow on it (202), then IMMEDIATELY call CreateFlowVersion(flowIdentifier=<id>).
   Observed: HTTP 409 ConflictException "...currently in status Preparing...".
   Expected: HTTP 201 with version '1'. (Waiting out the emulator's 3s span makes the same call succeed, which is itself the defect.)
3. Cause 2: with no version cut, call CreateFlowAlias(flowIdentifier=<id>, name=<alias>, routingConfiguration=[{flowVersion: '1'}]).
   Observed: HTTP 404 ResourceNotFoundException and no alias members.
   Expected: HTTP 201 with arn <flowArn>/alias/<aliasId>, flowId, id, name, description, routingConfiguration [{flowVersion: '1'}], createdAt, updatedAt.
4. Cause 3: call ListFlowAliases(flowIdentifier=<id>) on a freshly created flow, before creating any alias.
   Observed: HTTP 200 with an empty flowAliasSummaries.
   Expected: HTTP 200 carrying at least the service-created test alias — id/name/description 'TSTALIASID', arn <flowArn>/alias/TSTALIASID, routingConfiguration [{flowVersion: 'DRAFT'}].
5. Cause 4: call CreateFlowAlias naming a routing flowVersion that cannot be resolved for the addressed flow.
   Observed: HTTP 404 with message "...Failed to retrieve resource type Flow because it doesn't exist...".
   Expected: HTTP 404 with the same template naming resource type FlowVersion.
6. Replay fixtures bedrock-agent-flow-alias-lifecycle (setup#4, task#5, task#8) and bedrock-agent-flow-error-paths (task#5) to reproduce all four divergences from the recordings.
