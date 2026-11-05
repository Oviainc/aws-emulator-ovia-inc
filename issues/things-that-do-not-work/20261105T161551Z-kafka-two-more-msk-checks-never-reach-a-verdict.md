Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.update-monitoring-server-enforced kafka.topic-partition-message-ordering-preserved
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.update-monitoring-server-enforced|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: kafka.topic-partition-message-ordering-preserved|_wait_cluster_active#4|Timeout|time.sleep(interval_s)
Issue-Commit: 5ca9b2ecf350619dd6179a85533056a243174605

# Two more MSK checks never reach a verdict: the UpdateMonitoring lifecycle and the single-partition ordering round-trip both time out (2 causes)

> _Note: this issue was identified against the code base at commit `5ca9b2ecf350619dd6179a85533056a243174605`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMonitoring

## Description
TWO checks produced no verdict at all — both report only 'Check timed out after 60.0s' with no HTTP status, no error code and no traceback frame, so nothing was refused and no assertion was evaluated. They are TWO distinct root causes and each must be closed separately; both must be closed before this issue is submitted. Both root causes are already described for OTHER check ids by open issues that carry neither of these two ids, so neither of these checks is covered today:
  - 20261104T171616Z-kafka-msk-asynchronous-transitions-are-clock-owed-so-an.md (14 other control-plane lifecycle checks)
  - 20261104T171616Z-kafka-no-kafka-broker-answers-on-the-advertised-bootstrap.md (lists kafka.topic-partition-message-ordering-preserved in its trace ids, but its recorded replay key for that id is a different frame, so the replay no longer binds this check to it and the suite reports it unowned)

Cause 1 of 2: the UpdateMonitoring lifecycle sequence cannot complete, because every MSK transition is clock-owed rather than complete-on-return.
AWS: CreateClusterV2 returns with the cluster CREATING and the cluster reaches ACTIVE; UpdateMonitoring on the ACTIVE cluster returns a ClusterOperationArn whose DescribeClusterOperationV2 OperationState progresses UPDATE_IN_PROGRESS -> UPDATE_COMPLETE, after which DescribeClusterV2 reports the new Provisioned.EnhancedMonitoring. A client therefore polls DescribeClusterV2 at ~30 s and the operation at ~10-20 s, and the whole sequence (create -> ACTIVE -> update -> UPDATE_COMPLETE -> re-read -> stale-version refusal -> update again -> UPDATE_COMPLETE) completes in a handful of intervals. (Recorded behavior for the cadence; Smithy: UpdateMonitoring returns ClusterArn + ClusterOperationArn and models BadRequestException/NotFoundException paths.)
Emulator: each transition is owed to a clock instead of being settled on return, so the FIRST probe after every write always observes the non-terminal value and the caller is charged a full poll interval per transition. With three transitions to wait on, the check exhausts its budget inside the create/ACTIVE and operation polls and never reaches the behaviours it exists to verify: that a stale CurrentVersion is refused with BadRequestException 'The version of the cluster isn’t current. Check the current version and try again.' and changes nothing, that the identical call with the current version then applies PER_TOPIC_PER_PARTITION, and that UpdateMonitoring on a well-formed-but-nonexistent cluster ARN answers NotFoundException 'The requested resource doesn’t exist.' in both the plain and the OpenMonitoring form. Hypothesis (not stated by the replay output): the same clock-owed settling the related issue above describes; an alternative this evidence does not rule out is that the UPDATE_CLUSTER_MONITORING operation never reaches a terminal state at all.
Why: a client polling at an AWS-appropriate cadence pays a full interval per transition the emulator had in fact already finished, so emulator-backed suites stall on sequences AWS completes; and UpdateMonitoring's optimistic-lock and not-found contracts are unverifiable.
Fix: make the transition instants in the kafka service's state layer complete-on-return (or settle them on the write path) for a created cluster and for a cluster operation, so a first probe after a write observes the terminal value; then confirm the UPDATE_CLUSTER_MONITORING operation does reach UPDATE_COMPLETE.
Traces: kafka.update-monitoring-server-enforced.

Cause 2 of 2: the single-partition ordering round-trip never connects to a broker, because nothing serves the Kafka wire protocol the control plane advertises.
AWS: an ACTIVE MSK cluster with public access and SASL/SCRAM enabled serves the Kafka protocol on the endpoint GetBootstrapBrokers returns in BootstrapBrokerStringPublicSaslScram; a TCP connect is accepted, the SASL/SCRAM handshake completes, and records produced in order to a single-partition topic are consumed back in exactly that order (the per-partition FIFO guarantee). CreateAcls over the public SASL/IAM listener grants the SCRAM principal. (Recorded behavior for the data-plane semantics; the control-plane model says only that the endpoints exist.)
Emulator: the sequence (provision VPC/subnets/SG, create the SCRAM+IAM cluster, wait ACTIVE, UpdateConnectivity to SERVICE_PROVIDED_EIPS, create the topic, grant ACLs over the advertised SASL/IAM endpoint, then produce and consume over the advertised SASL/SCRAM endpoint) never completes: the check ends with the bare 60 s timeout having asserted nothing. Because the emulator announces bootstrap addresses, this is the emulator's own egress gap, not the runner's.
Why: per-partition ordering — the one guarantee a faked data plane cannot meet — is unverifiable, and a client that configures itself from GetBootstrapBrokers hangs until its own timeout.
Fix: the kafka service must serve the Kafka wire protocol (broker listener plus per-scheme SASL authentication and the record/partition/offset state the produce and fetch paths read and write) on the ports it advertises, or advertise only endpoints it serves; the cluster/connectivity transitions this check waits on first must also settle (Cause 1).
Traces: kafka.topic-partition-message-ordering-preserved.

## Evidence
Both checks are kafka behavioral checks with verdict mismatch and the identical detail 'Error: Check timed out after 60.0s' — no status, no error code, no frame. Neither is marked ALREADY FILED, and the suite reports neither as OWNED by an open issue.

Cause 1 — kafka.update-monitoring-server-enforced. Read from its source, the poll cadences are AWS-sized and are what the 60 s ceiling cannot absorb: _wait_cluster_v2_active polls DescribeClusterV2 at 30 s until ACTIVE/FAILED, then _wait_operation_complete polls DescribeClusterOperationV2 at 10 s until OperationState ends in _COMPLETE/_FAILED, and that operation poll is paid TWICE (the PER_TOPIC_PER_BROKER update and the PER_TOPIC_PER_PARTITION restore).
  Expected (AWS), none of which was reached: UpdateMonitoring on the ACTIVE cluster returns ClusterArn == the cluster's ARN plus a ClusterOperationArn that reaches UPDATE_COMPLETE, after which DescribeClusterV2 reports Provisioned.EnhancedMonitoring == 'PER_TOPIC_PER_BROKER'; re-using the pre-update CurrentVersion is then refused with ('BadRequestException', 'The version of the cluster isn’t current. Check the current version and try again.') and leaves EnhancedMonitoring at PER_TOPIC_PER_BROKER; the same call with the fresh version applies 'PER_TOPIC_PER_PARTITION'; and UpdateMonitoring against a well-formed-but-nonexistent cluster ARN answers ('NotFoundException', 'The requested resource doesn’t exist.') both with and without OpenMonitoring.
  Actual: no verdict.
  Message anatomy for the two expected refusals: both Messages are entirely FIXED text — the version value, the cluster ARN and the cluster name are NOT interpolated.

Cause 2 — kafka.topic-partition-message-ordering-preserved. The check creates its own VPC/subnets/SG, a 2-broker cluster with Sasl.Scram and Sasl.Iam and TLS, waits ACTIVE, enables public access with UpdateConnectivity, creates a 1-partition topic, grants ALLOW ALL ACLs for the SCRAM principal over BootstrapBrokerStringPublicSaslIam, then produces 25 ordered records and consumes them back over BootstrapBrokerStringPublicSaslScram.
  Expected (AWS): the consumed values equal the produced values in the same order (msg-000 .. msg-024 for this check's payloads — the count and payload text are the check's own, the requirement is that consume order equals produce order within the partition).
  Actual: no verdict; nothing was produced or consumed. The advertised host:port a given run sees is a per-run value; the requirement is that whatever endpoint GetBootstrapBrokers advertises accepts a connection and completes the handshake.

Prior art naming the same two root causes for OTHER check ids: 20261104T171616Z-kafka-msk-asynchronous-transitions-are-clock-owed-so-an.md and 20261104T171616Z-kafka-no-kafka-broker-answers-on-the-advertised-bootstrap.md.

## Steps to Reproduce
Cause 1:
1. Boot the emulator. CreateClusterV2 a PROVISIONED cluster (kafka.m5.large, 2 brokers, 2 subnets) and, IMMEDIATELY after the response, call DescribeClusterV2 once; record whether State is already ACTIVE or still CREATING and for how long.
2. Once ACTIVE, call UpdateMonitoring(CurrentVersion=<current>, EnhancedMonitoring='PER_TOPIC_PER_BROKER') and, immediately after the response, call DescribeClusterOperationV2 once; record whether OperationState is already UPDATE_COMPLETE.
3. Assert the full client sequence completes inside 60 s of wall clock at an AWS cadence (DescribeClusterV2 every 30 s, DescribeClusterOperationV2 every 10 s): create -> ACTIVE -> update -> UPDATE_COMPLETE -> DescribeClusterV2 shows PER_TOPIC_PER_BROKER -> stale-version refusal -> update with the fresh version -> UPDATE_COMPLETE -> DescribeClusterV2 shows PER_TOPIC_PER_PARTITION.
4. Then assert the refusals themselves: the pre-update CurrentVersion gives 400 BadRequestException 'The version of the cluster isn’t current. Check the current version and try again.' and changes nothing, and a well-formed-but-nonexistent cluster ARN gives 404 NotFoundException 'The requested resource doesn’t exist.' both with and without OpenMonitoring.
5. Replay kafka.update-monitoring-server-enforced and confirm it produces a pass/fail verdict rather than a timeout.

Cause 2:
1. Create a 2-broker cluster with ClientAuthentication Sasl.Scram+Sasl.Iam and TLS in transit, wait for ACTIVE, then UpdateConnectivity PublicAccess=SERVICE_PROVIDED_EIPS and wait for ACTIVE again.
2. Associate a SCRAM secret, call GetBootstrapBrokers and take BootstrapBrokerStringPublicSaslScram and BootstrapBrokerStringPublicSaslIam.
3. Open a plain TCP connection to each advertised host:port and assert it is ACCEPTED (today it never is).
4. Over the SASL/IAM endpoint grant ALLOW ALL topic and group ACLs for the SCRAM principal with CreateAcls; over the SASL/SCRAM endpoint create a 1-partition topic, produce N ordered records and consume them back, asserting the consumed sequence equals the produced sequence exactly.
5. Replay kafka.topic-partition-message-ordering-preserved and confirm it reaches a verdict.
