Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.vpcconnection-requires-multi-vpc-enabled-cluster kafka.cluster-policy-enforced-on-cross-account-vpc-connection kafka.vpc-connection-malformed-target-cluster-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.vpcconnection-requires-multi-vpc-enabled-cluster|_err#3|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: kafka.cluster-policy-enforced-on-cross-account-vpc-connection|_error_of#9|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Key: kafka.vpc-connection-malformed-target-cluster-rejected|run#9|AssertionError|assert code == expected, f"expected {expected!r}, got {code!r}"
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# CreateVpcConnection skips MSK's cluster-state and SASL/IAM gates and answers a parameter error where MSK denies authorization (3 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcConnection

## Description
THREE distinct root causes on the CreateVpcConnection admission path. Each is a different gate and must be closed separately; all three must be closed before this issue is submitted. The open issue 20261029T175822Z-kafka-client-vpc-connection-surface-on-a-nonexistent-cluster.md covers ListClientVpcConnections/RejectClientVpcConnection on a nonexistent cluster and carries none of the check ids below.

Cause 1 of 3: CreateVpcConnection against a cluster that is still CREATING is accepted.
AWS: CreateVpcConnection (POST /v1/vpc-connection, 200) models BadRequestException/400 (Smithy: errors BadRequestException/400, ForbiddenException/403, UnauthorizedException/401, TooManyRequestsException/429, InternalServerErrorException/500, ServiceUnavailableException/503). MSK checks the target cluster's STATE first and refuses with BadRequestException and the fixed message 'Invalid cluster state. To create, delete or reject a VPC connection, your cluster must be in one of these states: ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER.' The state gate runs BEFORE the VpcId is resolved, so a real VpcId and a nonexistent one get the identical refusal, and nothing is registered: ListClientVpcConnections for the cluster stays empty.
Emulator: HTTP 200 with a VpcConnectionArn and State='CREATING' — the connection is minted against a CREATING cluster, and a bogus VpcId is accepted too.
Why: a client that provisions a cluster and immediately wires private connectivity gets a false green in the emulator and a 400 in AWS; retry logic that waits for the state gate never triggers, and the emulator accumulates connections AWS would never have created.
Fix: a cluster-state precondition at the top of CreateVpcConnection's handler, evaluated before VpcId/subnet resolution.
Traces: kafka.vpcconnection-requires-multi-vpc-enabled-cluster.

Cause 2 of 3: CreateVpcConnection against a cluster that has no SASL/IAM client authentication (multi-VPC private connectivity not enabled) is accepted.
AWS: a VPC connection requires the cluster to have the corresponding client-authentication scheme enabled for VPC connectivity; with Authentication='SASL_IAM' against a cluster created without SASL/IAM, MSK refuses with BadRequestException and the fixed message 'SASL/IAM client authentication must be enabled for the cluster.' No connection is created, so the cluster's VPC-connection list is unchanged. (The same check further shows that once a cluster policy explicitly denies kafka:CreateVpcConnection, the call is refused AHEAD of this validation with AccessDeniedException whose message is AWS's standard explicit-deny template 'User: <signing principal ARN> is not authorized to perform: kafka:CreateVpcConnection on resource: <the cluster ARN> with an explicit deny in a resource-based policy', and reverts to the BadRequestException above after DeleteClusterPolicy — that ordering is unobservable while the baseline call succeeds.)
Emulator: HTTP 200 with a VpcConnectionArn, State='CREATING', Authentication='SASL_IAM'.
Why: the emulator reports private connectivity as available on a cluster that cannot serve it, and because the baseline call succeeds the resource-policy deny that should pre-empt it is never exercised — a cluster-policy enforcement test passes vacuously.
Fix: the client-authentication precondition in CreateVpcConnection's handler (after the state gate, before minting), and the resource-policy authorization decision that must precede it.
Traces: kafka.cluster-policy-enforced-on-cross-account-vpc-connection.

Cause 3 of 3: a TargetClusterArn that is not a cluster ARN answers a parameter error instead of MSK's authorization denial.
AWS: MSK evaluates authorization for kafka:CreateVpcConnection before resolving the target, so a caller without permission for the action on the resource is refused with AccessDeniedException whose message is the standard template 'User: <signing principal ARN> is not authorized to perform: kafka:CreateVpcConnection on resource: <the resource the request named>' — fixed connectives with the principal and resource taken from the request/caller identity; the malformed ARN is never reported as a parameter problem and the target's existence is not disclosed.
Emulator: ('BadRequestException', 'One or more of the parameters are not valid.') — the parameter validation pre-empts the authorization decision, so the status class (400) is wrong where AWS answers 403.
Why: a client that branches on 403 versus 400 (escalate to an operator versus fix the request) takes the opposite branch, and IAM negative tests for this operation assert on a class the emulator never produces.
Fix: run the authorization decision for kafka:CreateVpcConnection before TargetClusterArn validation in the handler, emitting the templated access-denied refusal when it denies.
Traces: kafka.vpc-connection-malformed-target-cluster-rejected.

## Evidence
All three checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

Cause 1 — kafka.vpcconnection-requires-multi-vpc-enabled-cluster, CreateVpcConnection(TargetClusterArn=<a CREATING cluster>, Authentication='SASL_IAM', VpcId=<real>, ClientSubnets=[2], SecurityGroups=[1]):
  Expected: ('BadRequestException', 'Invalid cluster state. To create, delete or reject a VPC connection, your cluster must be in one of these states: ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER.') for BOTH a real VpcId and VpcId='vpc-00000000deadbeef', and ListClientVpcConnections == [].
  Actual: HTTP 200 {'VpcConnectionArn': 'arn:aws:kafka:us-west-2:000000000000:vpc-connection/000000000000/sim2real-behav-shared-<id>/<uuid>-1', 'State': 'CREATING', 'Authentication': 'SASL_IAM', ...} (ids per-run).

Cause 2 — kafka.cluster-policy-enforced-on-cross-account-vpc-connection, same call against an ACTIVE cluster created WITHOUT SASL/IAM:
  Expected baseline: ('BadRequestException', 'SASL/IAM client authentication must be enabled for the cluster.'); then with an explicit-deny cluster policy: ('AccessDeniedException', 'User: <caller ARN> is not authorized to perform: kafka:CreateVpcConnection on resource: <cluster ARN> with an explicit deny in a resource-based policy'); after DeleteClusterPolicy, back to the BadRequestException.
  Actual: HTTP 200 {'VpcConnectionArn': 'arn:aws:kafka:us-west-2:000000000000:vpc-connection/000000000000/sim2real-behav-msk-<id>/<uuid>-1', 'State': 'CREATING', 'Authentication': 'SASL_IAM', ...} — the check aborts at the baseline, so the deny step is never measured.

Cause 3 — kafka.vpc-connection-malformed-target-cluster-rejected:
  Expected: ('AccessDeniedException', 'User: arn:aws:iam::<account>:user/<principal> is not authorized to perform: kafka:CreateVpcConnection on resource: *') — the principal ARN and resource are values filling AWS's template.
  Actual: ('BadRequestException', 'One or more of the parameters are not valid.').

Smithy cited: CreateVpcConnection POST /v1/vpc-connection code 200; input TargetClusterArn, Authentication, VpcId, ClientSubnets, SecurityGroups required, Tags optional; errors BadRequestException (client, 400), ForbiddenException (403), UnauthorizedException (401), TooManyRequestsException (429), InternalServerErrorException (500), ServiceUnavailableException (503). Output State is a VpcConnectionState enum.

## Steps to Reproduce
1. Boot the emulator; discover a VPC, two distinct-AZ subnets and a security group.
2. Cause 1: CreateClusterV2 a PROVISIONED cluster and do NOT wait (State CREATING). Call CreateVpcConnection against it with Authentication='SASL_IAM' and a real VpcId; assert 400 BadRequestException with the 'Invalid cluster state. To create, delete or reject a VPC connection, ...' sentence. Repeat with VpcId='vpc-00000000deadbeef' and assert the identical refusal. Assert ListClientVpcConnections(ClusterArn=...) is empty.
3. Cause 2: create a cluster WITHOUT SASL/IAM client authentication, wait for ACTIVE, then CreateVpcConnection with Authentication='SASL_IAM'. Assert 400 BadRequestException 'SASL/IAM client authentication must be enabled for the cluster.' and that ListVpcConnections is unchanged. Then PutClusterPolicy with an explicit Deny of kafka:CreateVpcConnection for the caller and assert the same call now answers AccessDeniedException with the explicit-deny template; DeleteClusterPolicy and assert it reverts to the BadRequestException.
4. Cause 3: CreateVpcConnection with TargetClusterArn='not-an-arn'. Assert 403 AccessDeniedException with 'User: <principal> is not authorized to perform: kafka:CreateVpcConnection on resource: *' and NOT 400 'One or more of the parameters are not valid.'
5. Replay kafka.vpcconnection-requires-multi-vpc-enabled-cluster, kafka.cluster-policy-enforced-on-cross-account-vpc-connection and kafka.vpc-connection-malformed-target-cluster-rejected.
