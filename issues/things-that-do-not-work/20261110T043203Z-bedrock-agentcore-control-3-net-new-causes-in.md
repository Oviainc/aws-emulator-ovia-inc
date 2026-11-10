Issue-Source: trace-sweep-divergence
Issue-Trace: acc-builtin-browser-and-code-interpreter-inventory
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-builtin-browser-and-code-interpreter-inventory bedrock-agentcore-control-dataset-not-found-errors acc-gateway-update-and-target-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|ListBrowsers|2xx->2xx|$.browserSummaries[*]
Issue-Key: bedrock-agentcore-control|CreateDatasetVersion|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|UpdateGateway|2xx->2xx|$.gatewayUrl
Issue-Commit: 97e87117743d236dfa23fd76b444a194861aeef0

# bedrock-agentcore-control: 3 net-new causes in the built-in browser inventory, dataset-version not-found message and UpdateGateway gatewayUrl

> _Note: this issue was identified against the code base at commit `97e87117743d236dfa23fd76b444a194861aeef0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBrowsers

## Description
Three unrelated net-new divergences on service bedrock-agentcore-control, bundled because they share the service and one gate verdict. 3 causes.

Cause 1 of 3: ListBrowsers misrepresents the AWS built-in browser `aws.browser.v1`.
AWS: ListBrowsers returns the service-owned built-in browser as a BrowserSummary whose `browserArn` carries the literal account segment `aws` (Smithy BrowserArn pattern `^arn:aws(-[^:]+)?:bedrock-agentcore:[a-z0-9-]+:(aws|[0-9]{12}):browser(-custom)?/(aws\\.browser\\.v1|...)$` — the `aws` alternative exists exactly for this built-in), whose `name` is the service's display name for the tool, and which carries a `description` of the sandbox. The recording shows it with no createdAt/lastUpdatedAt members at all; name/description/ARN-account are fixed service-owned values, not values derived from the caller.
Emulator: returns the built-in with `browserArn` scoped to the CALLER's account id instead of `aws`, a `name` derived from the id (underscored form of the browser id) instead of the service's display name, NO `description`, and extra `createdAt`/`lastUpdatedAt` epoch-number members AWS does not send for the built-in.
Why: a client that selects the built-in sandbox by its documented ARN (`...:aws:browser/aws.browser.v1`) or by its display name finds no match and falls through to "no browser available"; code that renders the inventory shows an id-shaped name and no description.
Fix: the ListBrowsers handler's built-in/seeded browser entry (its ARN construction, name, description and timestamp emission).
Traces: acc-builtin-browser-and-code-interpreter-inventory#task#1.

Cause 2 of 3: CreateDatasetVersion's not-found refusal uses the generic emulator message instead of AWS's dataset message.
AWS: CreateDatasetVersion (POST /datasets/{datasetId}/versions) raises ResourceNotFoundException (404) for an unknown datasetId with the recorded message `The specified dataset does not exist. Please verify the datasetId and try again.` — entirely fixed text, no variable part.
Emulator: 404 with the emulator's generic resource prose `Failed to retrieve dataset because it doesn't exist. Retry the request with a different resource identifier.`
Why: a client that matches on AWS's message (or surfaces it to an operator) gets different text, and the generic wording does not name the datasetId field the caller must fix. The already-open issue 20261109T043845Z-bedrock-agentcore-control-dataset-surface-diverges-getdataset-not.md covers the GetDataset/CreateDataset clusters; it does not cover CreateDatasetVersion, whose cluster is unowned.
Fix: the CreateDatasetVersion handler's not-found error message.
Traces: bedrock-agentcore-control-dataset-not-found-errors#task#4.

Cause 3 of 3: UpdateGateway publishes a gatewayUrl on a host the emulator does not serve.
AWS: Smithy UpdateGateway.output.gatewayUrl (GatewayUrl, @length 1..1024) is the MCP endpoint a client then connects to; on AWS that host resolves and serves the gateway. The host is a per-environment value, so the requirement is only that the announced endpoint is one the serving environment actually answers on.
Emulator: returns a `https://<gateway-name>-<suffix>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp` URL — a real AWS DNS name that the emulator does not serve and cannot be reached on (replay verdict `host_not_own`), so it announces an address other than the one it is reached on.
Why: a client that takes gatewayUrl from UpdateGateway and opens an MCP session against it leaves the emulator entirely: the connection fails (or, worse, is attempted against real AWS DNS), so no gateway traffic is ever exercised.
Fix: the UpdateGateway handler's gatewayUrl construction, which must publish the emulator's own reachable endpoint. The already-open issue 20261102T055559Z-bedrock-agentcore-control-creategateway-and-getgateway-gateway-representation-diverges-gatewayurl-carries.md owns the CreateGateway/GetGateway clusters only; the UpdateGateway cluster is unowned.
Traces: acc-gateway-update-and-target-errors#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (296 steps, 143 passed, score 48.3%).

Cause 1 — ListBrowsers, expected 200 / got 200:
  $.browserSummaries[0]: expected {browserArn: 'arn:aws:bedrock-agentcore:us-west-2:aws:browser/aws.browser.v1', browserId: 'aws.browser.v1', description: 'AWS built-in browser sandbox for secure web browsing', name: 'AgentCore Browser Tool', status: 'READY'}, got {browserId: 'aws.browser.v1', browserArn: 'arn:aws:bedrock-agentcore:us-west-2:000000000000:browser/aws.browser.v1', name: 'aws_browser_v1', status: 'READY', createdAt: <epoch number>, lastUpdatedAt: <epoch number>} (account segment, name, missing description, extra timestamps)
Cause 2 — CreateDatasetVersion, expected 404 / got 404:
  $.message: expected 'The specified dataset does not exist. Please verify the datasetId and try again.', got "Failed to retrieve dataset because it doesn't exist. Retry the request with a different resource identifier."
Cause 3 — UpdateGateway, expected 202 / got 202:
  $.gatewayUrl: host_not_own — the emulator announced 'https://<name>-<suffix>.gateway.bedrock-agentcore.us-west-2.amazonaws.com/mcp', a host it does not serve.

Model citations: Smithy BrowserSummary {browserId, browserArn (pattern allows the `aws` account segment for aws.browser.v1), name (optional), description (optional, sensitive), status @required, createdAt/lastUpdatedAt DateTimestamp 'date-time'}; CreateDatasetVersion POST /datasets/{datasetId}/versions, errors include ResourceNotFoundException (404); UpdateGateway PUT /gateways/{gatewayIdentifier}/ code 202, output.gatewayUrl -> GatewayUrl @length 1..1024.

## Steps to Reproduce
1. ListBrowsers with no filter -> the aws.browser.v1 summary comes back with the caller's account id in its ARN, name 'aws_browser_v1', no description, and extra createdAt/lastUpdatedAt members.
2. CreateDatasetVersion on a datasetId that does not exist -> 404 whose message is the generic 'Failed to retrieve dataset...' prose instead of 'The specified dataset does not exist. Please verify the datasetId and try again.'
3. CreateGateway, wait for it, then UpdateGateway -> the 202 response's gatewayUrl names a *.gateway.bedrock-agentcore.<region>.amazonaws.com host the emulator does not serve; connecting to it does not reach the emulator.
Replay: fixture acc-builtin-browser-and-code-interpreter-inventory step task#1, plus the fixture#step bindings listed in trace_ids.
