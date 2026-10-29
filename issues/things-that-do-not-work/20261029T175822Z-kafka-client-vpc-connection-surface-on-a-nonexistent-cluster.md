Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.client-vpc-connection-state-transition
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.client-vpc-connection-state-transition|run#8|AssertionError|assert observed == expected, f"observed={observed}"
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# Client-VPC-connection surface on a nonexistent cluster: ListClientVpcConnections answers BadRequestException instead of NotFoundException, and RejectClientVpcConnection is still a 501 stub (2 causes)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClientVpcConnections

## Description
TWO distinct root causes on the inbound client-VPC-connection surface, both reached by one check. Both must be closed.

Cause 1 of 2 - ListClientVpcConnections does not resolve its ClusterArn to a not-found refusal.
AWS: Smithy kafka#ListClientVpcConnections (GET /v1/clusters/{ClusterArn}/client-vpc-connections) takes ClusterArn as a required httpLabel. Its modeled error list is BadRequestException/Forbidden/InternalServerError/ServiceUnavailable/Unauthorized - NotFoundException is NOT in the model for this operation, which is exactly why the recorded behavior has to be stated rather than read off the model: recorded behavior only - a well-formed but never-issued cluster ARN is refused with Error.Code 'NotFoundException' (serialized as an unmodeled not-found, i.e. the cluster is resolved before the listing is served).
Emulator: Error.Code 'BadRequestException' - the ARN is treated as bad input rather than as naming a cluster that does not exist. (A fixed issue, 20261027T024105Z-kafka-listclientvpcconnections-is-an-unimplemented-scaffold-stub.md, stood this operation up from a 501; it did not add the cluster resolution, so the ghost-cluster path is still wrong.)
Why: the whole point of this operation for a client is to enumerate inbound connections on a cluster it believes exists. 'NotFoundException' tells it the cluster is gone (re-resolve, or stop polling); 'BadRequestException' tells it the ARN it is holding is malformed, which it is not, so a correct client retries forever or crashes on an input it built correctly.
Fix: resolve ClusterArn against cluster state in services/service-com-amazonaws-kafka-kafka/src/ops/list_client_vpc_connections.rs and refuse an unresolvable one with the service's NotFoundException, before any listing or pagination work.
Traces: kafka.client-vpc-connection-state-transition.

Cause 2 of 2 - RejectClientVpcConnection is an unimplemented scaffold stub.
AWS: Smithy kafka#RejectClientVpcConnection (PUT /v1/clusters/{ClusterArn}/client-vpc-connection) takes ClusterArn (httpLabel, required) and VpcConnectionArn (required) and models BadRequestException (400), ForbiddenException (403), InternalServerErrorException (500), ServiceUnavailableException (503) and UnauthorizedException (401), with an empty success body at 200. Recorded behavior only: rejecting a VpcConnectionArn that is not a PENDING inbound connection on that cluster - here, a well-formed but never-issued connection ARN on a never-issued cluster ARN - is REFUSED with Error.Code 'NotFoundException'; it must not report a successful REJECTED transition.
Emulator: HTTP 501 with Error.Code 'NotImplemented' and the message 'operation RejectClientVpcConnection is scaffolded but not implemented in this emulator'.
Why: 501 is not any of the operation's modeled errors, so exception-type dispatch in every SDK fails and no retry or not-found branch fires; and because the operation never runs, an emulator cannot be used to test the reject path of the multi-VPC connection workflow at all. The suite did not mark this as an already-filed stub (no filed issue names this operation), so it is reported here.
Fix: implement services/service-com-amazonaws-kafka-kafka/src/ops/reject_client_vpc_connection.rs - resolve ClusterArn and VpcConnectionArn against state, refuse with NotFoundException when the named connection is not a PENDING inbound connection on that cluster, and only transition a genuinely PENDING connection to REJECTED on success.
Traces: kafka.client-vpc-connection-state-transition.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

kafka.client-vpc-connection-state-transition (mismatch). Positive control first: list_vpc_connections() succeeds and returns 'VpcConnections', so the credentials and the kafka surface are healthy and both refusals below are specific to the ARNs under test.
  ghost   = 'arn:aws:kafka:us-west-2:<acct>:cluster/sim2real-behav-ghost-cluster-<rand>/<uuid>-2'   (well-formed, never issued)
  unassoc = 'arn:aws:kafka:us-west-2:<acct>:vpc-connection/<acct>/sim2real-behav-ghost-vpc-<rand>/<uuid>-7'
  list_client_vpc_connections(ClusterArn=ghost)                                  expected 'NotFoundException'
  reject_client_vpc_connection(ClusterArn=ghost, VpcConnectionArn=unassoc)       expected 'NotFoundException'
  actual  AssertionError: observed={'ListClientVpcConnections': 'BadRequestException', 'RejectClientVpcConnection': 'NotImplemented'}

The RejectClientVpcConnection arm is the scaffold stub message 'operation RejectClientVpcConnection is scaffolded but not implemented in this emulator' (HTTP 501).

## Steps to Reproduce
1. Boot the emulator.
2. Positive control: kafka.list_vpc_connections() -> expect 200 with a 'VpcConnections' member.
3. Cause 1: kafka.list_client_vpc_connections(ClusterArn='arn:aws:kafka:<region>:<account>:cluster/never-issued/00000000-0000-0000-0000-000000000000-2') -> expect Error.Code 'NotFoundException'. Observe 'BadRequestException'.
4. Cause 2: kafka.reject_client_vpc_connection(ClusterArn=<the same never-issued cluster ARN>, VpcConnectionArn='arn:aws:kafka:<region>:<account>:vpc-connection/<account>/never-issued/00000000-0000-0000-0000-000000000000-7') -> expect Error.Code 'NotFoundException' and NO successful REJECTED transition. Observe HTTP 501 'NotImplemented'.
5. Re-replay kafka.client-vpc-connection-state-transition.
