Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.deploy-cfn-msk-cluster-active-and-reachable kafka.deploy-msk-cluster-cfn-active-and-bootstrap-brokers kafka.e2e-cluster-scram-auth-produce-consume-teardown kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown kafka.kafka-consumer-group-offsets-survive-a-rebalance-and-no-record-is-redelivered kafka.kafka-explicit-partition-key-routing-preserves-per-key-order kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets kafka.kafka-iam-auth-produce-consume-round-trip-over-the-native-protocol kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds kafka.kafka-produce-fetch-in-vpc kafka.kafka-record-above-max-message-bytes-rejected-and-a-just-under-record-round-trips kafka.msk-cluster-configuration-num-partitions-default-is-applied-by-the-broker kafka.msk-consumer-group-two-member-range-split-then-session-timeout-expiry-reassigns kafka.msk-iam-group-scope-denies-a-consumer-group-join-while-an-assigned-read-succeeds kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse kafka.msk-serverless-refuses-a-scram-client-and-serves-the-iam-client-on-the-same-socket kafka.scram-secret-disassociation-immediate-auth-revocation kafka.topic-partition-key-hash-consistent-routing kafka.topic-partition-key-ordering-preserved kafka.topic-partition-message-order-preserved kafka.topic-partition-message-ordering-preserved kafka.cluster-delete-stale-version-rejected kafka.configuration-delete-in-use-conflict
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.deploy-cfn-msk-cluster-active-and-reachable|run#16|PublishedUrlRejected|socket.gethostbyname(ctx.endpoint_host(host, 9092)[0])
Issue-Key: kafka.deploy-msk-cluster-cfn-active-and-bootstrap-brokers|run#18|PublishedUrlRejected|socket.getaddrinfo(*ctx.endpoint_host(host, int(port))) # raises socket.gaierror if the hostname is not resolvable
Issue-Key: kafka.e2e-cluster-scram-auth-produce-consume-teardown|_grant_scram_acls#13|PublishedUrlRejected|brokers = ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown|_grant_scram_acls#15|PublishedUrlRejected|brokers = ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.kafka-consumer-group-offsets-survive-a-rebalance-and-no-record-is-redelivered|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.kafka-explicit-partition-key-routing-preserves-per-key-order|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.kafka-iam-auth-produce-consume-round-trip-over-the-native-protocol|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.kafka-produce-fetch-in-vpc|run#17|PublishedUrlRejected|broker_list = [ctx.endpoint_host(h, p) for h, p in broker_list]
Issue-Key: kafka.kafka-record-above-max-message-bytes-rejected-and-a-just-under-record-round-trips|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.msk-cluster-configuration-num-partitions-default-is-applied-by-the-broker|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.msk-consumer-group-two-member-range-split-then-session-timeout-expiry-reassigns|<genexpr>#2|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.msk-iam-group-scope-denies-a-consumer-group-join-while-an-assigned-read-succeeds|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.msk-serverless-refuses-a-scram-client-and-serves-the-iam-client-on-the-same-socket|<genexpr>#1|PublishedUrlRejected|return ",".join("%s:%d" % ctx.endpoint_host(h, port or int(p)) for h, p in pairs)
Issue-Key: kafka.scram-secret-disassociation-immediate-auth-revocation|_route#9|PublishedUrlRejected|return ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.topic-partition-key-hash-consistent-routing|_grant_scram_acls#3|PublishedUrlRejected|brokers = ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.topic-partition-key-ordering-preserved|_grant_scram_acls#14|PublishedUrlRejected|brokers = ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.topic-partition-message-order-preserved|_grant_scram_acls#3|PublishedUrlRejected|brokers = ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.topic-partition-message-ordering-preserved|_grant_scram_acls#6|PublishedUrlRejected|brokers = ["%s:%d" % ctx.endpoint_host(h, int(p)) for h, p in pairs]
Issue-Key: kafka.cluster-delete-stale-version-rejected|run#7|AssertionError|assert message == "One or more of the parameters are not valid.", f"stale-version delete message: {message!r}"
Issue-Key: kafka.configuration-delete-in-use-conflict|run#19|AssertionError|assert message == (
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# MSK bootstrap endpoints are published as real AWS broker DNS names the emulator does not serve, and two delete refusals carry emulator prose (3 causes)

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBootstrapBrokers

## Description
THREE distinct root causes in the kafka service. Each must be closed separately before this issue is submitted.

Cause 1 of 3: the MSK bootstrap endpoints the emulator publishes are fully-qualified public AWS broker DNS names, not an address of the emulator itself, so no client can ever reach the MSK data plane. (22 failing checks: kafka.deploy-cfn-msk-cluster-active-and-reachable, kafka.deploy-msk-cluster-cfn-active-and-bootstrap-brokers, kafka.e2e-cluster-scram-auth-produce-consume-teardown, kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown, kafka.kafka-consumer-group-offsets-survive-a-rebalance-and-no-record-is-redelivered, kafka.kafka-explicit-partition-key-routing-preserves-per-key-order, kafka.kafka-four-concurrent-producers-all-land-every-record-with-distinct-offsets, kafka.kafka-iam-auth-produce-consume-round-trip-over-the-native-protocol, kafka.kafka-iam-policy-without-write-permission-fails-produce-and-a-read-still-succeeds, kafka.kafka-produce-fetch-in-vpc, kafka.kafka-record-above-max-message-bytes-rejected-and-a-just-under-record-round-trips, kafka.msk-cluster-configuration-num-partitions-default-is-applied-by-the-broker, kafka.msk-consumer-group-two-member-range-split-then-session-timeout-expiry-reassigns, kafka.msk-iam-group-scope-denies-a-consumer-group-join-while-an-assigned-read-succeeds, kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses, kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse, kafka.msk-serverless-refuses-a-scram-client-and-serves-the-iam-client-on-the-same-socket, kafka.scram-secret-disassociation-immediate-auth-revocation, kafka.topic-partition-key-hash-consistent-routing, kafka.topic-partition-key-ordering-preserved, kafka.topic-partition-message-order-preserved, kafka.topic-partition-message-ordering-preserved).
AWS: Smithy GetBootstrapBrokers (GET /v1/clusters/{ClusterArn}/bootstrap-brokers, 200) returns broker-string members (BootstrapBrokerString, ...Tls, ...SaslScram, ...SaslIam and their Public*/VpcConnectivity*/Ipv6 siblings), each a comma-separated list of host:port pairs. The contract of those strings is that they are endpoints the service ITSELF accepts a Kafka wire-protocol connection on: a client takes the string verbatim, connects, completes the handshake, and then produces/fetches, commits consumer-group offsets, is authenticated per scheme, and (for open monitoring) scrapes the exporter port on the same broker hosts. The DNS form of the names is an AWS implementation detail, not a requirement; what IS required is that the announced host:port is the one the caller can reach the broker on. (Recorded behavior for the data-plane semantics; the model contributes only that the members are endpoint strings.)
Emulator: the broker-string members are synthesised as real, public AWS-domain names that belong to AWS and that this emulator neither owns nor serves: for provisioned clusters `b-<n>.<cluster-name>-<id>.<suffix>.c<n>.kafka.<region>.amazonaws.com:<port>` (ports 9092/9098/9196/9198 observed) and for serverless clusters `boot-<id>.c<n>.kafka-serverless.<region>.amazonaws.com:9098`. The host/id/suffix/port values in any one recording are per-run and are not the requirement — the defect is that the announced host is not an address of the emulator at all. Every check that tries to use the published endpoint is refused before a byte is sent, with `simparity.published.PublishedUrlRejected: endpoint_host was given <published host>:<port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]`. This is the emulator's own egress gap: it announced an address other than the one it is reached on.
Why: a client configured from GetBootstrapBrokers (or from the serverless bootstrap string) points its producer and consumer at a hostname that resolves to AWS, or to nothing, instead of to the emulator — so produce/fetch, per-key partition routing, per-partition FIFO order, consumer-group offset commit and rebalance, max.message.bytes enforcement, per-scheme SASL/IAM and SASL/SCRAM authentication, SCRAM credential revocation and the open-monitoring exporter are all unreachable and unverifiable. An application cannot be developed or tested against this emulator's MSK beyond the control plane.
Fix: the kafka service's endpoint publication — the GetBootstrapBrokers handler's broker-string assembly and the serverless bootstrap-string assembly must emit a host:port the emulator itself answers on, and the broker listener (with its per-scheme authentication, record/partition/offset state and open-monitoring exporter ports) must accept connections there.
Traces: the 22 check ids listed above; every one of them fails with the PublishedUrlRejected refusal quoted above.
Related open issues describe the same family from the older symptom (a connect that timed out rather than a host that is refused up front) and do NOT carry these checks' replay keys: 20261104T171616Z-kafka-no-kafka-broker-answers-on-the-advertised-bootstrap.md and 20261107T210159Z-kafka-three-more-msk-data-plane-checks-reach-no.md. 20261104T171616Z-kafka-getbootstrapbrokers-serves-every-broker-string-variant-regardless-of.md is a different defect (WHICH members are populated, not what host they name).

Cause 2 of 3: DeleteCluster's stale-CurrentVersion refusal carries emulator prose instead of MSK's message. (Failing check: kafka.cluster-delete-stale-version-rejected.)
AWS: Smithy DeleteCluster (DELETE /v1/clusters/{ClusterArn}, CurrentVersion as httpQuery `currentVersion`; errors BadRequestException 400, NotFoundException 404, ForbiddenException 403). A DeleteCluster whose CurrentVersion does not match the cluster's current version is refused with HTTP 400 BadRequestException and the message `One or more of the parameters are not valid.` — fixed text with no variable part — and the cluster stays ACTIVE at its unchanged CurrentVersion.
Emulator: HTTP 400 BadRequestException (class and status correct) with the message `The version of the cluster isn’t current. Check the current version and try again.` — emulator-authored prose, entirely fixed text, which is not AWS's message for this refusal.
Why: a client that branches on the refusal message (or an operator reading it) sees wording that never occurs in AWS; message-matching retry/alerting logic written against AWS does not fire, and a recorded-vs-live comparison of the same call diverges on the one field the caller is given to explain the failure.
Fix: the DeleteCluster handler's CurrentVersion validation — the message it attaches to its BadRequestException.
Traces: kafka.cluster-delete-stale-version-rejected.
The open issue 20261104T171616Z-kafka-msk-state-version-conflict-duplicate-topic-and-invalid.md covers other version-conflict wordings and does not carry this check's replay key.

Cause 3 of 3: DeleteConfiguration's in-use refusal carries emulator prose instead of MSK's message. (Failing check: kafka.configuration-delete-in-use-conflict.)
AWS: Smithy DeleteConfiguration (DELETE /v1/configurations/{Arn}; errors BadRequestException 400, NotFoundException 404). Deleting a configuration that is still attached to at least one cluster is refused with HTTP 400 BadRequestException and the message `Configuration is in use by one or more clusters. Dissociate the configuration from the clusters.` — fixed text, no variable part — and the configuration stays ACTIVE. Once no cluster references it, the same call succeeds (200, State DELETING) and DescribeConfiguration then refuses the ARN with BadRequestException `Configuration ARN does not exist.`
Emulator: HTTP 400 BadRequestException (class and status correct) with `The configuration is in use by one or more clusters and can’t be deleted.` — emulator prose, and it omits the remediation sentence AWS words as `Dissociate the configuration from the clusters.`
Why: same as cause 2 — the only field that explains the refusal differs from AWS, so message-driven client logic and operator runbooks written against AWS do not match, and the caller is not told the remediation AWS states.
Fix: the DeleteConfiguration handler's in-use guard — the message it attaches to its BadRequestException.
Traces: kafka.configuration-delete-in-use-conflict.

## Evidence
Cause 1 (22 checks, verdict mismatch; none marked ALREADY FILED or OWNED). Representative refusals, exactly as replayed:
  kafka.deploy-cfn-msk-cluster-active-and-reachable — `simparity.published.PublishedUrlRejected: endpoint_host was given b-1.sim2real-msk-4e8d3331c98f.05f96b.c1.kafka.us-west-2.amazonaws.com:9092 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address`
  kafka.e2e-cluster-scram-auth-produce-consume-teardown — same refusal for `b-1.sim2real-msk-scram-bf88d3d277e8.7bfc0a.c8.kafka.us-west-2.amazonaws.com:9098`
  kafka.topic-partition-message-ordering-preserved — same refusal for `b-1.sim2real-ord-3fb7f71c1a27.f1dfba.c1.kafka.us-west-2.amazonaws.com:9198`
  kafka.msk-serverless-advertised-bootstrap-ports-accept-and-unadvertised-ports-refuse — same refusal for `boot-dfd460.c4.kafka-serverless.us-west-2.amazonaws.com:9098`
  kafka.msk-open-monitoring-jmx-exporter-serves-prometheus-and-the-disabled-node-exporter-refuses — same refusal for `b-1.sim2real-behav-kmon-msk-79b11c78354c.1a6afb.c1.kafka.us-west-2.amazonaws.com:9092` (the exporter scrape target is the same broker host).
  In every case the host came straight from the emulator's own GetBootstrapBrokers/serverless bootstrap string, which each check routes through ctx.endpoint_host before connecting. The cluster names, 12-hex ids, suffixes and ports are per-run values; the requirement is that the published host be an address the emulator answers on.
  Expected (AWS): the published host:port accepts a TCP connection and completes the Kafka handshake, so the produce/consume/ordering/auth/monitoring assertions can be evaluated. Actual: the published host is not the emulator's, so nothing is ever attempted.

Cause 2 — kafka.cluster-delete-stale-version-rejected: after CreateClusterV2 + wait ACTIVE, `delete_cluster(ClusterArn=<arn>, CurrentVersion="bogus-stale-version")`.
  Expected (AWS): 400 BadRequestException, Message `One or more of the parameters are not valid.`
  Actual (emulator): 400 BadRequestException, Message `The version of the cluster isn’t current. Check the current version and try again.`
  Suite assertion: `AssertionError: stale-version delete message: 'The version of the cluster isn’t current. Check the current version and try again.'`

Cause 3 — kafka.configuration-delete-in-use-conflict: a configuration attached to an ACTIVE cluster via UpdateClusterConfiguration, then `delete_configuration(Arn=<config arn>)`.
  Expected (AWS): 400 BadRequestException, Message `Configuration is in use by one or more clusters. Dissociate the configuration from the clusters.`
  Actual (emulator): 400 BadRequestException, Message `The configuration is in use by one or more clusters and can’t be deleted.`
  Suite assertion: `AssertionError: in-use DeleteConfiguration message: 'The configuration is in use by one or more clusters and can’t be deleted.'`

Smithy cited: DeleteCluster DELETE /v1/clusters/{ClusterArn} (CurrentVersion httpQuery currentVersion) errors BadRequestException/400, NotFoundException/404; DeleteConfiguration DELETE /v1/configurations/{Arn} errors BadRequestException/400, NotFoundException/404; GetBootstrapBrokers GET /v1/clusters/{ClusterArn}/bootstrap-brokers code 200 with the optional broker-string members listed above.

## Steps to Reproduce
Cause 1:
1. Boot the emulator. CreateClusterV2 a PROVISIONED cluster (2 brokers, Sasl.Iam enabled) and wait for DescribeClusterV2 State == ACTIVE; also CreateClusterV2 a SERVERLESS cluster with Sasl.Iam enabled.
2. Call GetBootstrapBrokers on each and read the populated broker string(s).
3. Assert every host:port in those strings is an address of the emulator itself (its endpoint host, a documented own host, or a loopback address) — today they are `b-<n>.<cluster>.<suffix>.c<n>.kafka.<region>.amazonaws.com:<port>` / `boot-<id>.c<n>.kafka-serverless.<region>.amazonaws.com:9098`.
4. Open a TCP connection to each published host:port and assert it is accepted and the Kafka handshake completes; then produce and consume a record over it, and scrape the enabled open-monitoring exporter port on the same broker host.
5. Replay the 22 check ids in trace_ids (all but cluster-delete-stale-version-rejected and configuration-delete-in-use-conflict).

Cause 2:
1. CreateClusterV2 a provisioned cluster; wait for State ACTIVE and note CurrentVersion.
2. Call DeleteCluster with CurrentVersion="bogus-stale-version".
3. Assert 400 BadRequestException with Message exactly `One or more of the parameters are not valid.`, and that DescribeCluster still reports State ACTIVE at the unchanged CurrentVersion.
4. Replay kafka.cluster-delete-stale-version-rejected.

Cause 3:
1. CreateConfiguration, then UpdateClusterConfiguration to attach it to an ACTIVE cluster and wait for the operation to complete.
2. Call DeleteConfiguration on that configuration's ARN.
3. Assert 400 BadRequestException with Message exactly `Configuration is in use by one or more clusters. Dissociate the configuration from the clusters.`, and that DescribeConfiguration still reports State ACTIVE.
4. Replay kafka.configuration-delete-in-use-conflict.
