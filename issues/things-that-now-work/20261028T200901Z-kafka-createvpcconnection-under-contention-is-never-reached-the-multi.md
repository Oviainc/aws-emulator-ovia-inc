Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.concurrent-vpc-connection-duplicate-create-race
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.concurrent-vpc-connection-duplicate-create-race|_enable_multi_vpc#2|Timeout|time.sleep(interval_s)
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# CreateVpcConnection under contention is never reached: the multi-VPC setup sequence does not resolve inside the check's 60 s ceiling

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcConnection

## Description
Cause 1 of 1: the behavioral check kafka.concurrent-vpc-connection-duplicate-create-race (GROUP "concurrency") never produces a verdict against this emulator — the replay ends with "Check timed out after 60.0s" and no exception, so no request was refused; the run simply did not finish. The consequence is that CreateVpcConnection's behaviour under contention is UNVERIFIED by this suite today.

AWS: CreateVpcConnection (POST /v1/vpc-connection, 200) carries NO idempotency token in its model — input is TargetClusterArn, Authentication, VpcId, ClientSubnets, SecurityGroups, Tags only (Smithy: CreateVpcConnectionRequest). So two simultaneous, byte-identical CreateVpcConnection calls against one TargetClusterArn must EACH succeed with 200 and EACH return a DISTINCT VpcConnectionArn; neither may be refused and neither may be merged into the other. Both connections must then appear, exactly once each, in ListClientVpcConnections (Smithy: ListClientVpcConnections -> ClientVpcConnections, GET /v1/clusters/{ClusterArn}/client-vpc-connections). The modeled refusals for this operation are BadRequestException 400, ForbiddenException 403, UnauthorizedException 401, TooManyRequestsException 429, InternalServerErrorException 500, ServiceUnavailableException 503 — contention is not among them. Recorded behavior only for the timing of the asynchronous transitions: AWS reports a freshly created cluster as CREATING and an UpdateConnectivity cluster operation as UPDATE_IN_PROGRESS until the asynchronous work completes.

Emulator: the two concurrency assertions are never evaluated. The setup the check must complete first is (a) CreateClusterV2 then poll DescribeClusterV2 until State==ACTIVE, and (b) UpdateConnectivity then poll DescribeClusterOperationV2 until OperationState==UPDATE_COMPLETE; only then are the two concurrent CreateVpcConnection calls issued. Hypothesis (grounded in the checkout, not in the replay output): the emulator makes each of those two transitions clock-owed rather than complete-on-return — services/service-com-amazonaws-kafka-kafka/src/state.rs sets a new cluster to CLUSTER_STATE_CREATING with active_due = now + CLUSTER_ACTIVATION_DELAY (500 ms) and Cluster::settle only promotes it to ACTIVE at a later read, and ClusterOperation::operation_state returns OPERATION_STATE_IN_PROGRESS ("UPDATE_IN_PROGRESS") until end_due = now + CLUSTER_UPDATE_DELAY (500 ms). A caller that probes immediately after each write therefore always observes the non-terminal value on its FIRST probe and is charged its whole poll interval; the check polls at 30 s, so the two transitions cost ~60.5 s and the 60.0 s ceiling fires first. An alternative this evidence does NOT rule out is a genuine stall in the concurrent path itself (two simultaneous CreateVpcConnection calls, or ListClientVpcConnections immediately after them) — the replay reports only the timeout, with no frame, so the stall site is not localized.

Why: a client gets no coverage at all of the one behaviour that matters here — that an API with no idempotency token mints two independent connections under contention. If the stall is in the concurrent path, a real client issuing two parallel CreateVpcConnection calls hangs or loses one of the two connections, and every ARN it holds afterwards is unaccounted for in ListClientVpcConnections. If it is the clock-owed settle, then any client that polls at an AWS-appropriate cadence pays a full interval per transition that the emulator had in fact already finished, which makes emulator-backed test suites time out on sequences AWS completes.

Fix: belongs in the kafka service's cluster/VPC-connection state settling and in the CreateVpcConnection handler's concurrency path — the clock-owed transition instants for a created cluster and for a cluster operation (the cluster record and the cluster-operation record), and the registry insertion CreateVpcConnection performs under the state lock while minting each connection's distinct ARN. Re-replay the check to localize whether it is the settle cadence or the concurrent create/list path.

Traces: kafka.concurrent-vpc-connection-duplicate-create-race (verdict mismatch, "Check timed out after 60.0s"). File and line citations used while reasoning: services/service-com-amazonaws-kafka-kafka/src/state.rs lines 151, 262, 4432-4433, 4683-4686, 1186-1192; services/service-com-amazonaws-kafka-kafka/src/ops/create_vpc_connection.rs lines 231-317.

Not covered by an existing open issue: 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md covers operations that answer 501, and this check received no response at all; 20261027T231320Z-kafka-scram-secret-operations-are-over-permissive-no.md and 20261026T085017Z-kafka-describeconfigurationrevision-refuses-an-unissued-revision-number-with-notfoundexception.md concern SCRAM secrets and configuration revisions respectively.

## Evidence
Expected (AWS): both concurrent CreateVpcConnection calls return HTTP 200; the two VpcConnectionArn values differ; each appears exactly once in ListClientVpcConnections for the target cluster. No error code is emitted — contention is not a modeled refusal for this operation and the request carries no idempotency token.

Actual (emulator): the check produces no verdict. Replay detail, verbatim: "Error: Check timed out after 60.0s" — no HTTP status, no error code, no traceback frame, so nothing was refused and nothing was asserted. The two assertions in the check ("expected both concurrent CreateVpcConnection calls to succeed independently" and "expected two distinct VpcConnectionArns") are never reached.

Supporting evidence from the checkout for the leading hypothesis: state.rs CLUSTER_ACTIVATION_DELAY = 500 ms and a new cluster is stored as CREATING with active_due = now + 500 ms (lines 151, 4432-4433); Cluster::settle promotes CREATING -> ACTIVE only when a later read's `now` has passed active_due (lines 4683-4686); CLUSTER_UPDATE_DELAY = 500 ms and ClusterOperation::operation_state answers "UPDATE_IN_PROGRESS" until end_due (lines 262, 1186-1192). The check's first probe after each write lands inside that window, and it then sleeps its 30 s interval, twice.

## Steps to Reproduce
1. Boot the emulator and point a kafka client at it.
2. CreateClusterV2 a 3-broker provisioned cluster with Sasl/Iam enabled across 3 AZ subnets, then poll DescribeClusterV2 until ClusterInfo.State == ACTIVE.
3. UpdateConnectivity on that cluster with ConnectivityInfo.VpcConnectivity.ClientAuthentication.Sasl.Iam.Enabled = true, using the cluster's CurrentVersion; poll DescribeClusterOperationV2 on the returned ClusterOperationArn until OperationState == UPDATE_COMPLETE.
4. From two threads simultaneously, issue the SAME CreateVpcConnection request (TargetClusterArn = the cluster, Authentication = "SASL_IAM", VpcId, ClientSubnets, SecurityGroups).
5. Observe: both calls must return 200 with two DISTINCT VpcConnectionArn values, and ListClientVpcConnections(ClusterArn=...) must list both exactly once. Against the emulator the whole sequence does not complete within 60 s, so step 4's outcome is never observed.
6. To separate the two candidate causes, re-run step 2 and step 3 probing immediately after each write and timing how long DescribeClusterV2 reports CREATING and DescribeClusterOperationV2 reports UPDATE_IN_PROGRESS; then run step 4 alone against an already-prepared cluster and time the two concurrent calls.
