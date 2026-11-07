Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.scram-secret-disassociation-immediate-auth-revocation kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown kafka.topic-partition-message-ordering-preserved
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.scram-secret-disassociation-immediate-auth-revocation|_scram_auth_outcome#10|AssertionError|raise AssertionError(
Issue-Key: kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown|_grant_scram_acls#15|Timeout|time.sleep(15.0)
Issue-Key: kafka.topic-partition-message-ordering-preserved|_grant_scram_acls#6|Timeout|admin = KafkaAdminClient(
Issue-Commit: 9a655abc910364880d25d282d4207456228c1efc

# Three more MSK data-plane checks reach no verdict: nothing accepts a connection on the advertised SASL/SCRAM bootstrap endpoint

> _Note: this issue was identified against the code base at commit `9a655abc910364880d25d282d4207456228c1efc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBootstrapBrokers

## Description
Cause 1 of 1: the emulator advertises Kafka bootstrap endpoints that no broker serves, so three further checks never reach a verdict. These three check ids are unowned today: none is marked ALREADY FILED, and the suite reports none of them as OWNED by an open issue (20261104T171616Z-kafka-no-kafka-broker-answers-on-the-advertised-bootstrap.md names the same root cause and lists two of these ids in its trace ids, but its recorded replay keys for them point at different frames, so the replay no longer binds them to it; 20261105T161551Z-kafka-two-more-msk-checks-never-reach-a-verdict.md likewise covers kafka.topic-partition-message-ordering-preserved under a key that no longer binds).

AWS: an ACTIVE MSK cluster serves the Kafka wire protocol on exactly the endpoints GetBootstrapBrokers advertises — BootstrapBrokerStringSaslScram (port 9096 provisioned, TLS) and BootstrapBrokerStringSaslIam (9098) among them. A TCP connect to an advertised host:port is accepted, the SASL/SCRAM handshake either completes (associated secret, correct password) or returns an explicit authentication FAILURE verdict (disassociated or wrong secret) rather than hanging; a KafkaAdminClient can then create topics and ACLs, and records produced in order to a single-partition topic are consumed back in that order. A disassociated SCRAM secret leaves ListScramSecrets at once and its credentials stop authenticating at the brokers within the documented propagation window, while a still-associated secret keeps authenticating. (Recorded behavior for the data-plane semantics; the Smithy model only states that the bootstrap strings exist — GetBootstrapBrokers returns BootstrapBrokerStringSaslScram/SaslIam etc.)
Emulator: nothing answers on the advertised endpoint. The SCRAM-revocation check localizes it precisely: its SASL/SCRAM probe for the still-associated positive-control user ends in KafkaTimeoutError("Unable to bootstrap from ['<advertised host:port>', '<advertised host:port>']") — no authentication verdict at all, success or failure. The other two checks report only the bare 'Check timed out after 180.0s' with no HTTP status, no error code and no frame: they block on the first admin/produce/consume call against the advertised address. Because the emulator itself announced those addresses, this is the emulator's egress gap, not the runner's; kafka-python is present (each check raises an explicit driver-absent assertion when it is not, and none did).
Why: the credential-revocation contract a client depends on (a disassociated SCRAM secret can no longer authenticate, an associated one still can) is unverifiable, as are per-partition ordering and the full create-cluster/create-topic/produce/consume/teardown lifecycle. A client configured from GetBootstrapBrokers hangs until its own timeout instead of connecting or being refused.
Fix: the kafka service must serve the Kafka wire protocol on the ports it advertises — the broker listener with per-scheme SASL authentication (SCRAM credentials wired to the cluster's associated secrets, so a disassociated secret is rejected and an associated one accepted) plus the topic/record/partition/offset state the produce and fetch paths read and write — or advertise only endpoints it actually serves. The cluster/topic transitions these checks wait on before connecting must also settle (see 20261104T171616Z-kafka-msk-asynchronous-transitions-are-clock-owed-so-an.md).
Traces: kafka.scram-secret-disassociation-immediate-auth-revocation (carries the KafkaTimeoutError frame), kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown and kafka.topic-partition-message-ordering-preserved (bare 180 s timeouts, no frame).

## Evidence
All three are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

kafka.scram-secret-disassociation-immediate-auth-revocation — AssertionError raised from the check's own _scram_auth_outcome/_assert_auth_succeeds: "SASL/SCRAM probe for '<user>' failed without an authentication verdict: KafkaTimeoutError(\"Unable to bootstrap from ['127.0.0.1:9196', '127.0.0.1:9196']\")". The host:port shown is where this run reached the emulator (a per-run value); the requirement is that whatever endpoint GetBootstrapBrokers advertises accepts a connection and yields an authentication verdict.
  Expected (AWS): the still-associated secret's probe returns an AUTHENTICATED verdict (positive control), and the disassociated secret's probe returns an authentication FAILURE within the propagation window while ListScramSecrets no longer lists it.
  Actual: no verdict either way — the connection is never established.

kafka.e2e-cluster-topic-lifecycle-produce-consume-teardown — 'Error: Check timed out after 180.0s'; no status, no error code, no frame. The check creates a SCRAM+IAM provisioned cluster with an ACL-enforced configuration, waits ACTIVE, grants ACLs with a KafkaAdminClient over the advertised bootstrap string, produces and consumes one message over the Kafka wire protocol, then asserts the deleted topic and cluster both resolve to NotFoundException.
  Expected (AWS): the produced message is consumed back, then both resources resolve to NotFoundException. Actual: nothing was produced or consumed; no assertion was evaluated.

kafka.topic-partition-message-ordering-preserved — 'Error: Check timed out after 180.0s'; no status, no error code, no frame. The check creates a 1-partition topic on a SCRAM/IAM cluster and produces an ordered batch, then consumes it back.
  Expected (AWS): the consumed sequence equals the produced sequence exactly within the partition (the payload count and text are the check's own; the requirement is order equality). Actual: no verdict.

kafka-python is installed in this environment — each of these checks raises an explicit 'kafka-python is not installed' assertion when it is absent, and none of the three did, so these are not driver-absent skips.

## Steps to Reproduce
1. Boot the emulator. Create a PROVISIONED MSK cluster with ClientAuthentication Sasl.Scram (+ Sasl.Iam) and TLS in transit; wait for ACTIVE.
2. Create two SCRAM secrets and BatchAssociateScramSecret both; call GetBootstrapBrokers and take BootstrapBrokerStringSaslScram (and SaslIam).
3. Open a plain TCP connection to each advertised host:port and assert it is ACCEPTED (today it never is).
4. Run a SASL/SCRAM handshake with the first (associated) user and assert an explicit AUTHENTICATED verdict; BatchDisassociateScramSecret the second secret, assert it leaves ListScramSecrets immediately, and assert its handshake returns an explicit authentication FAILURE within the propagation window — never a bootstrap timeout.
5. Over a KafkaAdminClient on the advertised endpoint create a 1-partition topic and ACLs, produce N ordered records and consume them back; assert the consumed order equals the produced order, then delete the topic and the cluster and assert both resolve to NotFoundException.
6. Replay the three check ids in trace_ids and confirm each reaches a pass/fail verdict instead of a timeout.
