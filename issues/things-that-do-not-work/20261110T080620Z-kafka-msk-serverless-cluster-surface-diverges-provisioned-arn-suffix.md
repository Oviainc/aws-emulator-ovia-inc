Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-serverless-iam-bootstrap-broker-discovery
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-serverless-iam-bootstrap-broker-discovery
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|CreateClusterV2|2xx->2xx|$.clusterArn
Issue-Key: kafka|DescribeClusterV2|2xx->2xx|$.clusterInfo.serverless.vpcConfigs[*].securityGroupIds
Issue-Key: kafka|GetBootstrapBrokers|2xx->2xx|$.bootstrapBrokerStringSaslIam
Issue-Key: kafka|GetBootstrapBrokers|4xx->4xx|$.invalidParameter
Issue-Commit: 9080d0da009b629d5b5a26b7843a3b4b93752dc8

# MSK serverless cluster surface diverges: provisioned ARN suffix, dropped securityGroupIds, non-own bootstrap host and an omitted invalidParameter (4 causes)

> _Note: this issue was identified against the code base at commit `9080d0da009b629d5b5a26b7843a3b4b93752dc8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterV2

## Description
FOUR distinct root causes in the kafka service, all on the SERVERLESS cluster create/describe/bootstrap-discovery path. Each must be closed separately before this issue is submitted.

Cause 1 of 4: CreateClusterV2 mints a SERVERLESS cluster ARN with the provisioned-cluster trailing segment. (fixture kafka-serverless-iam-bootstrap-broker-discovery, step task#6.)
AWS: Smithy CreateClusterV2 (POST /api/v2/clusters, code 200) returns ClusterArn (jsonName clusterArn, target __string) for a request whose only cluster body is `serverless` (ServerlessRequest, VpcConfigs required). The model places no constraint on the ARN text, so: recorded behavior only — MSK's cluster ARN has the form `arn:aws:kafka:<region>:<account>:cluster/<clusterName>/<uuid>-<type-suffix>`, and the trailing suffix encodes the cluster type: a SERVERLESS cluster carries `-s2`, a provisioned cluster carries a small decimal (`-1`). The region, account, cluster name and uuid are request/per-run values that fill the template; the suffix is the part the recording pins, and it is what tells a client (and MSK's own routing) which cluster type the ARN names.
Emulator: for the same serverless CreateClusterV2 it returns an ARN ending `-1`, i.e. the provisioned suffix, with the serverless `-s2` suffix never produced (recorded: expected `arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-probe-msksl-a27ac9f7/0fd90f6d-e947-4255-8b8c-397491b32338-s2`, got `arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-probe-msksl-a27ac9f7/ab0721d7-d61f-42f4-bbef-114a13bea5c8-1`; the uuid differs per run and is not the requirement, the `-s2` vs `-1` suffix is).
Why: a client that reads the cluster type out of the ARN suffix — as the MSK console, CloudFormation physical ids, IAM resource patterns such as `cluster/*/*-s2` and tooling that distinguishes serverless from provisioned ARNs all do — classifies every emulated serverless cluster as provisioned, and resource-policy/permission patterns scoped to serverless ARNs never match.
Fix: the CreateClusterV2 handler's ARN minting for the serverless branch (the cluster-ARN construction in the kafka service's cluster state/ARN helper), which must select the suffix from the cluster type rather than always emitting the provisioned one.
Traces: kafka-serverless-iam-bootstrap-broker-discovery#task#6.

Cause 2 of 4: DescribeClusterV2 drops securityGroupIds from a serverless cluster's vpcConfigs. (fixture kafka-serverless-iam-bootstrap-broker-discovery, step task#7.)
AWS: Smithy DescribeClusterV2 (GET /api/v2/clusters/{ClusterArn}, code 200) returns ClusterInfo (jsonName clusterInfo) of shape Cluster; its Serverless member (shape Serverless) has VpcConfigs (jsonName vpcConfigs, required, list of VpcConfig), and VpcConfig has SubnetIds (jsonName subnetIds, required) and SecurityGroupIds (jsonName securityGroupIds, list of string, documented "The IDs of the security groups associated with the cluster"). MSK echoes the security groups that were supplied in CreateClusterV2's serverless.vpcConfigs, so the member is present and non-empty on a describe of a cluster created with them; the actual group ids are account/VPC values from the request, not fixed text.
Emulator: `$.clusterInfo.serverless.vpcConfigs[0].securityGroupIds` is absent entirely (expected a list, shape asserted only; got None) while the sibling subnetIds is served, so the serverless VPC configuration the caller submitted is only half round-tripped.
Why: a client that reads the cluster's network configuration back from DescribeClusterV2 — to reuse the groups when attaching an ESM, a VPC connection or another client in the same VPC, or to assert its deployment converged — sees no security groups at all and cannot distinguish "none configured" from "not reported", so configuration-drift checks and dependent provisioning both break.
Fix: the serverless branch of the DescribeClusterV2 serializer / the cluster state that records a serverless cluster's VpcConfigs — it must persist and emit SecurityGroupIds alongside SubnetIds.
Traces: kafka-serverless-iam-bootstrap-broker-discovery#task#7.

Cause 3 of 4: GetBootstrapBrokers publishes a serverless SASL/IAM bootstrap endpoint on a real AWS kafka-serverless DNS name the emulator does not serve. (fixture kafka-serverless-iam-bootstrap-broker-discovery, step task#8.)
AWS: Smithy GetBootstrapBrokers (GET /v1/clusters/{ClusterArn}/bootstrap-brokers, code 200) returns broker-string members, here BootstrapBrokerStringSaslIam (jsonName bootstrapBrokerStringSaslIam, __string), a comma-separated list of host:port pairs. The model contributes only that these are endpoint strings; the contract (recorded behavior) is that the announced host:port is one the serving endpoint ITSELF accepts a Kafka wire-protocol connection on — a client takes the string verbatim, connects, completes the SASL/IAM handshake and then produces/fetches. The DNS form of AWS's own names is an implementation detail, not a requirement; what is required is that the published host be an address the caller can reach the broker on.
Emulator: it answers with a public AWS-domain name it neither owns nor serves — of the shape `boot-<id>.c<n>.kafka-serverless.<region>.amazonaws.com:9098` (recorded `boot-0a0d1c.c4.kafka-serverless.us-west-2.amazonaws.com:9098`; the id, c-number, region and port are per-run/environment values and not the requirement). The replay verdict for this field is `host_not_own`: the emulator announced an address other than the one it is reached on, so the endpoint is unreachable before a byte is sent.
Why: a client configured from GetBootstrapBrokers points its producer/consumer at a hostname that resolves to AWS or to nothing instead of to the emulator, so the entire serverless MSK data plane (produce, fetch, offset commit, SASL/IAM authentication) is unreachable and no application can be developed against it beyond the control plane.
Fix: the GetBootstrapBrokers handler's serverless broker-string assembly — it must emit a host:port the emulator itself answers on, and the broker listener must accept the SASL/IAM connection there.
Traces: kafka-serverless-iam-bootstrap-broker-discovery#task#8. Related OPEN issues describe this family from the behavioral suite and do NOT carry this fixture step's replay key: 20261109T211434Z-kafka-msk-bootstrap-endpoints-are-published-as-real-aws.md, 20261109T210043Z-kafka-lambda-msk-bootstrap-brokers-are-published-on-a-kafka.md and 20261104T171616Z-kafka-no-kafka-broker-answers-on-the-advertised-bootstrap.md; 20261104T171616Z-kafka-getbootstrapbrokers-serves-every-broker-string-variant-regardless-of.md is a different defect (which members are populated, not what host they name).

Cause 4 of 4: GetBootstrapBrokers' not-found refusal omits the invalidParameter member. (fixture kafka-serverless-iam-bootstrap-broker-discovery, step task#9.)
AWS: Smithy NotFoundException (httpError 404) carries two members, InvalidParameter (jsonName invalidParameter, "The parameter that caused the error") and Message (jsonName message). On a GetBootstrapBrokers for a cluster ARN that does not exist, MSK answers 404 NotFoundException with invalidParameter naming the offending input by its WIRE name, `clusterArn` — fixed text drawn from the request member's jsonName, not a per-run value — alongside the message. (Note GetBootstrapBrokers' modeled error list does not include NotFoundException; the recorded response is the authority for the 404, and the status itself matched.)
Emulator: it answers 404 with the message but with no invalidParameter member at all (expected 'clusterArn', got None), so the field AWS uses to attribute the failure is simply missing from the payload.
Why: a client or SDK that reads e.NotFoundException.invalidParameter to attribute the refusal to a specific input — for error reporting, for deciding which argument to correct, or for a recorded-vs-live payload comparison — gets nothing, and code that dereferences it sees a null where AWS always supplies the parameter name.
Fix: the GetBootstrapBrokers handler's cluster lookup / the kafka error serializer — the NotFoundException it raises must set InvalidParameter to the wire name of the input member that failed resolution.
Traces: kafka-serverless-iam-bootstrap-broker-discovery#task#9. The open issue 20261029T202809Z-kafka-msk-error-payloads-name-the-smithy-member-instead.md covers the adjacent defect where invalidParameter is PRESENT but carries the Smithy member name (`ClusterArn`) instead of the wire name, and it does not carry this fixture step's replay key; here the member is absent altogether.

## Evidence
All four causes replayed from fixture `kafka-serverless-iam-bootstrap-broker-discovery` (service kafka, verdict mismatch, no stub_path and no owning issue).

Cause 1 — step task#6, CreateClusterV2, status 200 expected / 200 actual:
  $.clusterArn: value_mismatch
  Expected (AWS): arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-probe-msksl-a27ac9f7/0fd90f6d-e947-4255-8b8c-397491b32338-s2
  Actual (emulator): arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-probe-msksl-a27ac9f7/ab0721d7-d61f-42f4-bbef-114a13bea5c8-1
  The uuid segment is a per-run value; the divergence that is a requirement is the trailing type suffix `-s2` (serverless) vs `-1` (provisioned).

Cause 2 — step task#7, DescribeClusterV2, status 200 expected / 200 actual:
  $.clusterInfo.serverless.vpcConfigs[0].securityGroupIds: missing (expected a list, shape asserted only; got None)
  Sibling subnetIds was served; only securityGroupIds is absent.

Cause 3 — step task#8, GetBootstrapBrokers, status 200 expected / 200 actual:
  $.bootstrapBrokerStringSaslIam: host_not_own (expected an own host:port, shape asserted only; got 'boot-0a0d1c.c4.kafka-serverless.us-west-2.amazonaws.com:9098')
  The verdict is the replay's own egress check: the published host is not one of the emulator's hosts. The id/c-number/region/port are per-run; the defect is that the announced host is not an address the emulator is reached on.

Cause 4 — step task#9, GetBootstrapBrokers, status 404 expected / 404 actual:
  $.invalidParameter: missing (expected 'clusterArn', got None)
  Status and error class agreed; the NotFoundException payload omits the invalidParameter member.

Smithy cited: CreateClusterV2 POST /api/v2/clusters code 200, output ClusterArn (jsonName clusterArn); ServerlessRequest.VpcConfigs required; DescribeClusterV2 GET /api/v2/clusters/{ClusterArn} code 200, output ClusterInfo (jsonName clusterInfo) -> Cluster.Serverless -> Serverless.VpcConfigs (required, list of VpcConfig) -> VpcConfig.SubnetIds (required) + VpcConfig.SecurityGroupIds (jsonName securityGroupIds); GetBootstrapBrokers GET /v1/clusters/{ClusterArn}/bootstrap-brokers code 200 with BootstrapBrokerStringSaslIam (jsonName bootstrapBrokerStringSaslIam); NotFoundException httpError 404 with members InvalidParameter (jsonName invalidParameter) and Message (jsonName message).

## Steps to Reproduce
Boot the emulator from this checkout, then replay fixture `kafka-serverless-iam-bootstrap-broker-discovery`.

Cause 1 (step task#6):
1. Call CreateClusterV2 with only the `serverless` body set (vpcConfigs with subnetIds and securityGroupIds, clientAuthentication.sasl.iam.enabled = true).
2. Read clusterArn from the 200 response.
3. Assert its trailing segment is `<uuid>-s2` (the serverless cluster-type suffix), not `<uuid>-1`.

Cause 2 (step task#7):
1. Using the ARN from step 1, call DescribeClusterV2.
2. Assert clusterInfo.serverless.vpcConfigs[0].securityGroupIds is present and lists the security groups supplied at create time (today the member is absent while subnetIds is served).

Cause 3 (step task#8):
1. Call GetBootstrapBrokers on the serverless cluster ARN.
2. Assert every host:port in bootstrapBrokerStringSaslIam is an address of the emulator itself (its endpoint host, a documented own host, or a loopback address) — today it is `boot-<id>.c<n>.kafka-serverless.<region>.amazonaws.com:9098`.
3. Open a TCP connection to the published host:port and assert it is accepted and the SASL/IAM Kafka handshake completes.

Cause 4 (step task#9):
1. Call GetBootstrapBrokers with a cluster ARN that does not exist (or the deleted cluster's ARN).
2. Assert 404 NotFoundException AND that the payload's invalidParameter is exactly `clusterArn` — today the member is omitted.
