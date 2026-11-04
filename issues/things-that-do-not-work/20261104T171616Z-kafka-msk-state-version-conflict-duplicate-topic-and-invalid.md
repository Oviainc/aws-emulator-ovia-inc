Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-replicator-updates-blocked-while-creating-rejected kafka.cluster-version-config-updates-rejected-while-creating kafka.list-nodes-reflects-cluster-brokers kafka.update-broker-count-concurrent-version-conflict kafka.create-topic-duplicate-rejected kafka.topic-create-duplicate-exists kafka.update-replication-info-invalid-topic-pattern-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-replicator-updates-blocked-while-creating-rejected|run#9|AssertionError|assert storage_message == (
Issue-Key: kafka.cluster-version-config-updates-rejected-while-creating|run#7|AssertionError|assert not bad, f"expected {_STATE_REFUSAL!r} from CREATING state, got {bad}"
Issue-Key: kafka.list-nodes-reflects-cluster-brokers|run#8|AssertionError|assert err_message == "You can't list the nodes for a cluster that is in the CREATING state.", (
Issue-Key: kafka.update-broker-count-concurrent-version-conflict|run#13|AssertionError|assert messages == ["The version of the cluster isn\u2019t current. Check the current version and try again."], (
Issue-Key: kafka.create-topic-duplicate-rejected|run#17|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: kafka.topic-create-duplicate-exists|run#10|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: kafka.update-replication-info-invalid-topic-pattern-rejected|run#10|AssertionError|assert code == expected, f"unterminated character class: expected {expected!r}, got {code!r}"
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# MSK state, version-conflict, duplicate-topic and invalid-regex refusals carry emulator prose instead of AWS's fixed messages, and a stale CurrentVersion is reported as a state problem (5 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStorage

## Description
FIVE distinct root causes on MSK refusal paths: four message-text divergences in different handlers and one wrong refusal decision. Each must be closed separately; all five must be closed before this issue is submitted. Related open issues name other operations and carry none of these check ids: 20261031T011306Z-kafka-msk-cluster-mutation-refusals-answer-the-wrong-error.md (UpdateBrokerCount malformed ARN, UpdateConnectivity not-found) and 20261029T202809Z-kafka-msk-not-found-already-exists-and-tagging-refusals.md.

Cause 1 of 5: the 'cluster is not in a mutable state' refusal uses the emulator's own sentences.
AWS: a cluster-mutating operation issued while the cluster is CREATING is refused with BadRequestException (400) and the message 'This operation is only valid for resources that are in one of the following states :[ACTIVE]' — entirely FIXED text (including the space before the colon); neither the cluster's current state nor its ARN is interpolated. The same sentence is MSK's for UpdateStorage, UpdateRebalancing, UpdateClusterConfiguration and UpdateConnectivity.
Emulator: UpdateStorage and UpdateClusterConfiguration answer 'The cluster is in the CREATING state; it must be ACTIVE to be updated.' and UpdateConnectivity answers 'A cluster can be updated only while it is ACTIVE; this one is CREATING.' — three self-composed sentences, two of which interpolate the observed state, where AWS emits one constant string. Status (400) and code (BadRequestException) are right.
Why: the message is part of the contract; clients, log scrapers and retry logic keyed on MSK's state sentence do not match, and because the emulator interpolates the state the text also varies run to run where AWS's is byte-identical.
Fix: one shared state-gate message constant in the kafka service's state module, raised by every cluster-mutating handler instead of per-handler prose.
Traces: kafka.cluster-replicator-updates-blocked-while-creating-rejected (UpdateStorage/UpdateRebalancing), kafka.cluster-version-config-updates-rejected-while-creating (UpdateClusterConfiguration/UpdateConnectivity).

Cause 2 of 5: ListNodes' CREATING refusal wording is wrong.
AWS: ListNodes on a cluster that is still CREATING is refused with BadRequestException and the fixed message "You can't list the nodes for a cluster that is in the CREATING state."
Emulator: "You can't list nodes for a cluster in CREATING state." — close but not AWS's sentence (missing 'the nodes for a cluster that is in the').
Why: same contract problem as Cause 1 — an assertion or classifier on MSK's sentence fails even though the status and code are right.
Fix: the ListNodes handler's state-refusal message constant.
Traces: kafka.list-nodes-reflects-cluster-brokers.

Cause 3 of 5: a stale CurrentVersion on UpdateBrokerCount is reported as a cluster-state problem instead of a version conflict.
AWS: two concurrent UpdateBrokerCount calls that share one CurrentVersion admit exactly ONE scale: the winner returns a ClusterOperationArn that DescribeClusterOperation reads back and the cluster's CurrentVersion advances, and the loser is refused with BadRequestException and the fixed message 'The version of the cluster isn\u2019t current. Check the current version and try again.' (no version value appears in the sentence).
Emulator: the loser is refused with 'The cluster is in the UPDATING state; it must be ACTIVE to be updated.' — the state gate pre-empts the CurrentVersion check, so a version conflict is misreported as a transient state problem.
Why: the two conditions mean opposite things to a client: a version conflict says 're-read the cluster and retry with the new version', a state refusal says 'wait and retry the same request'. A compare-and-set scaling loop therefore retries a stale version forever against the emulator, and optimistic-locking tests never see the conflict class.
Fix: evaluate the CurrentVersion precondition before the cluster-state gate in UpdateBrokerCount's handler, and raise the version-not-current message.
Traces: kafka.update-broker-count-concurrent-version-conflict.

Cause 4 of 5: the duplicate-topic refusal message is emulator prose.
AWS: CreateTopic for a topic the cluster already holds is refused with TopicExistsException and the fixed message 'Topic already exists.' — no cluster ARN or topic name is interpolated.
Emulator: the right code (TopicExistsException) with "The cluster <cluster ARN> already has a topic named '<topic name>'." — a self-composed sentence that leaks both the cluster ARN and the topic name into a field AWS keeps constant.
Why: idempotent-create logic that matches MSK's sentence stops matching; and because the emulator interpolates per-run values the message can never be asserted stably.
Fix: the duplicate-topic refusal in the CreateTopic handler must raise the service's fixed message constant.
Traces: kafka.create-topic-duplicate-rejected, kafka.topic-create-duplicate-exists.

Cause 5 of 5: the invalid topic-pattern refusal message is emulator prose.
AWS: UpdateReplicationInfo with a TopicReplication pattern that is not a valid regular expression is refused with BadRequestException and the message 'The regex "<the pattern from the request>" is invalid.' — fixed text with the offending pattern quoted in it.
Emulator: BadRequestException with "'<pattern>' is not a valid regular expression." — the code matches, the sentence does not.
Why: same contract problem; a client that surfaces or matches MSK's wording shows text AWS never emits.
Fix: the regex-validation refusal message in the UpdateReplicationInfo handler.
Traces: kafka.update-replication-info-invalid-topic-pattern-rejected.

## Evidence
All seven checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

Cause 1 — kafka.cluster-replicator-updates-blocked-while-creating-rejected: UpdateStorage(VolumeSizeGB=11) on a CREATING cluster.
  Expected: ('BadRequestException', 'This operation is only valid for resources that are in one of the following states :[ACTIVE]'); same for UpdateRebalancing.
  Actual message: 'The cluster is in the CREATING state; it must be ACTIVE to be updated.'
  kafka.cluster-version-config-updates-rejected-while-creating: expected the same sentence from UpdateClusterConfiguration and UpdateConnectivity; got {'UpdateClusterConfiguration': ('BadRequestException', 'The cluster is in the CREATING state; it must be ACTIVE to be updated.'), 'UpdateConnectivity': ('BadRequestException', 'A cluster can be updated only while it is ACTIVE; this one is CREATING.')}.

Cause 2 — kafka.list-nodes-reflects-cluster-brokers: expected "You can't list the nodes for a cluster that is in the CREATING state."; got "You can't list nodes for a cluster in CREATING state."

Cause 3 — kafka.update-broker-count-concurrent-version-conflict: two concurrent UpdateBrokerCount calls (targets 4 and 6) sharing the ACTIVE cluster's CurrentVersion.
  Expected loser: ['The version of the cluster isn\u2019t current. Check the current version and try again.']
  Actual loser: ['The cluster is in the UPDATING state; it must be ACTIVE to be updated.']

Cause 4 — kafka.topic-create-duplicate-exists: expected ('TopicExistsException', 'Topic already exists.'); got ('TopicExistsException', "The cluster arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-behav-topic-dup-cluster-<id>/<uuid>-1 already has a topic named 'sim2real-behav-topic-dup-<id>'."). kafka.create-topic-duplicate-rejected shows the identical divergence. The ARN/topic name are per-run values that must not appear in the message at all.

Cause 5 — kafka.update-replication-info-invalid-topic-pattern-rejected: pattern '['.
  Expected: ('BadRequestException', 'The regex "[" is invalid.')
  Actual: ('BadRequestException', "'[' is not a valid regular expression.")

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: CreateClusterV2 a PROVISIONED cluster and, while DescribeClusterV2 reports CREATING, call UpdateStorage(VolumeSizeGB=current+11) and UpdateRebalancing with the cluster's CurrentVersion; assert both answer 400 BadRequestException with Message exactly 'This operation is only valid for resources that are in one of the following states :[ACTIVE]'. Repeat with UpdateClusterConfiguration and UpdateConnectivity. Assert the cluster's CurrentVersion is unchanged by the refused calls.
3. Cause 2: while the same cluster is CREATING, call ListNodes and assert the Message is exactly "You can't list the nodes for a cluster that is in the CREATING state."
4. Cause 3: wait for ACTIVE, then from two threads issue UpdateBrokerCount with the SAME CurrentVersion and different targets. Assert exactly one succeeds with a ClusterOperationArn readable via DescribeClusterOperation, the cluster's CurrentVersion advances, and the loser's Message is exactly 'The version of the cluster isn\u2019t current. Check the current version and try again.'
5. Cause 4: CreateTopic a topic, wait for ACTIVE, then CreateTopic the same name again; assert TopicExistsException with Message exactly 'Topic already exists.' and that neither the cluster ARN nor the topic name appears in the message.
6. Cause 5: UpdateReplicationInfo with TopicsToReplicate=['['] and assert BadRequestException with Message exactly 'The regex "[" is invalid.'
7. Replay kafka.cluster-replicator-updates-blocked-while-creating-rejected, kafka.cluster-version-config-updates-rejected-while-creating, kafka.list-nodes-reflects-cluster-brokers, kafka.update-broker-count-concurrent-version-conflict, kafka.create-topic-duplicate-rejected, kafka.topic-create-duplicate-exists and kafka.update-replication-info-invalid-topic-pattern-rejected.
