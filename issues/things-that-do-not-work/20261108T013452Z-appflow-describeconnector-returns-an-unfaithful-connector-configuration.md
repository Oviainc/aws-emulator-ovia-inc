Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-discover-connectors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-discover-connectors
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.isSupportedApiVersionsRequired
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.supportedDestinationConnectors[*]
Issue-Commit: 034d67882baedb5546cc15e58be35e810f93ad27

# AppFlow DescribeConnector returns an unfaithful connector configuration

> _Note: this issue was identified against the code base at commit `034d67882baedb5546cc15e58be35e810f93ad27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnector

## Description
AppFlow's DescribeConnector answers 200 but the ConnectorConfiguration it projects for a built-in connector is not the one AWS serves: one boolean member of the configuration is absent entirely, and the supportedDestinationConnectors list carries different connector tokens at two positions. The shared fault is the emulator's static connector catalog / DescribeConnector response projection, not request handling. 2 causes.

Cause 1 of 2: connectorConfiguration omits the isSupportedApiVersionsRequired boolean.
AWS: recorded behavior only - the ConnectorConfiguration structure in this checkout's Smithy model (services/service-com-amazonaws-appflow-sandstoneconfigurationservicelambda) models canUseAsSource, canUseAsDestination, isPrivateLinkEnabled and isPrivateLinkEndpointUrlRequired as Boolean members with @default false, plus supportedApiVersions (SupportedApiVersionList), but does NOT model isSupportedApiVersionsRequired at all. The recording shows AWS serving that member on DescribeConnector's connectorConfiguration as a present JSON boolean, false for a connector that does not require the caller to pin an API version; it is a serve-always flag like the other ConnectorConfiguration booleans, false unless the connector requires an API version to be supplied. Hypothesis: the member is newer than the model version vendored in this repo.
Emulator: 200, and connectorConfiguration is otherwise populated, but $.connectorConfiguration.isSupportedApiVersionsRequired is missing from the body (replay reports expected False, got None), so the key is absent rather than false.
Why: a client that reads the flag to decide whether it must pass an apiVersion when it builds a connector profile or a flow sees no key. SDKs that model the member as a non-optional boolean fall back to a default (or fail to deserialize, depending on language), and discovery code that branches on it takes the wrong branch, so a connector that needs no apiVersion can be treated as if it did, or vice versa. Because the response is otherwise well formed and 200, nothing signals the omission.
Fix: the appflow DescribeConnector handler's connector-catalog data and its ConnectorConfiguration serializer - the flag must be emitted for every connector the catalog describes (and the same member must be emitted by DescribeConnectors, which projects the same ConnectorConfiguration shape into connectorConfigurations).
Traces: appflow-discover-connectors#task#2 (DescribeConnector, 200 vs 200, key $.connectorConfiguration.isSupportedApiVersionsRequired).

Cause 2 of 2: supportedDestinationConnectors carries the wrong connector tokens at two positions.
AWS: Smithy: DescribeConnector (POST /describe-connector, 200) output connectorConfiguration -> ConnectorConfiguration.supportedDestinationConnectors targets ConnectorTypeList (@length 0..100) of ConnectorType, documented "Lists the connectors that are available for use as destinations". Recorded behavior for the contents: AWS serves a fixed, per-connector list of destination tokens in a stable order, and the tokens are the wire values AWS itself emits - the recording has the token 'Locke' at index 5 and 'CustomConnector' at index 12 of that list. The requirement is the list AWS serves for that connector, element for element and in AWS's order, using AWS's wire spelling of each connector token (which, for this element, is not the ConnectorType value spelled in the vendored enum).
Emulator: 200, a list of the same length, but index 5 is 'LookoutMetrics' where AWS serves 'Locke', and index 12 is 'EventBridge' where AWS serves 'CustomConnector'. So the emulator substitutes a different spelling for one destination token and orders/populates another position differently; the remaining elements match.
Why: supportedDestinationConnectors is the capability-discovery answer a client uses before it creates a flow - it checks membership to decide whether a destination is allowed, and it feeds the token straight back into CreateFlow's destinationFlowConfigList connectorType. A token AWS never emits ('LookoutMetrics' here) makes a membership test for AWS's token fail, and a token the emulator omits ('CustomConnector') makes a legitimate destination look unsupported, so a client refuses a flow AWS would accept or builds one with a connectorType string the real service would reject. Index-based readers get a different connector than AWS reports at that position.
Fix: the appflow DescribeConnector handler's static connector-catalog data (the per-connector supportedDestinationConnectors table) - it must reproduce AWS's destination list for each connector, in AWS's order and with AWS's wire token spellings, and the same table must back DescribeConnectors.
Traces: appflow-discover-connectors#task#2 (DescribeConnector, 200 vs 200, keys $.connectorConfiguration.supportedDestinationConnectors[5] and [12]).

Related open issues, none of which covers this surface: 20261029T192701Z-appflow-error-message-text-diverges-from-aws-wording.md (error wording), 20261105T081532Z-appflow-read-paths-still-misreport-the-flow-principal.md (DescribeFlow/ListFlows flow projection), 20261029T194452Z-appflow-flow-lifecycle-wrong-initial-flowstatus-executions-always.md (flow lifecycle).

## Evidence
Fixture appflow-discover-connectors, step task#2, operation DescribeConnector (POST /describe-connector), expected status 200, actual status 200; body diverges on three keys in the single connectorConfiguration structure:
- $.connectorConfiguration.isSupportedApiVersionsRequired: expected the boolean False (member present), got None - the key is absent from the emulator's response. Fixed part = the member name and its boolean type; the value (false here) comes from the described connector's capabilities.
- $.connectorConfiguration.supportedDestinationConnectors[5]: expected 'Locke', got 'LookoutMetrics'.
- $.connectorConfiguration.supportedDestinationConnectors[12]: expected 'CustomConnector', got 'EventBridge'.
All other elements of supportedDestinationConnectors and all other configuration members matched, so the list length is right and only these two positions and the one missing boolean diverge.
Model basis (this checkout's appflow model): DescribeConnector input connectorType: ConnectorType @required, connectorLabel: ConnectorLabel; output connectorConfiguration: ConnectorConfiguration; errors ValidationException(400), ResourceNotFoundException(404), InternalServerException(500); http POST /describe-connector, code 200. ConnectorConfiguration.supportedDestinationConnectors: ConnectorTypeList (@length 0..100) of ConnectorType. ConnectorConfiguration has no isSupportedApiVersionsRequired member in this vendored model version, so that member is grounded in the recording only.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus). Fixture appflow-discover-connectors, step task#2 is reported as a mismatch with expected status 200 and actual status 200.
2. POST /describe-connector with a body naming a built-in connector, e.g. {"connectorType": "S3"} (the connectorType the fixture's step sends), signed for the emulator's appflow endpoint.
3. Cause 1: inspect the 200 response body and look for connectorConfiguration.isSupportedApiVersionsRequired. The key is absent; AWS serves it as a boolean (false for this connector).
4. Cause 2: inspect connectorConfiguration.supportedDestinationConnectors. Element 5 is 'LookoutMetrics' where AWS serves 'Locke', and element 12 is 'EventBridge' where AWS serves 'CustomConnector'.
5. The same ConnectorConfiguration projection is reachable via POST /describe-connectors (connectorConfigurations map), so check it there too after fixing.
