Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the kafka (Amazon MSK) emulator

_Filed: 2026-10-21T21:08:48Z_
_Operation: service-com-amazonaws-kafka-kafka | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `service-com-amazonaws-kafka-kafka` is an untouched scaffold: all 64 files under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501 `NotImplemented`, and the service holds no state. This issue defines the FIRST buildable slice: the provisioned-cluster lifecycle, implemented statefully so a realistic MSK client script works end to end.

### Happy path (implement these 5 operations, in this order)

1. **CreateCluster** — `POST /v1/clusters` (restJson1). Input: `ClusterName` (required, len 1..64), `KafkaVersion` (required), `NumberOfBrokerNodes` (required, 1..15), `BrokerNodeGroupInfo` (required), plus optional `EncryptionInfo`, `EnhancedMonitoring`, `ConfigurationInfo`, `ClientAuthentication`, `LoggingInfo`, `StorageMode`, `Tags`. Must mint a deterministic ARN (`arn:aws:kafka:<region>:<account>:cluster/<ClusterName>/<uuid>-<n>`), persist the whole submitted configuration, and return `ClusterArn`, `ClusterName`, `State` = `CREATING` (or `ACTIVE`). A second CreateCluster with the same `ClusterName` must fail `ConflictException` (HTTP 409), not succeed twice.
2. **DescribeCluster** — `GET /v1/clusters/{ClusterArn}`. Returns `ClusterInfo` built FROM the stored record: `ClusterArn`, `ClusterName`, `State`, `CreationTime`, `CurrentVersion`, `NumberOfBrokerNodes`, `BrokerNodeGroupInfo`, `EncryptionInfo`, `EnhancedMonitoring`, `StorageMode`, `CurrentBrokerSoftwareInfo.KafkaVersion`, `Tags`, `ZookeeperConnectString`. An unknown/deleted ARN must return `NotFoundException` (HTTP 404).
3. **ListClusters** — `GET /v1/clusters`. Returns `ClusterInfoList` of the SAME `ClusterInfo` values DescribeCluster returns, honouring `ClusterNameFilter` (name prefix), `MaxResults` (1..100) and the opaque `NextToken` round trip (`@paginated(inputToken=NextToken, outputToken=NextToken, items=ClusterInfoList)`); `NextToken` absent on the last page.
4. **GetBootstrapBrokers** — `GET /v1/clusters/{ClusterArn}/bootstrap-brokers`. Must derive broker endpoint strings from the stored cluster (broker count and the cluster's security settings): a comma-separated `BootstrapBrokerString` with one `b-<n>...:9092` entry per persisted broker node, `BootstrapBrokerStringTls` on `:9094`, and `BootstrapBrokerStringSaslIam`/`BootstrapBrokerStringSaslScram` (`:9098`/`:9096`) only when the stored `ClientAuthentication` enables that mechanism. Unknown ARN → `NotFoundException`/`BadRequestException` per the model, never a canned string.
5. **DeleteCluster** — `DELETE /v1/clusters/{ClusterArn}` (optional `currentVersion` query). Returns `ClusterArn` and `State` = `DELETING`, and REMOVES the cluster from state: the following DescribeCluster must return `NotFoundException` (404) and ListClusters must no longer include it. When `currentVersion` is supplied and does not match the stored `CurrentVersion`, fail `BadRequestException` (400).

No canned or echoed responses: every one of the five must compute its response from the persisted record. A create followed by a describe must return exactly what was created; a delete must be observable by the next reader. Random-ARN-without-storage is a failure.

## State Model

Per-service state in `src/state.rs` (`ComAmazonawsKafkaKafkaState`), reached through `crate::state::service_state(&*self.engine)`:

- `clusters: BTreeMap<String /* ClusterArn */, Cluster>` — insertion-ordered-by-ARN registry, plus `cluster_arn_by_name: HashMap<String, String>` to enforce name uniqueness for the 409.
- `Cluster { arn: String, name: String, state: ClusterState /* CREATING|ACTIVE|DELETING|... */, cluster_type: ClusterType /* PROVISIONED */, kafka_version: String, current_version: String /* e.g. "K3AEGXETSR30VB" */, creation_time: SystemTime, number_of_broker_nodes: i32, broker_node_group_info: BrokerNodeGroupInfo, client_authentication: Option<ClientAuthentication>, encryption_info: Option<EncryptionInfo>, enhanced_monitoring: Option<EnhancedMonitoring>, open_monitoring: Option<OpenMonitoringInfo>, logging_info: Option<LoggingInfo>, storage_mode: Option<StorageMode>, configuration_info: Option<ConfigurationInfo>, tags: BTreeMap<String,String>, zookeeper_connect_string: String, brokers: Vec<BrokerNode> }`
- `BrokerNode { id: i32, endpoint_host: String /* b-<id>.<cluster>.<suffix>.kafka.<region>.amazonaws.com */ }` — the source GetBootstrapBrokers renders its strings from.
- `next_cluster_suffix: u64` — monotonic counter for ARN/endpoint suffix generation, so ARNs are unique and reproducible.

Pagination tokens for ListClusters are opaque encodings of the last returned ARN resolved against `clusters`, not an index into a freshly built list.

Operations outside this slice (ClusterV2 APIs, configurations, topics, channels, replicators, VPC connections, tagging, Update*/Reboot) stay 501 and are tracked separately as individual gap issues.

## Evidence

`services/service-com-amazonaws-kafka-kafka/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-kafka-kafka.json`): header says "64 operations", protocol `restJson1`, sigv4 name `kafka`. It gives CreateCluster `POST /v1/clusters` with required `ClusterName`/`KafkaVersion`/`NumberOfBrokerNodes`/`BrokerNodeGroupInfo` and output `ClusterArn`/`ClusterName`/`State`; DescribeCluster `GET /v1/clusters/{ClusterArn}` → `ClusterInfo` with `NotFoundException (HTTP 404)`; ListClusters `GET /v1/clusters` with `@paginated(inputToken=NextToken, outputToken=NextToken, items=ClusterInfoList)` and `ClusterNameFilter`/`MaxResults (1..100)`; GetBootstrapBrokers `GET /v1/clusters/{ClusterArn}/bootstrap-brokers` with the `BootstrapBrokerString*` members; DeleteCluster `DELETE /v1/clusters/{ClusterArn}` with `currentVersion` query and output `State`. `smithy_query shape=ClusterInfo` confirms the stored fields DescribeCluster must echo (`clusterArn`, `clusterName`, `state`, `creationTime`, `currentVersion`, `numberOfBrokerNodes`, `brokerNodeGroupInfo`, `currentBrokerSoftwareInfo`, `tags`, `zookeeperConnectString`, `storageMode`). All-stub status verified by `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-kafka-kafka/src/ops/` — every op file matches (create_cluster.rs, describe_cluster.rs, list_clusters.rs, get_bootstrap_brokers.rs, delete_cluster.rs included). No tests assert any of this behavior; only the generated `tests/state_roundtrip.rs` exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `kafka` client at it.
2. `aws kafka create-cluster --cluster-name demo --kafka-version 3.6.0 --number-of-broker-nodes 3 --broker-node-group-info '{"InstanceType":"kafka.m5.large","ClientSubnets":["subnet-a","subnet-b","subnet-c"]}'` → expect 200 with a `ClusterArn` for `demo` and `State` CREATING/ACTIVE (today: HTTP 501 NotImplemented).
3. `aws kafka describe-cluster --cluster-arn <arn>` → expect `ClusterInfo` whose `ClusterName` is `demo`, `NumberOfBrokerNodes` is 3, `CurrentBrokerSoftwareInfo.KafkaVersion` is 3.6.0, with a `CreationTime` and `CurrentVersion` — i.e. exactly what was created.
4. `aws kafka list-clusters` → expect the `demo` cluster in `ClusterInfoList`; repeat with `--cluster-name-filter demo` (match) and `--cluster-name-filter other` (empty), and with `--max-results 1` to verify `NextToken` round-trips over two clusters without repeats or omissions.
5. `aws kafka get-bootstrap-brokers --cluster-arn <arn>` → expect `BootstrapBrokerString` with 3 comma-separated `b-1/b-2/b-3` endpoints on port 9092 derived from the stored broker count.
6. `aws kafka create-cluster --cluster-name demo ...` again → expect `ConflictException` (HTTP 409).
7. `aws kafka delete-cluster --cluster-arn <arn>` → expect 200 with `State` DELETING.
8. `aws kafka describe-cluster --cluster-arn <arn>` → expect `NotFoundException` (HTTP 404), and `aws kafka list-clusters` no longer lists `demo`.
