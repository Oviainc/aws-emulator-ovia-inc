Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-cluster-parameter-group-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-cluster-parameter-group-lifecycle neptune-parameter-group-family-not-engine-scoped neptune-event-subscription-and-events-by-source neptune-db-parameter-group-copy-and-lifecycle
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|ModifyDBClusterParameterGroup|4xx->2xx|$.ErrorResponse
Issue-Key: neptune|ModifyDBClusterParameterGroup|4xx->2xx|status
Issue-Key: neptune|CreateDBClusterParameterGroup|4xx->2xx|$.ErrorResponse
Issue-Key: neptune|CreateDBClusterParameterGroup|4xx->2xx|status
Issue-Key: neptune|CreateEventSubscription|4xx->2xx|$.ErrorResponse
Issue-Key: neptune|CreateEventSubscription|4xx->2xx|status
Issue-Key: neptune|DescribeDBParameters|2xx->4xx|$.DescribeDBParametersResponse
Issue-Key: neptune|DescribeDBParameters|2xx->4xx|status
Issue-Key: neptune|DeleteDBParameterGroup|2xx->4xx|$.DeleteDBParameterGroupResponse
Issue-Key: neptune|DeleteDBParameterGroup|2xx->4xx|status
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# Neptune parameter-group and event-subscription handlers skip AWS's gates and lose the copied group

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBClusterParameterGroup

## Description
Three Neptune handlers accept a request AWS refuses (answering 200 where the recording shows a 4xx error), and CopyDBParameterGroup does not register the copy it reports, so the copied group is unreachable afterwards. THIS ISSUE HAS 5 CAUSES — all five must be closed before submitting.

Cause 1 of 5: ModifyDBClusterParameterGroup accepts a parameter name the group's family does not define. AWS: Smithy ModifyDBClusterParameterGroup requires DBClusterParameterGroupName and Parameters; the recording refuses an unknown parameter name with 400, Error.Type 'Sender', Error.Code 'InvalidParameterValue' and the message 'Could not find parameter with name: <parameter name>' — fixed text plus the rejected ParameterName from the request. Emulator: 200, the write is accepted and the unknown parameter is stored. Why: a client that mistypes a parameter name is told the change succeeded and silently never gets the setting it asked for. Fix: the ModifyDBClusterParameterGroup handler's parameter validation, which must check each supplied ParameterName against the group's family catalog before applying any of them. Traces: neptune-cluster-parameter-group-lifecycle#task#8, neptune-cluster-parameter-group-lifecycle#task#9, neptune-parameter-group-family-not-engine-scoped#task#5.

Cause 2 of 5: CreateDBClusterParameterGroup accepts a DBParameterGroupFamily that does not exist. AWS: DBParameterGroupFamily is a required member of CreateDBClusterParameterGroupMessage and the recording refuses an unknown family with 400, Error.Type 'Sender', Error.Code 'InvalidParameterValue' and the message 'ParameterGroupFamily <family> is not a valid parameter group family' — fixed text plus the rejected family from the request. Emulator: 200, the group is created against a family the service does not publish. Why: the group is created but has no parameter catalog behind it, so later Describe/Modify calls on it behave inconsistently instead of the request being rejected up front. Fix: the CreateDBClusterParameterGroup handler's family validation, against the same family list DescribeDBEngineVersions publishes. Traces: neptune-parameter-group-family-not-engine-scoped#task#6.

Cause 3 of 5: CreateEventSubscription accepts an SnsTopicArn that does not exist. AWS: SnsTopicArn is required, and Smithy CreateEventSubscription models SNSTopicArnNotFoundFault (httpError 404, awsQueryError code SNSTopicArnNotFound); the recording refuses a non-existent topic with 404, Error.Type 'Sender', Error.Code 'SNSTopicArnNotFound' and the message 'Failed to create Subscription because of Topic <topic arn> Not Found.' — fixed text around the SnsTopicArn from the request. Emulator: 200, the subscription is created pointing at a topic that does not exist. Why: the client believes event delivery is configured when no notifications can ever be delivered. Fix: the CreateEventSubscription handler's topic-existence gate. Traces: neptune-event-subscription-and-events-by-source#task#5.

Cause 4 of 5: the group produced by CopyDBParameterGroup is not readable through DescribeDBParameters. AWS: CopyDBParameterGroup returns the new DBParameterGroup under TargetDBParameterGroupIdentifier, and DescribeDBParameters on that identifier answers 200 with the copied parameters, carrying the source group's user-set values and Source 'user'. Emulator: 404 with no DescribeDBParametersResponse. Why: a copy reported as created cannot be read, so the copy-then-tune workflow is broken. Fix: CopyDBParameterGroup must register the target group (and its copied parameter values) in state, where DescribeDBParameters looks it up. Traces: neptune-db-parameter-group-copy-and-lifecycle#task#6.

Cause 5 of 5: the copied group cannot be deleted either. AWS: DeleteDBParameterGroup on the copy answers 200 with an empty DeleteDBParameterGroupResponse (ResponseMetadata only). Emulator: 404 with no DeleteDBParameterGroupResponse. Why: teardown of a copied parameter group fails, leaking the resource from the caller's point of view. This shares cause 4's root (the copy was never registered) but is a separate step that must be re-replayed. Fix: same registration in CopyDBParameterGroup plus the DeleteDBParameterGroup lookup. Traces: neptune-db-parameter-group-copy-and-lifecycle#teardown#10.

## Evidence
Replay of simparity-wire-aws-v1:
Cause 1 (neptune-cluster-parameter-group-lifecycle#task#8, 3 affected steps): status expected 400, got 200; expected $.ErrorResponse = {Error: {Type 'Sender', Code 'InvalidParameterValue', Message 'Could not find parameter with name: <parameter name>'}}, got None.
Cause 2 (neptune-parameter-group-family-not-engine-scoped#task#6): status expected 400, got 200; expected $.ErrorResponse = {Error: {Type 'Sender', Code 'InvalidParameterValue', Message 'ParameterGroupFamily <family> is not a valid parameter group family'}}, got None.
Cause 3 (neptune-event-subscription-and-events-by-source#task#5): status expected 404, got 200; expected $.ErrorResponse = {Error: {Type 'Sender', Code 'SNSTopicArnNotFound', Message 'Failed to create Subscription because of Topic <topic arn> Not Found.'}}, got None.
Cause 4 (neptune-db-parameter-group-copy-and-lifecycle#task#6): status expected 200, got 404; expected DescribeDBParametersResult.Parameters.Parameter with ParameterValue '20000', Source 'user', ApplyMethod 'pending-reboot', got no response body.
Cause 5 (neptune-db-parameter-group-copy-and-lifecycle#teardown#10): status expected 200, got 404; $.DeleteDBParameterGroupResponse missing.

## Steps to Reproduce
1. Replay neptune-cluster-parameter-group-lifecycle step task#8 — ModifyDBClusterParameterGroup with an undefined parameter name returns 200 instead of 400 InvalidParameterValue.
2. Replay neptune-parameter-group-family-not-engine-scoped step task#6 — CreateDBClusterParameterGroup with a non-existent family returns 200 instead of 400 InvalidParameterValue.
3. Replay neptune-event-subscription-and-events-by-source step task#5 — CreateEventSubscription with a non-existent SNS topic returns 200 instead of 404 SNSTopicArnNotFound.
4. Replay neptune-db-parameter-group-copy-and-lifecycle: CopyDBParameterGroup, then step task#6 DescribeDBParameters on the copy (404) and step teardown#10 DeleteDBParameterGroup on the copy (404).
