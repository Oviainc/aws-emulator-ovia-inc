Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.bootstrap-brokers-endpoint-format-by-auth-type kafka.bootstrap-brokers-endpoint-formats-match-cluster-security kafka.bootstrap-brokers-endpoint-types-match-security-config kafka.deploy-msk-cluster-cfn-scram-auth-enforced
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.bootstrap-brokers-endpoint-format-by-auth-type|run#22|AssertionError|assert not brokers.get("BootstrapBrokerStringTls"), "unauthenticated TLS endpoint must not be offered"
Issue-Key: kafka.bootstrap-brokers-endpoint-formats-match-cluster-security|run#11|AssertionError|assert not brokers.get(absent_field), (
Issue-Key: kafka.bootstrap-brokers-endpoint-types-match-security-config|run#15|AssertionError|assert present == {"BootstrapBrokerStringSaslScram"}, (
Issue-Key: kafka.deploy-msk-cluster-cfn-scram-auth-enforced|run#17|AssertionError|assert not brokers.get("BootstrapBrokerStringTls"), "unauthenticated TLS broker string must not be offered"
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# GetBootstrapBrokers serves every broker-string variant regardless of the cluster's client authentication and connectivity configuration

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBootstrapBrokers

## Description
Cause 1 of 1: GetBootstrapBrokers populates broker-string members that the cluster's configuration does not entitle, so a caller cannot tell from the response which connection schemes the cluster actually accepts.

AWS: Smithy GetBootstrapBrokers (GET /v1/clusters/{ClusterArn}/bootstrap-brokers, 200) returns an output whose members are each a DIFFERENT connection scheme: BootstrapBrokerString (plaintext), BootstrapBrokerStringTls (unauthenticated TLS), BootstrapBrokerStringSaslScram, BootstrapBrokerStringSaslIam, their Public* and VpcConnectivity* siblings and the *Ipv6 siblings. Every member is OPTIONAL, and MSK populates ONLY the ones the cluster is configured for: a member is present exactly when the cluster's EncryptionInfo.EncryptionInTransit.ClientBroker setting and ClientAuthentication (Sasl.Scram / Sasl.Iam / Tls / Unauthenticated) and its public-access / multi-VPC connectivity settings enable that scheme, and is ABSENT (not empty-string) otherwise. For a SASL/IAM-only, non-public, non-multi-VPC cluster only BootstrapBrokerStringSaslIam is populated; for a SASL/SCRAM-only cluster only BootstrapBrokerStringSaslScram; neither plaintext nor unauthenticated TLS may be offered when client authentication is required. (Recorded behavior for which member maps to which setting; the model contributes that all members are optional.)
Emulator: the handler fills multiple variants unconditionally from the cluster's broker DNS names. On a SASL/IAM-only cluster it returns BootstrapBrokerString (plaintext, port 9092) — 'b-1.<cluster>.<suffix>.kafka.<region>.amazonaws.com:9092,b-2...:9092' — and on a SASL/SCRAM-only cluster it returns BootstrapBrokerStringTls alongside BootstrapBrokerStringSaslScram. The host names, suffixes and ports quoted here are this run's generated values; the defect is that the member is populated at all for a cluster whose configuration does not offer that scheme.
Why: the response is how a client chooses its security protocol. A client that picks the first populated member, or that asserts an unauthenticated endpoint is NOT offered on an authenticated cluster, configures PLAINTEXT/SSL against a cluster that in AWS only speaks SASL_SSL and fails at connect time with no API-level signal. Security tests that prove 'no unauthenticated endpoint is exposed' pass vacuously against the emulator, and CloudFormation-deployed SCRAM clusters look as if they accept anonymous TLS.
Fix: the GetBootstrapBrokers handler's response assembly must derive each member from the cluster record's encryption-in-transit, client-authentication and connectivity settings and leave the non-entitled members unset, rather than emitting a fixed set of endpoint strings.
Traces: kafka.bootstrap-brokers-endpoint-format-by-auth-type (assertion 'unauthenticated TLS endpoint must not be offered'), kafka.bootstrap-brokers-endpoint-formats-match-cluster-security ('BootstrapBrokerString should be absent for a SASL/IAM-only, non-public, non-multi-VPC cluster but was ...'), kafka.bootstrap-brokers-endpoint-types-match-security-config ('expected exactly {BootstrapBrokerStringSaslScram} populated, got {BootstrapBrokerStringTls, BootstrapBrokerStringSaslScram}'), kafka.deploy-msk-cluster-cfn-scram-auth-enforced ('unauthenticated TLS broker string must not be offered').

## Evidence
All four checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

kafka.bootstrap-brokers-endpoint-types-match-security-config — cluster created with SASL/SCRAM client authentication only.
  Expected (AWS): exactly {'BootstrapBrokerStringSaslScram'} populated.
  Actual (emulator): {'BootstrapBrokerStringTls', 'BootstrapBrokerStringSaslScram'}.

kafka.bootstrap-brokers-endpoint-formats-match-cluster-security — SASL/IAM-only, non-public, non-multi-VPC cluster.
  Expected (AWS): BootstrapBrokerString absent.
  Actual (emulator): BootstrapBrokerString = 'b-1.sim2real-msk-36d39d017ff5.b5f169.c5.kafka.us-west-2.amazonaws.com:9092,b-2.sim2real-msk-36d39d017ff5.b5f169.c5.kafka.us-west-2.amazonaws.com:9092' (the names/suffix are this run's values; the requirement is absence of the member).

kafka.bootstrap-brokers-endpoint-format-by-auth-type — AssertionError: 'unauthenticated TLS endpoint must not be offered' (BootstrapBrokerStringTls populated on an authenticated cluster).

kafka.deploy-msk-cluster-cfn-scram-auth-enforced — a CloudFormation-deployed SCRAM cluster: AssertionError 'unauthenticated TLS broker string must not be offered'.

Smithy cited: GetBootstrapBrokers http GET /v1/clusters/{ClusterArn}/bootstrap-brokers code 200; output members BootstrapBrokerString, BootstrapBrokerStringTls, BootstrapBrokerStringSaslScram, BootstrapBrokerStringSaslIam, BootstrapBrokerStringPublicTls, BootstrapBrokerStringPublicSaslScram, BootstrapBrokerStringPublicSaslIam, BootstrapBrokerStringVpcConnectivityTls/SaslScram/SaslIam and the Ipv6 siblings — all optional.

## Steps to Reproduce
1. Boot the emulator and point a kafka client at it.
2. CreateClusterV2 a PROVISIONED cluster with Provisioned.ClientAuthentication.Sasl.Scram.Enabled=true (and nothing else), EncryptionInfo requiring TLS; wait for DescribeClusterV2 State == ACTIVE.
3. GetBootstrapBrokers on it. Assert BootstrapBrokerStringSaslScram is populated and that BootstrapBrokerString, BootstrapBrokerStringTls and every Public*/VpcConnectivity* member are ABSENT from the body.
4. Repeat with a cluster whose only client authentication is Sasl.Iam: assert only BootstrapBrokerStringSaslIam is populated.
5. Deploy an AWS::MSK::Cluster with SCRAM client authentication via CloudFormation and repeat step 3 on the stack's cluster.
6. Replay kafka.bootstrap-brokers-endpoint-format-by-auth-type, kafka.bootstrap-brokers-endpoint-formats-match-cluster-security, kafka.bootstrap-brokers-endpoint-types-match-security-config and kafka.deploy-msk-cluster-cfn-scram-auth-enforced.
