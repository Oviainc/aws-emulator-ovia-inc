Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds kafka.kafka-produce-fetch-in-vpc kafka.kafka-consumer-group-offsets-survive-a-rebalance-and-no-record-is-redelivered kafka.kafka-explicit-partition-key-routing-preserves-per-key-order kafka.kafka-iam-auth-produce-consume-round-trip-over-the-native-protocol kafka.kafka-record-above-max-message-bytes-rejected-and-a-just-under-record-round-trips kafka.msk-cluster-configuration-num-partitions-default-is-applied-by-the-broker kafka.msk-iam-group-scope-denies-a-consumer-group-join-while-an-assigned-read-succeeds kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse kafka.msk-serverless-refuses-a-scram-client-and-serves-the-iam-client-on-the-same-socket kafka.topic-partition-key-hash-consistent-routing kafka.topic-partition-key-ordering-preserved kafka.topic-partition-message-order-preserved kafka.topic-partition-message-ordering-preserved kafka.e2e-cluster-scram-auth-produce-consume-teardown kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown kafka.e2e-cluster-topic-replication-consequence kafka.e2e-replicator-cross-cluster-topic-sync-teardown
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets|_await_data_plane#1|TimeoutError|raise TimeoutError(f"{bootstrap} accepting a TCP connection: the run budget ended first; not graded")
Issue-Key: kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds|_await_data_plane#1|TimeoutError|raise TimeoutError(f"{bootstrap} accepting a TCP connection: the run budget ended first; not graded")
Issue-Key: kafka.kafka-produce-fetch-in-vpc|run#24|TimeoutError|raise TimeoutError(f"a Kafka produce/fetch round trip on cluster {cluster_name}: the run budget ended first; not graded")
Issue-Key: kafka.kafka-consumer-group-offsets-survive-a-rebalance-and-no-record-is-redelivered|_await_data_plane#1|Timeout|time.sleep(15)
Issue-Key: kafka.kafka-explicit-partition-key-routing-preserves-per-key-order|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.kafka-iam-auth-produce-consume-round-trip-over-the-native-protocol|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.kafka-record-above-max-message-bytes-rejected-and-a-just-under-record-round-trips|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.msk-cluster-configuration-num-partitions-default-is-applied-by-the-broker|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.msk-iam-group-scope-denies-a-consumer-group-join-while-an-assigned-read-succeeds|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse|_await_data_plane#1|Timeout|time.sleep(15)
Issue-Key: kafka.msk-serverless-refuses-a-scram-client-and-serves-the-iam-client-on-the-same-socket|_await_data_plane#1|Timeout|time.sleep(2)
Issue-Key: kafka.topic-partition-key-hash-consistent-routing|_grant_scram_acls#3|Timeout|admin = KafkaAdminClient(
Issue-Key: kafka.topic-partition-key-ordering-preserved|_grant_scram_acls#14|Timeout|admin = KafkaAdminClient(
Issue-Key: kafka.topic-partition-message-order-preserved|_grant_scram_acls#3|Timeout|admin = KafkaAdminClient(
Issue-Key: kafka.topic-partition-message-ordering-preserved|_wait_cluster_active#4|Timeout|info = kafka.describe_cluster_v2(ClusterArn=cluster_arn)["ClusterInfo"]
Issue-Key: kafka.e2e-cluster-scram-auth-produce-consume-teardown|_grant_scram_acls#13|Timeout|admin = KafkaAdminClient(
Issue-Key: kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown|_grant_scram_acls#15|Timeout|admin = KafkaAdminClient(
Issue-Key: kafka.e2e-cluster-topic-replication-consequence|_wait_for_replicator_running#17|Timeout|time.sleep(interval_s)
Issue-Key: kafka.e2e-replicator-cross-cluster-topic-sync-teardown|_wait_replicator_state#15|Timeout|time.sleep(interval_s)
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# No Kafka broker answers on the advertised bootstrap endpoints: every native produce/consume, ordering, transaction and open-monitoring check never connects

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBootstrapBrokers

## Description
Cause 1 of 1: the emulator advertises bootstrap broker endpoints but nothing accepts a TCP connection on them, so the entire MSK data plane is unreachable and no check that produces or consumes a record reaches a verdict.

AWS: an ACTIVE MSK cluster serves the Kafka wire protocol on the endpoints GetBootstrapBrokers advertises — port 9092 (plaintext), 9094 (TLS), 9096 (SASL/SCRAM) and 9098 (SASL/IAM) for provisioned clusters, and the serverless SASL/IAM port for serverless clusters; a TCP connection to an advertised host:port is accepted and completes the Kafka handshake, while a port the cluster does not advertise refuses. On that socket a producer's records land with monotonically increasing per-partition offsets, consumer groups commit and resume offsets across a rebalance without redelivery, records with the same key keep per-key order in one partition, a record above max.message.bytes is rejected while a just-under record round-trips, broker-side configuration such as num.partitions is applied, IAM/SCRAM client authentication is enforced per scheme on the socket, aborted transactions are invisible to read_committed and visible to read_uncommitted, and open monitoring serves Prometheus on the enabled exporter's port while the disabled exporter's port refuses. (Recorded behavior for the data-plane semantics; the control plane's model only says the endpoints exist.)
Emulator: the advertised endpoint never accepts a connection. Three checks localize this precisely — two report 'TimeoutError: 127.0.0.1:9098 accepting a TCP connection: the run budget ended first' and one reports 'TimeoutError: a Kafka produce/fetch round trip on cluster <name>: the run budget ended first'. The remaining checks in this family report only 'Check timed out after 60.0s' with no frame, which is the same symptom: they block on the first produce/consume against the advertised address. Because the emulator announces an address that nothing serves, this is the emulator's own egress gap, not the runner's.
Why: every behaviour a Kafka client actually depends on is unavailable and unverifiable: a client that configures itself from GetBootstrapBrokers hangs until its own timeout, and producers, consumer groups, partitioning, size limits, transactions, per-scheme authentication and the monitoring surface are all unexercised. An application cannot be developed or tested against this emulator's MSK beyond the control plane.
Fix: the kafka service must serve the Kafka wire protocol on the ports it advertises (or advertise only endpoints it serves) — the broker listener and its per-scheme authentication, plus the record/partition/offset state the produce and fetch paths read and write, and the open-monitoring exporter endpoints.
Traces: the 20 checks listed in trace_ids; kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets, kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds and kafka.kafka-produce-fetch-in-vpc carry the TCP/round-trip frames quoted above, the rest report the bare 60 s timeout.

## Evidence
All 20 checks are kafka behavioral checks (verdict mismatch or error); none is marked ALREADY FILED or OWNED.

With a localized frame:
  kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets — 'TimeoutError: 127.0.0.1:9098 accepting a TCP connection: the run budget ended first; not graded' (raised from the check's _bring_up).
  kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds — same TimeoutError on 127.0.0.1:9098.
  kafka.kafka-produce-fetch-in-vpc — 'TimeoutError: a Kafka produce/fetch round trip on cluster sim2real-behav-msk-invpc-<id>: the run budget ended first; not graded'.
  (The host/port shown is where this run reached the emulator; the requirement is that the advertised endpoint accepts a connection, whatever the address.)

Reporting only 'Error: Check timed out after 60.0s', all of them blocked on a native produce/consume or exporter socket against the advertised endpoint: kafka.kafka-consumer-group-offsets-survive-a-rebalance-and-no-record-is-redelivered, kafka.kafka-explicit-partition-key-routing-preserves-per-key-order, kafka.kafka-iam-auth-produce-consume-round-trip-over-the-native-protocol, kafka.kafka-record-above-max-message-bytes-rejected-and-a-just-under-record-round-trips, kafka.msk-cluster-configuration-num-partitions-default-is-applied-by-the-broker, kafka.msk-iam-group-scope-denies-a-consumer-group-join-while-an-assigned-read-succeeds, kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses, kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse, kafka.msk-serverless-refuses-a-scram-client-and-serves-the-iam-client-on-the-same-socket, kafka.topic-partition-key-hash-consistent-routing, kafka.topic-partition-key-ordering-preserved, kafka.topic-partition-message-order-preserved, kafka.topic-partition-message-ordering-preserved, kafka.e2e-cluster-scram-auth-produce-consume-teardown, kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown, kafka.e2e-cluster-topic-replication-consequence, kafka.e2e-replicator-cross-cluster-topic-sync-teardown.

Expected (AWS) for the family: a TCP connect to the advertised bootstrap host:port is accepted and the Kafka handshake completes, so each check's produce/consume assertions can be evaluated. Actual: no connection is ever accepted, so none of them produced a verdict.

## Steps to Reproduce
1. Boot the emulator and create an ACTIVE PROVISIONED MSK cluster with Sasl.Iam enabled (and a second with Sasl.Scram), plus a serverless cluster.
2. Call GetBootstrapBrokers and take the advertised BootstrapBrokerStringSaslIam (and SaslScram) host:port pairs.
3. Open a plain TCP connection to each advertised host:port. Assert the connection is ACCEPTED (today it never is), and assert a port the cluster does NOT advertise refuses.
4. Over the accepted socket, with a Kafka client: create a topic, produce N records and assert distinct monotonic per-partition offsets; consume them in a group, commit, force a rebalance and assert no record is redelivered; produce keyed records and assert per-key order within a partition; produce a record above max.message.bytes and assert it is rejected while a just-under record round-trips; begin a transaction, abort it, and assert the records are invisible to read_committed and visible to read_uncommitted.
5. With open monitoring enabled for the JMX exporter only, scrape the JMX exporter port and assert Prometheus output, and assert the node-exporter port refuses.
6. Replay the 20 checks in trace_ids.
