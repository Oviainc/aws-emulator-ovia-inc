Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.concurrent-vpc-connection-duplicate-create-race
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.concurrent-vpc-connection-duplicate-create-race|_enable_multi_vpc#2|Timeout|time.sleep(interval_s)
Issue-Commit: e791f41e88db95f6b542a847da3090e11dbf22b2

# kafka ListClientVpcConnections is an unimplemented scaffold stub: HTTP 501 NotImplemented where MSK returns the cluster's client VPC connections

> _Note: this issue was identified against the code base at commit `e791f41e88db95f6b542a847da3090e11dbf22b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClientVpcConnections

## Description
ONE root cause.

Cause 1 of 1: ListClientVpcConnections is not implemented, so a cluster's client VPC connections can never be read back.

AWS: Smithy kafka#ListClientVpcConnections — GET /v1/clusters/{ClusterArn}/client-vpc-connections, success HTTP 200, output ListClientVpcConnectionsResponse { ClientVpcConnections: list<ClientVpcConnection>, NextToken }, paginated (inputToken NextToken / outputToken NextToken / items ClientVpcConnections / pageSize MaxResults, MaxResults @range 1..100). Each ClientVpcConnection carries VpcConnectionArn (required), Authentication, CreationTime, State (VpcConnectionState) and Owner. The operation's modeled errors are exactly BadRequestException (400), ForbiddenException (403), UnauthorizedException (401), InternalServerErrorException (500) and ServiceUnavailableException (503) — there is no NotImplemented error in the closure of this operation, so no AWS response can carry that code. Behaviourally: every VPC connection CreateVpcConnection minted against a target cluster appears exactly once in that cluster's listing, with no merged, duplicated or missing entry; the emulator already stores exactly that record (crate::state::VpcConnection, registered through register_vpc_connection and already rendered by ListVpcConnections/DescribeVpcConnection), so the data this listing must answer from exists.

Emulator: the handler is still the generated scaffold stub — it binds input/ctx/engine to `_` and returns `crate::scaffolded::Scaffolded::not_implemented("ListClientVpcConnections")`, i.e. HTTP 501 with Error.Code 'NotImplemented' and the message whose fixed text is "operation <OperationName> is scaffolded but not implemented in this emulator" (only the operation name varies). No per-cluster listing of client VPC connections is possible, even though the connections themselves are created and stored.

Why: a client that creates a multi-VPC (PrivateLink) connection cannot enumerate the connections of the cluster it targeted — the documented way both the cluster owner and tooling discover pending/available client connections (and the only way to confirm that two independently created connections are two distinct connections, since CreateVpcConnection has no idempotency token). 501/NotImplemented is unmodeled, so botocore cannot dispatch it to any of this operation's exception types, no retry or not-found branch applies, and a client cannot distinguish "no connections" from "endpoint missing".

Fix: services/service-com-amazonaws-kafka-kafka/src/ops/list_client_vpc_connections.rs — implement `handle` over the existing VPC-connection registry in src/state.rs (filter by TargetClusterArn and request region, settle each record so a connection past VPC_CONNECTION_ACTIVATION_DELAY reports AVAILABLE, render ClientVpcConnection, honour MaxResults/NextToken paging), and delete the SCAFFOLDED marker at the top of the file.

Traces: kafka.concurrent-vpc-connection-duplicate-create-race. The reported failure mode for that check is "Check timed out after 60.0s", i.e. it did not reach its final ListClientVpcConnections assertion within the suite's per-check budget; the stub above is nevertheless a hard blocker for it, because the check's closing assertion is that both freshly created VpcConnectionArns appear in ListClientVpcConnections(ClusterArn=...), which this handler can only answer 501 to. Hypothesis (neither modeled nor recorded, stated as such): the 60 s overrun comes from the check running two sequential settle waits (CreateClusterV2 -> ACTIVE, then UpdateConnectivity -> UPDATE_COMPLETE) whose poll interval is 30 s each, while the emulator's clock-owed transitions (CLUSTER_ACTIVATION_DELAY / CLUSTER_UPDATE_DELAY, both 500 ms) are deliberately non-instant — so each wait costs one 30 s sleep. That part is timing of the runner's cadence, not a fidelity divergence, and the requirement filed here is the 501 above.

## Evidence
Failing check: kafka.concurrent-vpc-connection-duplicate-create-race (verdict mismatch) — "Error: Check timed out after 60.0s". Suite: simparity-behavioral-ga-prod-v1, kafka 21/84 passed.

The check's closing step, which can never be satisfied against this emulator:
  kafka.create_vpc_connection(TargetClusterArn=<cluster>, Authentication='SASL_IAM', VpcId=<vpc>, ClientSubnets=[3 subnets], SecurityGroups=[<sg>])  x2 concurrently
  kafka.list_client_vpc_connections(ClusterArn=<cluster>)
  expected: HTTP 200 with ClientVpcConnections containing BOTH minted VpcConnectionArns (each once, distinct ARNs)
  actual:   HTTP 501, Error.Code 'NotImplemented', message 'operation ListClientVpcConnections is scaffolded but not implemented in this emulator'

Handler state in the checkout (services/service-com-amazonaws-kafka-kafka/src/ops/list_client_vpc_connections.rs, lines 150-167): `let _ = (&input, ctx, &self.engine);` followed by `Err(crate::scaffolded::Scaffolded::not_implemented("ListClientVpcConnections"))` — the scaffold stub, unchanged.

For contrast, the record the listing would answer from already exists: services/service-com-amazonaws-kafka-kafka/src/state.rs documents the multi-VPC connection registry (VpcConnection::requested, register_vpc_connection, vpc_connection_in, VpcConnection::settle CREATING->AVAILABLE at VPC_CONNECTION_ACTIVATION_DELAY = 500 ms) and renders it for ListVpcConnections/DescribeVpcConnection.

No open or fixed issue covers ListClientVpcConnections: the suite's ALREADY FILED stub notices for this run name CreateInternetGateway (ec2), RebootBroker, BatchAssociateScramSecret, ListScramSecrets and UpdateClusterKafkaVersion only, and no issue listing matches 'ListClientVpcConnections'. The related open issue 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md covers DescribeClusterV2, CreateConfiguration, CreateVpcConnection and DescribeTopicPartitions — not this operation.

## Steps to Reproduce
1. Boot the emulator (branch mainline).
2. Create a provisioned MSK cluster and wait for it to leave CREATING:
   arn = kafka.create_cluster_v2(ClusterName='vpcrace-1', Provisioned={'KafkaVersion':'3.8.x','NumberOfBrokerNodes':3,'ClientAuthentication':{'Sasl':{'Iam':{'Enabled':True}}},'EncryptionInfo':{'EncryptionInTransit':{'ClientBroker':'TLS','InCluster':True}},'BrokerNodeGroupInfo':{'InstanceType':'express.m7g.large','ClientSubnets':[<3 subnets in 3 AZs>],'SecurityGroups':[<sg>]}})['ClusterArn']; poll kafka.describe_cluster_v2(ClusterArn=arn) until State == 'ACTIVE'.
3. Enable multi-VPC private connectivity: kafka.update_connectivity(ClusterArn=arn, CurrentVersion=<describe_cluster CurrentVersion>, ConnectivityInfo={'VpcConnectivity':{'ClientAuthentication':{'Sasl':{'Iam':{'Enabled':True}}}}}); poll kafka.describe_cluster_operation_v2(ClusterOperationArn=<returned>) until OperationState == 'UPDATE_COMPLETE'.
4. Call kafka.create_vpc_connection(TargetClusterArn=arn, Authentication='SASL_IAM', VpcId=<client vpc>, ClientSubnets=[<3 subnets>], SecurityGroups=[<client sg>]) twice with identical parameters (concurrently or sequentially) and keep both VpcConnectionArns.
5. Call kafka.list_client_vpc_connections(ClusterArn=arn) -> expect HTTP 200 with ClientVpcConnections containing both ARNs, each with Authentication, CreationTime and State; observe HTTP 501 Error.Code 'NotImplemented' ('operation ListClientVpcConnections is scaffolded but not implemented in this emulator').
6. Also exercise paging once implemented: kafka.list_client_vpc_connections(ClusterArn=arn, MaxResults=1) -> expect one item plus a NextToken that pages to the second.
7. Re-replay kafka.concurrent-vpc-connection-duplicate-create-race.
