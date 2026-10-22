Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the kafkaconnect emulator

_Filed: 2026-10-21T21:08:39Z_
_Operation: service-com-amazonaws-kafkaconnect-kafkaconnect | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 19 KafkaConnect operations are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state, so no client workflow works. Implement a FIRST, SMALL stateful slice: the custom-plugin + connector lifecycle, which is the service's core loop. Protocol is `restJson1` (SDK id `KafkaConnect`, signing name `kafkaconnect`).

Happy path to implement, in order:

1. **CreateCustomPlugin** — `POST /v1/custom-plugins`. Input: `name` (required, 1..128), `contentType` (required, JAR|ZIP), `location` (CustomPluginLocation, required), `description`, `tags`. Must mint a deterministic `customPluginArn` (`arn:aws:kafkaconnect:<region>:<account>:custom-plugin/<name>/<uuid>`), store the plugin, and return `customPluginArn`, `name`, `customPluginState` (ACTIVE once stored), `revision` (1). Duplicate `name` -> `ConflictException` (409).
2. **CreateConnector** — `POST /v1/connectors`. Input: `connectorName` (required, 1..128), `capacity` (required), `connectorConfiguration` (required map<string,string>), `kafkaCluster`, `kafkaClusterClientAuthentication`, `kafkaClusterEncryptionInTransit`, `kafkaConnectVersion`, `plugins` (required list<Plugin>, each referencing a stored custom-plugin ARN+revision), `serviceExecutionRoleArn` (required), plus optional `connectorDescription`, `logDelivery`, `networkType`, `workerConfiguration`, `tags`. Must persist the WHOLE input, mint `connectorArn`, set `currentVersion` (e.g. "1"), record `creationTime`, and return `connectorArn`, `connectorName`, `connectorState` (RUNNING is acceptable for the emulator; CREATING->RUNNING also fine as long as DescribeConnector agrees). Duplicate `connectorName` -> `ConflictException`; `plugins` naming an unknown custom-plugin ARN -> `NotFoundException`; missing required member -> `BadRequestException` (400).
3. **DescribeConnector** — `GET /v1/connectors/{connectorArn}`. Must read the stored connector and echo back from state, NOT from the request: `connectorArn`, `connectorName`, `connectorDescription`, `connectorConfiguration`, `capacity` (as CapacityDescription), `kafkaCluster*` descriptions, `kafkaConnectVersion`, `networkType`, `plugins` (as PluginDescription), `serviceExecutionRoleArn`, `creationTime`, `currentVersion`, `connectorState`. Unknown ARN -> `NotFoundException` (404).
4. **ListConnectors** — `GET /v1/connectors`. Must return one `ConnectorSummary` per stored connector (arn, name, state, creationTime, currentVersion, capacity, kafkaConnectVersion), honour `connectorNamePrefix` filtering, honour `maxResults` (1..100) and emit/consume an opaque `nextToken` so paging through two pages returns each connector exactly once.
5. **DeleteConnector** — `DELETE /v1/connectors/{connectorArn}` (optional `currentVersion` query). Must remove the connector from state and return `connectorArn` + `connectorState` = DELETING. A subsequent DescribeConnector/ListConnectors must no longer see it; deleting an unknown ARN -> `NotFoundException`.
6. **DeleteCustomPlugin** — `DELETE /v1/custom-plugins/{customPluginArn}`. Must remove the plugin and return `customPluginArn` + `customPluginState` = DELETING; unknown ARN -> `NotFoundException`. (DescribeCustomPlugin / ListCustomPlugins are cheap reads over the same map and are welcome in the same slice.)

Every one of these must compute its response from persisted state. No canned responses, no echoing the request back, no minting an ARN without storing the resource — a create followed by a describe/list must return exactly what was created, and a delete must be observable by the next read.

## State Model

In `src/state.rs` (`ComAmazonawsKafkaconnectKafkaconnectState`, reached via `crate::state::service_state(&*self.engine)`):

- `custom_plugins: HashMap<String /* customPluginArn */, CustomPlugin>` where `CustomPlugin { arn, name, description, content_type (JAR|ZIP), location, state (ACTIVE|CREATING|DELETING|...), revision: i64, creation_time, tags: HashMap<String,String> }`
- `plugin_names: HashMap<String /* name */, String /* arn */>` (or an equivalent lookup) for the duplicate-name ConflictException check
- `connectors: HashMap<String /* connectorArn */, Connector>` where `Connector { arn, name, description, configuration: HashMap<String,String>, capacity, kafka_cluster, kafka_cluster_client_authentication, kafka_cluster_encryption_in_transit, kafka_connect_version, network_type, plugins: Vec<PluginRef{ custom_plugin_arn, revision }>, service_execution_role_arn, worker_configuration: Option<...>, log_delivery: Option<...>, state (CREATING|RUNNING|UPDATING|DELETING|FAILED), current_version: String, creation_time, tags: HashMap<String,String> }`
- `connector_names: HashMap<String /* connectorName */, String /* arn */>` for the ConflictException check
- a monotonic counter/uuid source for ARN suffixes and `current_version` bumps

Out of scope for this first slice (stay 501, filed separately later): CreateWorkerConfiguration/DescribeWorkerConfiguration/DeleteWorkerConfiguration/ListWorkerConfigurations, UpdateConnector, RestartConnector, DescribeConnectorOperation, ListConnectorOperations, TagResource/UntagResource/ListTagsForResource.

Done = an integration test driving steps 1-6 against the emulator passes with correct, stateful response bodies (not merely 2xx).

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/` matches all 19 handler files (create_connector.rs, create_custom_plugin.rs, describe_connector.rs, list_connectors.rs, delete_connector.rs, delete_custom_plugin.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and `src/state.rs` holds no connector or plugin structures. Operation contracts (routes, required members, output members, ConflictException 409 / NotFoundException 404 / BadRequestException 400, and the @paginated(nextToken, items=connectors) trait on ListConnectors) are taken from services/service-com-amazonaws-kafkaconnect-kafkaconnect/OPERATIONS.md, rendered from the Smithy model com-amazonaws-kafkaconnect-kafkaconnect.json.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK kafkaconnect client at it.
2. CreateCustomPlugin(name='demo-plugin', contentType='ZIP', location={s3Location:{bucketArn, fileKey}}) -> expect 200 with customPluginArn, customPluginState=ACTIVE, revision=1; today: HTTP 501 NotImplemented.
3. CreateConnector(connectorName='demo-connector', capacity={autoScaling|provisionedCapacity}, connectorConfiguration={'connector.class':'...'}, kafkaCluster={apacheKafkaCluster:{bootstrapServers, vpc}}, kafkaClusterClientAuthentication={authenticationType:'NONE'}, kafkaClusterEncryptionInTransit={encryptionType:'PLAINTEXT'}, kafkaConnectVersion='2.7.1', plugins=[{customPlugin:{customPluginArn:<from step 2>, revision:1}}], serviceExecutionRoleArn='arn:aws:iam::123456789012:role/demo') -> expect 200 with connectorArn and connectorState.
4. DescribeConnector(connectorArn=<from previous step>) -> expect the stored connectorName, connectorConfiguration, plugins, serviceExecutionRoleArn, creationTime and currentVersion to come back unchanged.
5. ListConnectors() and ListConnectors(connectorNamePrefix='demo', maxResults=1) -> expect the connector present, prefix filter honoured, and nextToken paging returning each connector exactly once.
6. CreateConnector again with connectorName='demo-connector' -> expect ConflictException (HTTP 409).
7. DeleteConnector(connectorArn=...) -> expect 200 with connectorState=DELETING, then DescribeConnector on the same ARN -> NotFoundException (404) and ListConnectors no longer lists it.
8. DeleteCustomPlugin(customPluginArn=...) -> expect 200 with customPluginState=DELETING; DescribeCustomPlugin on the same ARN -> NotFoundException (404).
