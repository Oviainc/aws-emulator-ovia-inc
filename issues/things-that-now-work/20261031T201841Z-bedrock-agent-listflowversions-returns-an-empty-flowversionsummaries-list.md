Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-flow-prepare-and-version
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-flow-prepare-and-version
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|ListFlowVersions|2xx->2xx|$.flowVersionSummaries[*]
Issue-Commit: c58e269113894f48dd3e032b4127d0ccc7e88163

# bedrock-agent ListFlowVersions returns an empty flowVersionSummaries list for a flow that has versions

> _Note: this issue was identified against the code base at commit `c58e269113894f48dd3e032b4127d0ccc7e88163`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFlowVersions

## Description
1 cause.

Cause 1 of 1: ListFlowVersions on a flow reports no versions at all — neither the flow's own DRAFT version nor any numbered version (fixture bedrock-agent-flow-prepare-and-version, step task#7).

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json): ListFlowVersions http {method: GET, uri: /flows/{flowIdentifier}/versions, code: 200}, paginated {inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: flowVersionSummaries}, readonly. Its output member flowVersionSummaries is @required, target FlowVersionSummaries (list, @length max 10), whose member FlowVersionSummary has all of id (FlowId, @required), arn (FlowArn, @required), status (FlowStatus, @required), createdAt (DateTimestamp, @required, timestampFormat date-time) and version (NumericalVersion, @required, pattern ^[0-9]{1,5}$). So a flow that exists always enumerates at least one entry — the DRAFT version it was created with — and one further entry per version minted by CreateFlowVersion (POST /flows/{flowIdentifier}/versions, 201, whose own output carries id/arn/status/createdAt/version). The recording shows exactly that shape: two summaries for the same flow id and arn, one with version 'DRAFT' and one with version '1', both with status 'Prepared'. Recorded behavior only for the ordering of the two entries (DRAFT first, then the numbered version); the model states no sort order.

Emulator: the request is routed and answers 200, but the response's flowVersionSummaries list is empty — the replay reports $.flowVersionSummaries[0] and $.flowVersionSummaries[1] both 'missing (... got None)'. The flow itself exists and GetFlow answers 200 for the same identifier in the same fixture, so this is not a not-found path: the emulator simply never enumerates any version for a flow, not even its DRAFT. Note that the DRAFT entry is independent of version minting, so this is not merely a consequence of the already-open 'bedrock-agent CreateFlowVersion answers 501 NotImplemented' issue (20261031T122907Z-bedrock-agent-createflowversion-answers-501-notimplemented-so-the.md) or of 'bedrock-agent PrepareFlow leaves the flow in NotPrepared and never populates validations' (20261029T172151Z-bedrock-agent-prepareflow-leaves-the-flow-in-notprepared.md): even with CreateFlowVersion implemented and PrepareFlow settling to Prepared, an enumeration that yields nothing for the DRAFT version is still wrong. Neither of those open issues carries this step's replay key.

Why: ListFlowVersions is the only way a client discovers which versions of a flow it may deploy or invoke, and it is the paginated enumeration the AWS CLI/SDK paginators drive (items: flowVersionSummaries). A client that prepares a flow, versions it, and then lists versions to pick the version to alias or invoke sees an account in which the flow has no versions: version-selection logic picks nothing, 'deploy the latest version' loops find no candidate, and code that asserts the @required list is non-empty for an existing flow — a guarantee AWS gives because DRAFT always exists — fails or dereferences an absent element. Because the operation answers 200 rather than an error, the client cannot tell 'this flow has no versions' from 'the service did not enumerate them', so it proceeds on a false negative instead of retrying or surfacing a fault.

Fix: the ListFlowVersions handler's enumeration over the flow's version collection, and the flow state that should hold a DRAFT version entry from CreateFlow onward plus one entry per CreateFlowVersion — the projection must emit a FlowVersionSummary (id, arn, status, createdAt, version) per version and the @required flowVersionSummaries member must never be an empty list for a flow that exists.

Traces: bedrock-agent-flow-prepare-and-version#task#7 (ListFlowVersions, expected 200 / actual 200).

## Evidence
Replay of trace set simparity-wire-aws-v1 — ListFlowVersions, expected status 200 / actual status 200, fixture bedrock-agent-flow-prepare-and-version, step task#7 (cluster affected_count 1; stub_path empty, owned_by empty — net-new):

  $.flowVersionSummaries[0]: missing (expected {'arn': 'arn:aws:bedrock:us-west-2:000000000000:flow/1sXxKHGpnR', 'createdAt': '<shaped-not-asserted>', 'id': '1sXxKHGpnR', 'status': 'Prepared', 'version': 'DRAFT'}, got None)
  $.flowVersionSummaries[1]: missing (expected {'arn': 'arn:aws:bedrock:us-west-2:000000000000:flow/1sXxKHGpnR', 'createdAt': '<shaped-not-asserted>', 'id': '1sXxKHGpnR', 'status': 'Prepared', 'version': '1'}, got None)

divergence_key: service=bedrock-agent|ListFlowVersions|status=200->200|keys=$.flowVersionSummaries[0],$.flowVersionSummaries[1]

Expected vs actual, in general terms: AWS enumerates one FlowVersionSummary per version of the flow — always including the DRAFT version, plus each numbered version created by CreateFlowVersion — each entry carrying the flow's id and arn, the version's status and createdAt, and the version string. The emulator answers 200 with an empty flowVersionSummaries list, so zero entries are enumerated where AWS enumerates at least one.

The flow id ('1sXxKHGpnR'), flow name, account id, region, role ARN and createdAt in the recording are per-run values and are not part of the requirement (createdAt is recorded as '<shaped-not-asserted>'). The requirement is that the number of enumerated summaries matches the flow's versions (never zero for an existing flow), that each entry's id/arn match the flow being listed, and that the version member reads 'DRAFT' for the draft and the minted numeric string for each created version.

Smithy: ListFlowVersions GET /flows/{flowIdentifier}/versions -> 200; output flowVersionSummaries @required, FlowVersionSummaries @length max 10; FlowVersionSummary members id/arn/status/createdAt/version all @required; paginated items = flowVersionSummaries.

## Steps to Reproduce
1. Replay fixture bedrock-agent-flow-prepare-and-version from its setup through task#7 against the emulator: CreateFlow with a valid two-node (Input -> Output) definition, PrepareFlow, CreateFlowVersion, then ListFlowVersions on the same flowIdentifier.
2. Observe ListFlowVersions (GET /flows/{flowIdentifier}/versions) answer 200 with flowVersionSummaries empty.
3. Compare against the recorded response, which carries two FlowVersionSummary entries for the same flow id/arn — version 'DRAFT' and version '1', both status 'Prepared'. The emulator enumerates neither, so $.flowVersionSummaries[0] and [1] are both reported missing.
4. Minimal variant that isolates it from the open CreateFlowVersion/PrepareFlow issues: CreateFlow alone, then ListFlowVersions — AWS still enumerates the flow's DRAFT version; the emulator still returns an empty list.
