Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.configuration-delete-in-use-conflict kafka.replicator-create-invalid-role-rejected kafka.update-channel-wrong-state-rejected kafka.update-replication-info-concurrent-conflicting-updates kafka.vpc-connection-malformed-subnet-config-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.configuration-delete-in-use-conflict|run#17|AssertionError|assert message == (
Issue-Key: kafka.replicator-create-invalid-role-rejected|run#10|AssertionError|assert info["ReplicatorState"] == "FAILED", (
Issue-Key: kafka.update-channel-wrong-state-rejected|run#17|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: kafka.update-replication-info-concurrent-conflicting-updates|run#24|AssertionError|assert failures[0][2] == (
Issue-Key: kafka.vpc-connection-malformed-subnet-config-rejected|run#9|AssertionError|assert malformed_code == "BadRequestException", (
Issue-Commit: 9a655abc910364880d25d282d4207456228c1efc

# MSK control-plane refusals: two reworded messages, UpdateChannel 501, and two validations MSK performs that the emulator skips (5 causes)

> _Note: this issue was identified against the code base at commit `9a655abc910364880d25d282d4207456228c1efc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfiguration

## Description
FIVE distinct root causes, all in the kafka (MSK) control plane, all net-new: none of the five failing checks is marked ALREADY FILED and none is reported OWNED by an open issue. Each cause must be closed separately; all five must be closed before this issue is submitted.

Cause 1 of 5: DeleteConfiguration's in-use refusal carries emulator prose instead of MSK's message.
AWS: DeleteConfiguration on a configuration a cluster still references is refused with HTTP 400 BadRequestException (Smithy: DeleteConfiguration errors BadRequestException httpError 400, ForbiddenException 403, NotFoundException 404, InternalServerErrorException 500; BadRequestException carries message + invalidParameter) and the Message is the fixed text "Configuration is in use by one or more clusters. Dissociate the configuration from the clusters." — that sentence is entirely FIXED text: no configuration name, ARN, revision or cluster id is interpolated into it (recorded behavior for the wording). The refused delete has no effect: DescribeConfiguration still reports State ACTIVE.
Emulator: the error CLASS and status are right (BadRequestException), but the Message is the emulator's own sentence, "The configuration is in use by one or more clusters and can\u2019t be deleted."
Why: a client that matches on the refusal text (log scraping, retry/cleanup helpers that distinguish "dissociate first" from other 400s, snapshot tests of error payloads) does not recognise the emulator's sentence, and operators are told to do nothing rather than to dissociate the configuration.
Fix: the DeleteConfiguration handler's in-use conflict path in the kafka service — emit MSK's fixed message text for the in-use BadRequestException instead of locally authored prose.
Traces: kafka.configuration-delete-in-use-conflict (the check's positive control — deleting an UNATTACHED configuration, which returns State DELETING — passes, so this is only the message of the in-use refusal).

Cause 2 of 5: CreateReplicator accepts a nonexistent ServiceExecutionRoleArn and the replicator reaches RUNNING; MSK's asynchronous validation drives it to FAILED.
AWS: CreateReplicator is asynchronous — it returns ReplicatorState CREATING (Smithy: CreateReplicator output replicatorState, ReplicatorState enum = RUNNING|CREATING|UPDATING|DELETING|FAILED; ServiceExecutionRoleArn is required input) — and MSK then tries to assume the service-execution role. When that role does not exist, the replicator settles in FAILED, never RUNNING, and DescribeReplicator reports StateInfo (Smithy: ReplicationStateInfo { code, message }) whose message names the role / the failure to assume it; ListReplicators reports the same replicator as FAILED (recorded behavior for the terminal state and for the StateInfo being populated).
Emulator: with a well-formed but never-created IAM role ARN in this account, the replicator walks to RUNNING and DescribeReplicator returns StateInfo = None. Nothing ever validates the role, so a never-viable replicator is reported as healthy.
Why: a client cannot detect a misconfigured replicator at all — an emulator-backed test of "replicator failed because its role is wrong" passes where AWS fails, and no FAILED/StateInfo surface exists for any asynchronous replicator failure.
Fix: the kafka service's replicator state machine (the CreateReplicator handler's asynchronous settle path plus the replicator state it writes): validate ServiceExecutionRoleArn resolves/assumable when the replicator leaves CREATING, settle it in FAILED when it does not, and populate DescribeReplicator/ListReplicators StateInfo with a message naming the role. Related but distinct open issue (synchronous refusal of nonexistent cluster/subnet references, not the role): 20261029T232719Z-kafka-createreplicator-is-not-asynchronous-a-well-formed-but.md.
Traces: kafka.replicator-create-invalid-role-rejected.

Cause 3 of 5: UpdateChannel answers 501 NotImplemented on the wrong-state path where MSK answers a modeled 400.
AWS: UpdateChannel on a channel that is DELETING is refused with HTTP 400 BadRequestException (Smithy: UpdateChannel PUT /v1/clusters/{ClusterArn}/channels/{ChannelArn}; errors BadRequestException 400, NotFoundException 404, ForbiddenException 403, UnauthorizedException 401, TooManyRequestsException 429, ServiceUnavailableException 503) with the Message "Channel cannot be updated in state DELETING. Channel must be in ACTIVE state to be updated." In that sentence "Channel cannot be updated in state " / ". Channel must be in ACTIVE state to be updated." is fixed text and the state token (DELETING here) is the channel's CURRENT state from the emulator's own state; the channel then proceeds to disappear (DescribeChannel -> 404 NotFoundException), and the identical S3DestinationUpdate on an ACTIVE channel is ACCEPTED and leaves the channel ACTIVE with DataFreshnessInSeconds 900.
Emulator: the call answers error code 'NotImplemented' (HTTP 501), which is not in the operation's modeled error set, so neither the refusal nor the accepted positive control is exercisable.
Why: 501 is not a client error any SDK models for this operation — botocore surfaces it as an unmodeled failure rather than BadRequestException, so a client's state-guard handling and its happy-path channel update both break.
Fix: implement the UpdateChannel handler in the kafka service (services/service-com-amazonaws-kafka-kafka/src/ops/update_channel.rs) with the channel-state guard above. NOTE for the fixer: the open stub issue 20261104T171616Z-kafka-updatechannel-stub.md names the same unimplemented operation for OTHER check ids; it does not carry this check's replay key, so kafka.update-channel-wrong-state-rejected is unowned today and its wrong-state message and ACTIVE-channel positive control are the specific contract to satisfy.
Traces: kafka.update-channel-wrong-state-rejected.

Cause 4 of 5: UpdateReplicationInfo's stale-CurrentVersion refusal message is the emulator's own prose, with live version values interpolated.
AWS: when two UpdateReplicationInfo calls share the same CurrentVersion, exactly one wins and the loser is refused with HTTP 400 BadRequestException (Smithy: UpdateReplicationInfo currentVersion is required; errors BadRequestException 400 / NotFoundException 404 / ...) whose Message is the fixed text "Invalid parameter currentVersion: The version of the connector isn\u2019t current. Check the current version and try again." — entirely fixed text, with NO version ids interpolated (recorded behavior for the wording).
Emulator: the error class is right but the Message is "The replicator's current version is <stored-version>, not <submitted-version>." — locally authored prose that additionally leaks the two version tokens into the message body.
Why: a client matching MSK's optimistic-lock wording (or snapshotting the error payload) does not recognise the refusal, and the "Invalid parameter currentVersion:" prefix that tells a client WHICH parameter was stale is absent. Related open issue, which covers the same wording defect for OTHER operations and check ids (cluster/topic version conflicts), not UpdateReplicationInfo's: 20261104T171616Z-kafka-msk-state-version-conflict-duplicate-topic-and-invalid.md.
Fix: the UpdateReplicationInfo handler's CurrentVersion check in the kafka service — emit MSK's fixed currentVersion message instead of a formatted comparison of the stored and submitted versions.
Traces: kafka.update-replication-info-concurrent-conflicting-updates (the concurrency contract itself holds: exactly one winner, one BadRequestException loser, and DescribeReplicator shows only the winner's pattern — only the message diverges).

Cause 5 of 5: CreateVpcConnection accepts ClientSubnets that are not in the declared VpcId.
AWS: CreateVpcConnection requires targetClusterArn, authentication, vpcId, clientSubnets and securityGroups (Smithy: all five @required; errors BadRequestException 400, ForbiddenException 403, UnauthorizedException 401, TooManyRequestsException 429, ServiceUnavailableException 503 — there is no NotFoundException), and a request whose clientSubnets live in a DIFFERENT VPC than vpcId is refused with HTTP 400 BadRequestException and NO connection is created (ListVpcConnections shows none for the target cluster). A consistent vpcId/clientSubnets/securityGroups request on the same multi-VPC-enabled cluster creates a connection that reaches State AVAILABLE.
Emulator: the inconsistent request SUCCEEDS — it returns a VpcConnectionArn (the check's error helper reports code None, i.e. no ClientError) and the connection is minted, so the cross-VPC input is silently accepted.
Why: this is over-permissive: an application whose VPC-connection wiring is wrong (subnets from another VPC) passes against the emulator and is refused by AWS at deploy time, and the emulator leaves behind a connection AWS would never create.
Fix: the CreateVpcConnection handler's input validation in the kafka service — cross-check each entry of clientSubnets (and the securityGroups) against the declared vpcId before any state is written, and refuse with BadRequestException naming the offending parameter. Related open issue covering OTHER VPC-connection paths and check ids: 20261029T175822Z-kafka-client-vpc-connection-surface-on-a-nonexistent-cluster.md.
Traces: kafka.vpc-connection-malformed-subnet-config-rejected.

## Evidence
All five are kafka behavioral checks in this suite's failing list; none is marked ALREADY FILED and none is reported OWNED by an open issue.

Cause 1 — kafka.configuration-delete-in-use-conflict [mismatch]:
  AssertionError at check line 214: in-use DeleteConfiguration message: 'The configuration is in use by one or more clusters and can\u2019t be deleted.'
  Expected (AWS): 400 BadRequestException, Message == "Configuration is in use by one or more clusters. Dissociate the configuration from the clusters." (all fixed text).
  Actual: 400 BadRequestException with the emulator's sentence above. The preceding assertion on the error CODE passed, as did the unattached positive control (State == DELETING).

Cause 2 — kafka.replicator-create-invalid-role-rejected [mismatch]:
  AssertionError at check line 252: "a nonexistent ServiceExecutionRoleArn must never let the replicator reach RUNNING; got RUNNING: None" (the trailing None is DescribeReplicator's StateInfo).
  Expected (AWS): CreateReplicator returns ReplicatorState CREATING (this assertion passed), then DescribeReplicator settles on ReplicatorState == 'FAILED' with StateInfo.Message naming the role (the check requires 'role' or 'assume' to appear in it), and ListReplicators(ReplicatorNameFilter=<name>) reports that replicator as FAILED.
  Actual: ReplicatorState 'RUNNING', StateInfo absent. Both source and target clusters are genuinely ACTIVE/SASL-IAM in this check, so the only invalid input is the role ARN (a well-formed role ARN in the caller's account that was never created).

Cause 3 — kafka.update-channel-wrong-state-rejected [mismatch]:
  AssertionError at check line 64: "expected BadRequestException, got 'NotImplemented'".
  Expected (AWS): 400 BadRequestException with Message == "Channel cannot be updated in state DELETING. Channel must be in ACTIVE state to be updated." (fixed text around the channel's current state token), the channel then disappearing (DescribeChannel -> NotFoundException), and the identical UpdateChannel(S3DestinationUpdate={DataFreshnessInSeconds: 900}) on the ACTIVE control channel accepted, leaving it ACTIVE with DataFreshnessInSeconds 900.
  Actual: error code 'NotImplemented' (HTTP 501) — an error the operation does not model.

Cause 4 — kafka.update-replication-info-concurrent-conflicting-updates [mismatch]:
  AssertionError at check line 125: stale-version loser's message: "The replicator's current version is KB81IPB56798B9, not K49XKAUPB7MDWG."
  Expected (AWS): Message == "Invalid parameter currentVersion: The version of the connector isn\u2019t current. Check the current version and try again." — entirely fixed text with no version values.
  Actual: the emulator's sentence with the stored and submitted version ids interpolated (the two K... tokens above are this run's generated versions, not part of any requirement). The error code assertion (BadRequestException), the one-winner/one-loser assertion, and DescribeReplicator reflecting only the winner all passed.

Cause 5 — kafka.vpc-connection-malformed-subnet-config-rejected [mismatch]:
  AssertionError at check line 47: "expected BadRequestException for a subnet outside the declared VPC, got None" — the helper returns (None, <VpcConnectionArn>) when the call SUCCEEDS, so the emulator returned 200 with a VpcConnectionArn.
  Expected (AWS): 400 BadRequestException and no connection left behind (ListVpcConnections has no entry for the target cluster), then the consistent-subnet CreateVpcConnection reaching State AVAILABLE.
  Actual: 200 with a created VPC connection whose ClientSubnets are in a different VPC than the declared VpcId.

## Steps to Reproduce
Cause 1 (DeleteConfiguration in-use message):
1. CreateConfiguration twice (ServerProperties 'auto.create.topics.enable=true', KafkaVersions ['3.9.x']). DeleteConfiguration the first one unattached and confirm State == DELETING (positive control).
2. Create a PROVISIONED cluster, wait ACTIVE, UpdateClusterConfiguration to the second configuration (Revision 1) and wait for the operation to reach UPDATE_COMPLETE and the cluster ACTIVE again.
3. DeleteConfiguration(Arn=<attached configuration>) and assert 400 BadRequestException with Message exactly "Configuration is in use by one or more clusters. Dissociate the configuration from the clusters."
4. DescribeConfiguration and assert State is still ACTIVE.

Cause 2 (replicator with a nonexistent service-execution role):
1. Create two ACTIVE SASL/IAM clusters in the same region.
2. CreateReplicator over them with ServiceExecutionRoleArn = a well-formed role ARN in the account that was never created. Assert the response ReplicatorState == 'CREATING'.
3. Poll DescribeReplicator until ReplicatorState is RUNNING or FAILED; assert it is 'FAILED' and that StateInfo.Message mentions the role (contains 'role' or 'assume').
4. ListReplicators(ReplicatorNameFilter=<name>) and assert the replicator is reported FAILED.

Cause 3 (UpdateChannel on a DELETING channel):
1. Create an Express cluster across three AZ subnets, a 1-partition topic, and an S3 channel destination (bucket + role); create two channels on that topic.
2. DeleteChannel one of them, then call UpdateChannel(ChannelArn=<deleting channel>, ClusterArn=<cluster>, S3DestinationUpdate={'DataFreshnessInSeconds': 900}).
3. Assert 400 BadRequestException with Message exactly "Channel cannot be updated in state DELETING. Channel must be in ACTIVE state to be updated." (today: 501 'NotImplemented').
4. Poll DescribeChannel on that channel until it answers NotFoundException (effect absent).
5. Positive control: the identical UpdateChannel on the ACTIVE second channel is accepted; poll until Status == ACTIVE and S3DestinationConfiguration.DataFreshnessInSeconds == 900.

Cause 4 (UpdateReplicationInfo stale currentVersion message):
1. Create a RUNNING replicator; read its CurrentVersion.
2. Fire two UpdateReplicationInfo calls concurrently, both carrying that same CurrentVersion with different ConsumerGroupsToReplicate patterns.
3. Assert exactly one succeeds and the loser is 400 BadRequestException with Message exactly "Invalid parameter currentVersion: The version of the connector isn\u2019t current. Check the current version and try again." — no version ids in the text.
4. DescribeReplicator and assert only the winner's pattern is live (never a merge).

Cause 5 (CreateVpcConnection cross-VPC subnets):
1. Create an ACTIVE SASL/IAM cluster in VPC A with multi-VPC connectivity enabled; find or create a subnet in a different VPC B.
2. CreateVpcConnection(TargetClusterArn=<cluster>, Authentication='SASL_IAM', VpcId=<VPC A>, ClientSubnets=[<subnet in VPC B>], SecurityGroups=[<SG in VPC A>]) and assert 400 BadRequestException (today it returns 200 with a VpcConnectionArn).
3. ListVpcConnections and assert no connection exists for that cluster.
4. Positive control: the same call with ClientSubnets from VPC A succeeds and reaches State AVAILABLE.

Then replay the five check ids in trace_ids.
