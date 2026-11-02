Issue-Source: trace-sweep-divergence
Issue-Trace: acc-gateway-rate-limit-batch-and-update
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-gateway-rate-limit-batch-and-update acc-gateway-update-and-target-errors bedrock-agentcore-control-gateway-get-returns-metadata bedrock-agentcore-control-gateway-lifecycle-basic
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.gatewayUrl
Issue-Key: bedrock-agentcore-control|UpdateGateway|2xx->2xx|$.gatewayUrl
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.gatewayUrl
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.workloadIdentityDetails.workloadIdentityArn
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Gateway responses omit the MCP transport path in gatewayUrl and vend workloadIdentityDetails on create

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
The emulator's gateway response payloads are shaped wrongly in two ways: the `gatewayUrl` it returns is the gateway host with no protocol transport path, and `CreateGateway` returns a `workloadIdentityDetails` block that AWS does not return on create. 4 causes must all be fixed before this issue is closed.

Cause 1 of 4: CreateGateway gatewayUrl has no transport path.
AWS: CreateGateway (POST /gateways/, HTTP 202) returns `gatewayUrl` (Smithy: CreateGateway.gatewayUrl -> GatewayUrl, @length min 1 max 1024, no pattern modeled). The model does not state the URL's composition, so recorded behavior only: AWS returns the gateway's endpoint as `https://<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp` — the host followed by the transport path for the gateway's `protocolType` (MCP, the modeled default for GatewayProtocolType). The path component is part of the returned value, not something the client appends.
Emulator: returns only the host form `https://<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com`, with the transport path missing, so the replayed value does not match the recorded gateway-URL shape for the step.
Why: a client that takes `gatewayUrl` straight from CreateGateway and opens an MCP session against it addresses the gateway root instead of the MCP transport endpoint; every tools/list and tools/call made through the vended URL goes to the wrong path, and nothing in the response tells the client a suffix is owed.
Fix: the CreateGateway handler's response serializer, where the gateway URL is composed from the gateway id, region and protocolType.
Traces: acc-gateway-rate-limit-batch-and-update#setup#2 (also acc-gateway-update-and-target-errors#setup#2).

Cause 2 of 4: UpdateGateway gatewayUrl has no transport path.
AWS: UpdateGateway (PUT /gateways/{gatewayIdentifier}/, HTTP 202) returns the same `gatewayUrl` member (Smithy: UpdateGateway.gatewayUrl -> GatewayUrl). Recorded behavior only: the value carries the `/mcp` transport path, identical to the URL CreateGateway vended for that gateway — an update does not change the endpoint.
Emulator: returns the host-only URL, dropping the transport path, so the value differs from the recorded URL for the same gateway in exactly the trailing path segment.
Why: a client that re-reads the endpoint after an update (role, authorizer or protocol change) silently downgrades a working URL to an unusable one.
Fix: the UpdateGateway handler's response serializer — it must emit the same composed URL as create, not a re-derived host.
Traces: acc-gateway-update-and-target-errors#task#5.

Cause 3 of 4: GetGateway gatewayUrl has no transport path.
AWS: GetGateway (GET /gateways/{gatewayIdentifier}/, HTTP 200) returns `gatewayUrl` (Smithy: GetGateway.gatewayUrl -> GatewayUrl). Recorded behavior only: the read-back value is byte-identical to the URL create/update returned, including the `/mcp` path.
Emulator: returns the host-only URL on the read path too, so create, update and get are all consistently missing the path — the defect is in how the URL is stored/composed, not in one handler's copy of it.
Why: GetGateway is the discovery call an MCP client makes to find the endpoint; a path-less URL breaks every subsequent gateway invocation and makes the emulator's gateway unusable end-to-end rather than merely cosmetically different.
Fix: the gateway state's URL composition (shared by GetGateway/CreateGateway/UpdateGateway serializers), so the stored or derived endpoint includes the protocol transport path.
Traces: acc-gateway-update-and-target-errors#task#6 (also bedrock-agentcore-control-gateway-get-returns-metadata#task#3).

Cause 4 of 4: CreateGateway returns a workloadIdentityDetails block AWS omits.
AWS: `workloadIdentityDetails` is an optional output member of CreateGatewayResponse (Smithy: CreateGateway.workloadIdentityDetails -> WorkloadIdentityDetails{workloadIdentityArn required}). Recorded behavior only: on the create response AWS does not populate it — the workload identity is not yet resolved when the gateway is accepted at 202, so the member is absent from the JSON body.
Emulator: includes `workloadIdentityDetails.workloadIdentityArn` with a synthesized workload-identity-directory ARN in the CreateGateway 202 body.
Why: a client or assertion that treats the presence of `workloadIdentityDetails` as the signal that the gateway's workload identity has been provisioned concludes provisioning finished at create time; against AWS that signal only appears later, so emulator-only code paths get exercised and real ones never do.
Fix: the CreateGateway handler's response serializer — do not emit workloadIdentityDetails on the create (202) response.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#setup#1.

## Evidence
Cause 1 — acc-gateway-rate-limit-batch-and-update#setup#2, CreateGateway, expected 202 / actual 202: $.gatewayUrl value_mismatch; recorded value is the gateway host plus the MCP transport path, emulator returned 'https://sim2real-probe-acc-gwrl-6322de97-flabla1jzn.gateway.bedrock-agentcore.us-west-2.amazonaws.com' with no path. Cluster also covers acc-gateway-update-and-target-errors#setup#2.
Cause 2 — acc-gateway-update-and-target-errors#task#5, UpdateGateway, 202 vs 202: $.gatewayUrl expected '...gateway.bedrock-agentcore.us-west-2.amazonaws.com/mcp', got the same URL without '/mcp'.
Cause 3 — acc-gateway-update-and-target-errors#task#6, GetGateway, 200 vs 200: $.gatewayUrl expected '...amazonaws.com/mcp', got '...amazonaws.com'. Cluster also covers bedrock-agentcore-control-gateway-get-returns-metadata#task#3.
Cause 4 — bedrock-agentcore-control-gateway-lifecycle-basic#setup#1, CreateGateway, 202 vs 202: $.workloadIdentityDetails.workloadIdentityArn reported 'extra' (expected absent, emulator emitted a workload-identity-directory ARN). Host/id/account values above are per-run; the requirement is the presence of the trailing transport path and the absence of the workloadIdentityDetails key.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture acc-gateway-rate-limit-batch-and-update, step setup#2 — CreateGateway with protocolType MCP; compare $.gatewayUrl with the recorded endpoint.
3. Cause 2/3: fixture acc-gateway-update-and-target-errors, steps task#5 (UpdateGateway) and task#6 (GetGateway) on that gateway; compare $.gatewayUrl.
4. Cause 4: fixture bedrock-agentcore-control-gateway-lifecycle-basic, step setup#1 — CreateGateway; check whether $.workloadIdentityDetails is present in the 202 body.

## Resolution

Causes 1, 2 and 3 are fixed: `gatewayUrl` is now composed as the gateway's endpoint host
plus the transport path of the protocol in its `protocolType` (`/mcp` for MCP, the model's
only value and its default), at the single composition point every gateway response is
rendered from, so create, update and read report one value and the URL the service
advertises is the URL it serves. Fixtures `acc-gateway-rate-limit-batch-and-update` and
`bedrock-agentcore-control-gateway-get-returns-metadata` replay clean, and
`acc-gateway-update-and-target-errors` gets past its gateway steps (task#5/#6) to an
unrelated `UpdateGatewayTarget` divergence at task#7.

Cause 4 is DECLINED on the recorded responses. Removing `workloadIdentityDetails` from the
create envelope was implemented and then reverted, because replay showed the recorded
`CreateGateway` responses of `acc-gateway-rate-limit-batch-and-update#setup#2` and
`acc-gateway-update-and-target-errors#setup#2` DO carry the block — omitting it diverges
those units, including the step this issue is bound to. The member's only content is a
per-run, server-minted workload-identity ARN that already has its modelled form and is
stable across reads and unique per gateway.

Conflicting fixture, kept on the record: `bedrock-agentcore-control-gateway-lifecycle-basic`
recorded the OPPOSITE of the other three units on exactly these two members — a path-less
`gatewayUrl` and no `workloadIdentityDetails` (see
`20261023T004131Z-bedrock-agentcore-control-gateway-responses-serialize-timestamps-as-epoch-numbers-and.md`,
filed from that same fixture). One response cannot satisfy both recordings, so the
implementation follows this issue's bound unit and the majority of units, which is also the
endpoint form AWS publishes for an MCP gateway; that fixture still diverges at
`$.gatewayUrl` and `$.workloadIdentityDetails.workloadIdentityArn`.
