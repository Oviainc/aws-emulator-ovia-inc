Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-mutating-updates-rejected-while-creating
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-mutating-updates-rejected-while-creating|run#12|AssertionError|assert messages == expected_messages, f"refusal messages from CREATING state: {messages}"
Issue-Commit: 5ca9b2ecf350619dd6179a85533056a243174605

# MSK cluster-state guard on UpdateBrokerStorage/UpdateMonitoring/UpdateSecurity emits emulator prose, and it pre-empts the CurrentVersion parameter check (2 causes)

> _Note: this issue was identified against the code base at commit `5ca9b2ecf350619dd6179a85533056a243174605`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMonitoring

## Description
TWO distinct root causes on the cluster-mutating refusal path of a PROVISIONED cluster that is still CREATING, both observed by one check. Each must be closed separately; both must be closed before this issue is submitted.

Related open issues name OTHER handlers and carry NEITHER of this check's ids, so they do not cover these causes: 20261104T171616Z-kafka-msk-state-version-conflict-duplicate-topic-and-invalid.md (its Cause 1 fixes the same sentence for UpdateStorage/UpdateRebalancing/UpdateClusterConfiguration/UpdateConnectivity, and its Cause 3 fixes version-vs-state precedence for UpdateBrokerCount only) and 20261031T011306Z-kafka-msk-cluster-mutation-refusals-answer-the-wrong-error.md (malformed-ARN / not-found classes). The three handlers named below are untouched by both.

Cause 1 of 2: the state gate of UpdateBrokerStorage, UpdateMonitoring and UpdateSecurity answers self-composed sentences instead of MSK's one fixed sentence.
AWS: a cluster-mutating call issued while the cluster is CREATING is refused with HTTP 400 BadRequestException (Smithy: UpdateBrokerStorage / UpdateMonitoring / UpdateSecurity all model BadRequestException with httpError 400) and the Message is entirely FIXED text, identical for every mutating operation and with no interpolated value: 'This operation is only valid for resources that are in one of the following states :[ACTIVE]' (note the space before the colon). Neither the cluster's observed state nor its ARN nor the member name appears in it. (Recorded behavior for the exact wording; the model states only the error class.)
Emulator: the status (400) and the code (BadRequestException) are right, but three different prose sentences come back — UpdateBrokerStorage and UpdateMonitoring answer 'The cluster is in the CREATING state; it must be ACTIVE to be updated.' and UpdateSecurity answers 'A cluster can be updated only while it is ACTIVE; this one is CREATING.'. Both interpolate the observed state, which AWS never does.
Why: the Message is part of the contract. A client, log scraper or retry classifier keyed on MSK's state sentence does not match, and because the emulator interpolates the observed state the text also varies with the cluster's phase where AWS's is byte-identical.
Fix: have the UpdateBrokerStorage, UpdateMonitoring and UpdateSecurity handlers raise the kafka service's single shared state-gate message constant (the same one the related issue above introduces for the other mutating handlers) instead of per-handler prose.
Traces: kafka.cluster-mutating-updates-rejected-while-creating.

Cause 2 of 2: a CurrentVersion value that is not a valid cluster version is reported as a state problem, because the state gate runs before the parameter check.
AWS: UpdateMonitoring carrying a CurrentVersion that was never issued (a non-version token) is refused with HTTP 400 BadRequestException and the fixed Message 'One or more of the parameters are not valid.' — the invalid-parameter class wins over the cluster-state class, even when the cluster is also CREATING. The sentence is fixed text; the offending value is not interpolated. (Recorded behavior for the precedence and wording; Smithy: UpdateMonitoring models BadRequestException httpError 400.)
Emulator: the same request answers the state-gate sentence 'The cluster is in the CREATING state; it must be ACTIVE to be updated.', i.e. the cluster-state check short-circuits before CurrentVersion is validated, so an invalid parameter is misreported as a transient state condition.
Why: the two conditions tell a client opposite things. 'invalid parameter' means 'the request is wrong, fix it and do not retry as-is'; the state sentence means 'the request is fine, wait and retry unchanged'. A client driven by the emulator retries a permanently malformed CurrentVersion forever, and the invalid-parameter class is unobservable on this path.
Fix: in the UpdateMonitoring handler's validation, evaluate the CurrentVersion parameter before the cluster-state gate and raise the invalid-parameter message; apply the same ordering wherever the mutating handlers share that gate.
Traces: kafka.cluster-mutating-updates-rejected-while-creating.

## Evidence
Check kafka.cluster-mutating-updates-rejected-while-creating (verdict mismatch; NOT marked ALREADY FILED or OWNED). It creates a PROVISIONED cluster with CreateClusterV2, confirms DescribeClusterV2 reports State=CREATING, and issues four refusable calls with the cluster's CurrentVersion. All four returned BadRequestException, so the earlier code assertion passed; the Message assertion failed:

AssertionError: refusal messages from CREATING state: {'UpdateBrokerStorage': 'The cluster is in the CREATING state; it must be ACTIVE to be updated.', 'UpdateMonitoring': 'The cluster is in the CREATING state; it must be ACTIVE to be updated.', 'UpdateSecurity': 'A cluster can be updated only while it is ACTIVE; this one is CREATING.', 'UpdateMonitoring(staleVersion)': 'The cluster is in the CREATING state; it must be ACTIVE to be updated.'}

Expected (AWS):
  UpdateBrokerStorage  -> ('BadRequestException', 'This operation is only valid for resources that are in one of the following states :[ACTIVE]')
  UpdateMonitoring     -> ('BadRequestException', 'This operation is only valid for resources that are in one of the following states :[ACTIVE]')
  UpdateSecurity       -> ('BadRequestException', 'This operation is only valid for resources that are in one of the following states :[ACTIVE]')
  UpdateMonitoring with CurrentVersion that was never issued -> ('BadRequestException', 'One or more of the parameters are not valid.')

Message anatomy: in all four expected Messages the ENTIRE sentence is fixed text — there is no request- or state-derived part at all. The emulator's two sentences interpolate the observed cluster state ('CREATING'), which is a per-run value and must not appear. The cluster ARN, cluster name and CurrentVersion token used by a particular run are likewise not part of any of these messages.

The calls made: UpdateBrokerStorage(TargetBrokerEBSVolumeInfo=[{KafkaBrokerNodeId:'ALL', VolumeSizeGB:200}]); UpdateMonitoring(EnhancedMonitoring='PER_TOPIC_PER_PARTITION'); UpdateSecurity(EncryptionInfo={EncryptionInTransit:{ClientBroker:'TLS'}}); UpdateMonitoring(CurrentVersion=<a token that is not a cluster version>, EnhancedMonitoring='PER_BROKER').

## Steps to Reproduce
1. Boot the emulator.
2. CreateClusterV2 a PROVISIONED cluster (kafka.m5.large, 2 brokers, 2 subnets in distinct AZs, KafkaVersion 3.9.x) and read its CurrentVersion while DescribeClusterV2 still reports State=CREATING.
3. Cause 1: with that CurrentVersion call, in turn, UpdateBrokerStorage(TargetBrokerEBSVolumeInfo=[{KafkaBrokerNodeId:'ALL',VolumeSizeGB:200}]), UpdateMonitoring(EnhancedMonitoring='PER_TOPIC_PER_PARTITION') and UpdateSecurity(EncryptionInfo={EncryptionInTransit:{ClientBroker:'TLS'}}). Assert each answers HTTP 400 BadRequestException with Message EXACTLY 'This operation is only valid for resources that are in one of the following states :[ACTIVE]', and that the cluster's observed state does not appear anywhere in the Message.
4. Cause 2: on the same CREATING cluster call UpdateMonitoring with a CurrentVersion that was never issued (any non-version token) and EnhancedMonitoring='PER_BROKER'. Assert HTTP 400 BadRequestException with Message EXACTLY 'One or more of the parameters are not valid.' — the parameter class, not the state sentence.
5. Assert none of the four refused calls changed State, CurrentVersion or Provisioned.NumberOfBrokerNodes.
6. Replay kafka.cluster-mutating-updates-rejected-while-creating and confirm it reaches its ACTIVE-cluster positive control.
