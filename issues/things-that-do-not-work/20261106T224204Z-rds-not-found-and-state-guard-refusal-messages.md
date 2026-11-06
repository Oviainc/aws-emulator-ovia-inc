Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-core-lifecycle-and-features
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-core-lifecycle-and-features rds-db-instance-target-error-paths rds-delete-requires-final-snapshot-param rds-global-cluster-delete-with-member rds-integration-lifecycle rds-read-replica-and-instance-promotion
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBInstanceReadReplica|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeleteDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeleteGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: rds|DeleteIntegration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# RDS not-found and state-guard refusal messages diverge from AWS's fixed sentences

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstanceReadReplica

## Description
Five net-new causes, all in the rds service: five refusal/not-found paths answer the right HTTP status but word the Error.Message in the emulator's own prose instead of the sentence RDS returns for that error class. The status and error code match in every one of them; only the message text diverges, and in three of the five the emulator interpolates request/state values into a sentence that AWS states as fixed text with no variable parts at all. Each cause is an independent message template in a different handler and must be fixed and closed separately.

=== Cause 1 of 4+1 (Cause 1 of 5): CreateDBInstanceReadReplica — missing SOURCE instance is reported with the generic instance-lookup sentence ===
Fixture rds-db-instance-core-lifecycle-and-features, step task#8.
AWS: when SourceDBInstanceIdentifier names an instance that does not exist, CreateDBInstanceReadReplica fails with DBInstanceNotFoundFault (Smithy: CreateDBInstanceReadReplica errors include DBInstanceNotFoundFault, errorKind client, httpError 404, awsQueryError code 'DBInstanceNotFound', httpResponseCode 404). The message is recorded behavior only (the model documents no message text): its FIXED text is 'The source instance could not be found: ' and its only VARIABLE part is the SourceDBInstanceIdentifier taken verbatim from the request. AWS distinguishes the SOURCE instance of a read-replica create from an ordinary instance lookup.
Emulator: 404 (status correct) with Error.Message 'DBInstance not found: <sourceDBInstanceIdentifier>' — the generic instance-lookup sentence, reusing the shared DBInstance-not-found template rather than the read-replica source one.
Why: a client or operator cannot tell from the message WHICH identifier in the request was not found. For a read-replica create the request carries both a target DBInstanceIdentifier and a source SourceDBInstanceIdentifier; AWS's sentence says the SOURCE is missing, the emulator's says only that 'DBInstance' is missing, so error-reporting and retry logic that keys on the message attributes the failure to the wrong parameter.
Fix: the CreateDBInstanceReadReplica handler's source-instance resolution step must raise its own DBInstanceNotFoundFault message instead of propagating the shared instance-lookup error template.
Traces: rds-db-instance-core-lifecycle-and-features#task#8 (representative); same cluster also rds-db-instance-target-error-paths#task#7.

=== Cause 2 of 5: DeleteDBCluster — the FinalDBSnapshotIdentifier/SkipFinalSnapshot combination refusal is reworded ===
Fixture rds-delete-requires-final-snapshot-param, step task#2.
AWS: DeleteDBCluster takes SkipFinalSnapshot and FinalDBSnapshotIdentifier (Smithy: DeleteDBClusterMessage members SkipFinalSnapshot: Boolean, FinalDBSnapshotIdentifier: String). When neither is supplied the call is refused with HTTP 400 (recorded status 400; the parameter-combination class of DeleteDBCluster). The message is recorded behavior only and is ENTIRELY fixed text with NO variable parts: 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'. No identifier, value or limit is interpolated.
Emulator: 400 (status correct) with Error.Message 'FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.' — a different sentence (different wording, different word order, and a trailing period AWS's sentence does not carry).
Why: this sentence is the only thing distinguishing the missing-snapshot-parameter refusal from DeleteDBCluster's other 400s (the error class carries no distinguishing code). A client, test or runbook that matches AWS's text to decide whether to re-issue the delete with SkipFinalSnapshot=true does not match, and treats a deterministically correctable request as an unknown failure.
Fix: the DeleteDBCluster input validation's parameter-combination guard — replace the emulator-authored sentence with RDS's fixed text.
Traces: rds-delete-requires-final-snapshot-param#task#2.

=== Cause 3 of 5: DeleteGlobalCluster — the non-empty global-cluster refusal names the identifier and invents a member count and a remedy clause ===
Fixture rds-global-cluster-delete-with-member, step task#8.
AWS: deleting a global cluster that still has member clusters attached fails with InvalidGlobalClusterStateFault (Smithy: DeleteGlobalCluster errors are GlobalClusterNotFoundFault httpError 404 and InvalidGlobalClusterStateFault errorKind client, httpError 400, awsQueryError code 'InvalidGlobalClusterStateFault', httpResponseCode 400). The message is recorded behavior only; its FIXED text is 'Global Cluster ' ... ' is not empty' and its single VARIABLE part is the global cluster's ARN, taken from the emulator's state for the GlobalClusterIdentifier in the request. AWS names the ARN, states no member count, and offers no remedy.
Emulator: 400 (status correct) with Error.Message "Global cluster <globalClusterIdentifier> can't be deleted because it still has 1 member cluster(s): detach them first" — it lowercases 'Cluster', substitutes the bare identifier where AWS puts the ARN, and appends two emulator-invented variable/fixed fragments (a member count and 'detach them first') that are not part of RDS's sentence at all.
Why: the ARN is the part of AWS's message a caller can feed back into other RDS and tagging APIs; the emulator supplies only the short identifier, so message-derived automation loses the account and partition context. The invented count also implies a stable contract (that RDS reports how many members remain) which AWS does not provide, so a client built against the emulator breaks on real RDS.
Fix: the DeleteGlobalCluster handler's non-empty guard — emit the ARN-bearing fixed sentence and drop the count and remedy fragments.
Traces: rds-global-cluster-delete-with-member#task#8.

=== Cause 4 of 5: DeleteIntegration — the integration-not-found message is truncated to its first sentence ===
Fixture rds-integration-lifecycle, step task#4.
AWS: DeleteIntegration on an unknown IntegrationIdentifier fails with IntegrationNotFoundFault (Smithy: DeleteIntegration errors include IntegrationNotFoundFault, errorKind client, httpError 404, awsQueryError code 'IntegrationNotFoundFault', httpResponseCode 404). The message is recorded behavior only; its FIXED text is three sentences — 'Integration with identifier ' <identifier> ' could not be found. The integration might have been deleted, or you provided an incorrect identifier. Check the integration identifier and try again.' — and its only VARIABLE part is the integration identifier/ARN from the request (Smithy: IntegrationIdentifier, length 1..255, pattern ^[a-zA-Z0-9_:\-\/]+$).
Emulator: 404 (status correct) and the first sentence is correct, but Error.Message stops there: 'Integration with identifier <identifier> could not be found.' The second and third sentences of AWS's fixed text are missing.
Why: the two dropped sentences are the actionable half of the message — they tell the caller the identifier may be stale rather than malformed. A client or support tooling that surfaces the full AWS message shows a materially less informative error, and text assertions against AWS's wording fail.
Fix: the DeleteIntegration handler's integration-lookup error construction — carry the complete fixed text, not just the leading sentence.
Traces: rds-integration-lifecycle#task#4.

=== Cause 5 of 5: CreateDBInstanceReadReplica — the source-not-available state guard interpolates the identifier and state into a sentence AWS states as fixed ===
Fixture rds-read-replica-and-instance-promotion, step task#2.
AWS: creating a read replica from an instance that is not yet 'available' fails with InvalidDBInstanceStateFault (Smithy: CreateDBInstanceReadReplica errors include InvalidDBInstanceStateFault, errorKind client, httpError 400, awsQueryError code 'InvalidDBInstanceState', httpResponseCode 400). The message is recorded behavior only and is ENTIRELY fixed text with NO variable parts: 'DB instance is not in the available state'. AWS deliberately names neither the instance nor its current state.
Emulator: 400 (status correct) with Error.Message 'DB instance <dbInstanceIdentifier> is in <currentState> state but must be available to be replicated' — a wholly different sentence that interpolates two values (the source instance identifier from the request and the instance's current lifecycle state from emulator state) into a message whose AWS form is a constant.
Why: the emulator's text is non-constant where AWS's is constant, so any exact-match or equality assertion on RDS's documented sentence fails, and a client that parses the emulator's message for a state name finds no such field on real RDS. It also leaks the in-flight lifecycle state into an error surface AWS does not expose it on.
Fix: the CreateDBInstanceReadReplica handler's source-instance state guard — emit the constant sentence.
Traces: rds-read-replica-and-instance-promotion#task#2.

Related open issues, none of which carries any of these five replay keys: 20261102T104537Z-rds-error-envelopes-return-wrong-code-status-and.md (cluster lookup/filter/create error envelopes), 20261102T074545Z-rds-create-path-refusal-messages-use-emulator-wording.md (create-path refusals) and 20261104T053605Z-rds-10-net-new-causes-refusal-messages-are.md (ten other refusal sentences). They cover different operations and different message templates, so none of them closes the five causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Expected = recorded AWS response, Actual = emulator response.

Cause 1 — rds-db-instance-core-lifecycle-and-features#task#8, CreateDBInstanceReadReplica, expected_status 404 / actual_status 404:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'The source instance could not be found: sim2real-probe-rds-dbinst-missing-c6a7d4cb'
    got      'DBInstance not found: sim2real-probe-rds-dbinst-missing-c6a7d4cb'
  (cluster also covers rds-db-instance-target-error-paths#task#7; the identifier shown is this run's generated name, the requirement is the fixed prefix 'The source instance could not be found: ' plus the request's SourceDBInstanceIdentifier)

Cause 2 — rds-delete-requires-final-snapshot-param#task#2, DeleteDBCluster, expected_status 400 / actual_status 400:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'
    got      'FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.'
  (entirely fixed text on both sides — no variable parts)

Cause 3 — rds-global-cluster-delete-with-member#task#8, DeleteGlobalCluster, expected_status 400 / actual_status 400:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Global Cluster arn:aws:rds::000000000000:global-cluster:rdserr-gm-gc-a7db1482 is not empty'
    got      "Global cluster rdserr-gm-gc-a7db1482 can't be deleted because it still has 1 member cluster(s): detach them first"
  (fixed text 'Global Cluster ' / ' is not empty'; the ARN is the variable part, resolved from state for the requested GlobalClusterIdentifier. The account id and the generated cluster name are this run's values, not requirements)

Cause 4 — rds-integration-lifecycle#task#4, DeleteIntegration, expected_status 404 / actual_status 404:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Integration with identifier <arn> could not be found. The integration might have been deleted, or you provided an incorrect identifier. Check the integration identifier and try again.'
    got      'Integration with identifier <arn> could not be found.'
  (the two trailing sentences are fixed text and are absent; the ARN is the request's IntegrationIdentifier)

Cause 5 — rds-read-replica-and-instance-promotion#task#2, CreateDBInstanceReadReplica, expected_status 400 / actual_status 400:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'DB instance is not in the available state'
    got      'DB instance sim2real-probe-rds-rrsorc-d6a36b5d is in creating state but must be available to be replicated'
  (AWS's sentence is a constant; the emulator interpolates the request identifier and the state from its own store)

Model evidence cited above comes from the on-disk Smithy model at services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json (operations CreateDBInstanceReadReplica, DeleteDBCluster, DeleteGlobalCluster, DeleteIntegration).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) and compare each step below.

Cause 1: replay fixture rds-db-instance-core-lifecycle-and-features step task#8 — CreateDBInstanceReadReplica with a SourceDBInstanceIdentifier that does not exist. Observe 404 with Error.Message 'DBInstance not found: <id>' instead of 'The source instance could not be found: <id>'. Same cluster: rds-db-instance-target-error-paths#task#7.

Cause 2: replay fixture rds-delete-requires-final-snapshot-param step task#2 — DeleteDBCluster on an existing cluster with neither SkipFinalSnapshot nor FinalDBSnapshotIdentifier. Observe 400 with 'FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.' instead of 'Please specify FinalDBSnapshotIdentifier or SkipFinalSnapshot'.

Cause 3: replay fixture rds-global-cluster-delete-with-member step task#8 — create a global cluster, attach a member DBCluster, then DeleteGlobalCluster. Observe 400 with the count/remedy sentence naming the bare identifier instead of 'Global Cluster <globalClusterArn> is not empty'.

Cause 4: replay fixture rds-integration-lifecycle step task#4 — DeleteIntegration with an IntegrationIdentifier that does not exist. Observe 404 whose message stops after 'could not be found.' and omits AWS's two following sentences.

Cause 5: replay fixture rds-read-replica-and-instance-promotion step task#2 — CreateDBInstanceReadReplica against a source instance still in 'creating'. Observe 400 with the interpolated identifier/state sentence instead of the constant 'DB instance is not in the available state'.

After fixing, re-replay all five fixture#step bindings (the representative plus the four in trace_ids); each must match the recorded Error.Message exactly, with the variable parts resolved from the request and from emulator state.
