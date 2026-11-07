Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-discover-connectors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-discover-connectors
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.connectorDescription
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.isSupportedApiVersionsRequired
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.supportedDataTransferTypes
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.supportedDestinationConnectors[*]
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.supportedOperators
Issue-Key: appflow|DescribeConnector|2xx->2xx|$.connectorConfiguration.supportedWriteOperations
Issue-Commit: 39e133812369a94b2cea1b65d870300828e6b9f2

# AppFlow DescribeConnector returns a synthetic connector configuration instead of the per-connector catalog

> _Note: this issue was identified against the code base at commit `39e133812369a94b2cea1b65d870300828e6b9f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnector

## Description
DescribeConnector answers 200 but the ConnectorConfiguration it serves is a single generic, hand-rolled object rather than the catalog entry for the requested connectorType. One cause, with several symptoms that all come from that one synthetic record.

Cause 1 of 1: DescribeConnector's ConnectorConfiguration is not per-connector-type.

AWS: DescribeConnector (POST /describe-connector, 200, input DescribeConnectorRequest{connectorType required, connectorLabel}, errors ValidationException 400 / ResourceNotFoundException 404 / InternalServerException 500) returns output member connectorConfiguration of shape ConnectorConfiguration. Smithy: ConnectorConfiguration.connectorDescription (ConnectorDescription, @length 0..2048, pattern ^[\w!@#\-.?,\s]*$) is 'A description about the connector'; supportedDestinationConnectors (ConnectorTypeList, @length 0..100) 'Lists the connectors that are available for use as destinations'; supportedOperators (SupportedOperatorList), supportedWriteOperations (SupportedWriteOperationList) and supportedDataTransferTypes (SupportedDataTransferTypeList) are all OPTIONAL members with no default, so AWS omits them for a connector that does not expose them. Every one of these is a property OF THE REQUESTED CONNECTOR, so the value set has to be selected by connectorType, not shared. For the member isSupportedApiVersionsRequired the Smithy model says nothing (not present in ConnectorConfiguration) - recorded behavior only: for the built-in S3 connector the recording shows it present and false. Likewise recorded behavior only for the concrete per-connector values: for S3 the recording shows connectorDescription 'Amazon S3', supportedDestinationConnectors carrying 13 entries including a Snowflake/Redshift/S3/Honeycode/Locke/CustomerProfiles/Zendesk/Marketo/CustomConnector/Upsolver ordering, and supportedOperators / supportedWriteOperations / supportedDataTransferTypes ABSENT.

Emulator: for the same request it returns one fixed configuration. connectorDescription is absent (null) where the connector has a description; isSupportedApiVersionsRequired is absent (null) where the recording carries false; supportedDestinationConnectors is a different set in a different order and one element SHORT (12 entries where 13 are expected - index 12 missing, and indices 1..10 each hold a different ConnectorType value than expected); and supportedOperators, supportedWriteOperations and supportedDataTransferTypes are POPULATED (supportedDataTransferTypes ['RECORD','FILE'], supportedOperators a 21-element PROJECTION/LESS_THAN/.../NO_OP list) where AWS omits them entirely for this connector. Status matches (200 -> 200); only the body diverges.

Why: DescribeConnector is the capability-discovery call. A client uses supportedDestinationConnectors to decide which destinationConnectorType a CreateFlow may legally name, supportedOperators / supportedWriteOperations to decide which task operators and write operations it may emit, and supportedDataTransferTypes to pick a transfer mode. Against this emulator that discovery is wrong in both directions: a destination AWS supports is reported unsupported (so the client refuses a flow AWS would accept), a destination AWS does not support is reported supported (so the client builds a flow AWS would reject), and operator/write-operation/transfer-type capability is advertised for a connector that does not advertise it, so generated task configurations are validated against a capability set that does not exist. Clients that key UI or codegen off connectorDescription also render nothing.

Fix: the DescribeConnector handler in the appflow service crate, specifically the connector-catalog lookup that builds ConnectorConfiguration - it must key off the request's connectorType (and connectorLabel for a registered custom connector) and return that connector's own record, with members AWS omits for a connector left unset rather than filled from a shared template. Shared defaults belong per-connector in that catalog table, not in one literal.

Traces: appflow-discover-connectors#task#2 (DescribeConnector, expected 200 / actual 200, body keys $.connectorConfiguration.connectorDescription, .isSupportedApiVersionsRequired, .supportedDataTransferTypes, .supportedDestinationConnectors[1..10] and [12], .supportedOperators, .supportedWriteOperations).

Not covered by the open appflow issues: 20261029T192701Z-appflow-describeflow-and-listflows-omit-flow-state-fidelity.md and 20261029T194452Z-appflow-flow-lifecycle-wrong-initial-flowstatus-executions-always.md are about flow state and lifecycle, and 20261029T192701Z-appflow-error-message-text-diverges-from-aws-wording.md is about error message wording; none touches the connector catalog served by DescribeConnector.

## Evidence
Fixture appflow-discover-connectors, step task#2, operation DescribeConnector, verdict mismatch, expected_status 200, actual_status 200.

Diverging body keys (expected = recorded AWS, got = emulator):
- $.connectorConfiguration.connectorDescription: expected 'Amazon S3', got None (missing)
- $.connectorConfiguration.isSupportedApiVersionsRequired: expected False, got None (missing)
- $.connectorConfiguration.supportedDataTransferTypes: expected absent (None), got ['RECORD', 'FILE'] (type_mismatch)
- $.connectorConfiguration.supportedOperators: expected absent (None), got a 21-element list ['PROJECTION','LESS_THAN','GREATER_THAN','CONTAINS','BETWEEN','LESS_THAN_OR_EQUAL_TO','GREATER_THAN_OR_EQUAL_TO','EQUAL_TO','NOT_EQUAL_TO','ADDITION','MULTIPLICATION','DIVISION','SUBTRACTION','MASK_ALL','MASK_FIRST_N','MASK_LAST_N','VALIDATE_NON_NULL','VALIDATE_NON_ZERO','VALIDATE_NON_NEGATIVE','VALIDATE_NUMERIC','NO_OP'] (type_mismatch)
- $.connectorConfiguration.supportedWriteOperations: expected absent, got populated (type_mismatch)
- $.connectorConfiguration.supportedDestinationConnectors: element-wise mismatch at [1]..[10] and a missing [12] - expected [1]='Snowflake' got 'Redshift'; [2]='Redshift' got 'S3'; [3]='S3' got 'Marketo'; [4]='Honeycode' got 'Zendesk'; [5]='Locke' got 'Snowflake'; [6]='CustomerProfiles' got 'EventBridge'; [7]='Zendesk' got 'LookoutMetrics'; [8]='Marketo' got 'Upsolver'; [9]='CustomConnector' got 'Honeycode'; [10]='Upsolver' got 'CustomerProfiles'; [12]='CustomConnector' got None (list is one element short)

The concrete strings above are this recording's values for the S3 connector and fill the template; the requirement is that each member be the requested connector's own value and that members AWS does not return for a connector be omitted, not emitted from a shared default.

Smithy basis: DescribeConnector -> DescribeConnectorResponse.connectorConfiguration : ConnectorConfiguration; connectorDescription @length 0..2048 pattern ^[\w!@#\-.?,\s]*$; supportedDestinationConnectors : ConnectorTypeList @length 0..100 over the ConnectorType enum; supportedOperators / supportedWriteOperations / supportedDataTransferTypes are optional, undefaulted list members. isSupportedApiVersionsRequired is absent from the model (recorded behavior only).

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm /__admin/health answers 200.
2. Replay fixture appflow-discover-connectors and stop at step task#2, or issue the step by hand:
   POST /describe-connector with body {"connectorType": "S3"} signed for the appflow endpoint.
3. Inspect the 200 response's connectorConfiguration object:
   - connectorDescription and isSupportedApiVersionsRequired are absent; AWS returns the connector's description and false.
   - supportedOperators, supportedWriteOperations and supportedDataTransferTypes are present; AWS omits them for this connector.
   - supportedDestinationConnectors has 12 entries whose values from index 1 onward differ from AWS's 13-entry list for this connector.
4. Repeat with a different connectorType (e.g. "Salesforce") and observe the SAME configuration body comes back, which is the underlying cause: the response is not selected by connectorType.
