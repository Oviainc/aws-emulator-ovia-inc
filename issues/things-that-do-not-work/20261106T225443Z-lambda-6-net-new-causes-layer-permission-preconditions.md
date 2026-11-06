Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.layer-permission-revisionid-precondition lambda.remove-layer-permission-precondition-failed lambda.docdb-esm-without-database-name-rejected-before-any-access-check lambda.msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up lambda.mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up lambda.weighted-alias-across-execution-roles-rejected lambda.weighted-alias-on-latest-primary-rejected lambda.weighted-alias-with-two-additional-versions-rejected lambda.snapstart-on-a-runtime-without-snapstart-rejected lambda.provisioned-concurrency-on-snapstart-version-rejected lambda.snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.layer-permission-revisionid-precondition|run#13|AssertionError|assert stale_add_msg == STALE_REVISION_MESSAGE, f"unexpected message: {stale_add_msg!r}"
Issue-Key: lambda.remove-layer-permission-precondition-failed|run#14|AssertionError|assert code_msg == (
Issue-Key: lambda.docdb-esm-without-database-name-rejected-before-any-access-check|run#11|AssertionError|assert got == _NO_DATABASE, f"no DatabaseName, no role access: refused with {got!r}"
Issue-Key: lambda.msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up|run#11|AssertionError|assert got == role_refusal, f"a role without kafka:DescribeClusterV2 was refused with {got!r}"
Issue-Key: lambda.mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up|run#10|AssertionError|assert got == _ROLE_REFUSAL, f"a role without mq:DescribeBroker was refused with {got!r}"
Issue-Key: lambda.weighted-alias-across-execution-roles-rejected|run#18|AssertionError|assert (code, status, message) == ("InvalidParameterValueException", 400, expected), (
Issue-Key: lambda.weighted-alias-on-latest-primary-rejected|run#10|AssertionError|assert (code, status, message) == ("InvalidParameterValueException", 400, expected), (
Issue-Key: lambda.weighted-alias-with-two-additional-versions-rejected|run#19|AssertionError|assert {r[0] for r in runs} == {v1, v2}, (
Issue-Key: lambda.snapstart-on-a-runtime-without-snapstart-rejected|run#5|AssertionError|assert (code, msg) == ("InvalidParameterValueException",
Issue-Key: lambda.provisioned-concurrency-on-snapstart-version-rejected|run#10|AssertionError|assert msg == (
Issue-Key: lambda.snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version|run#6|AssertionError|assert (conf["State"], conf.get("StateReasonCode"), conf.get("StateReason")) == (
Issue-Commit: 2d5907cd7e831b17c8c4fd60b3b325267ed3fe4d

# lambda: 6 net-new causes — layer-permission preconditions, event-source-mapping refusals, weighted aliases and SnapStart

> _Note: this issue was identified against the code base at commit `2d5907cd7e831b17c8c4fd60b3b325267ed3fe4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
Six further net-new root causes on the Lambda control plane, grounded in 11 failing behavioral checks that no open issue carries. Each cause is numbered and carries its own failing check ids.

Cause 1 of 6: the layer-version permission RevisionId precondition names the wrong API and subject.
AWS: AddLayerVersionPermission and RemoveLayerVersionPermission with a stale RevisionId are refused with PreconditionFailedException (modeled httpError 412) and the fixed message "The Revision Id provided does not match the latest Revision Id. Call the GetLayerPolicy API to retrieve the latest Revision Id" (no variable parts). Smithy: both operations take a RevisionId and declare PreconditionFailedException (412).
Emulator: returns "The Revision Id provided does not match the latest Revision Id for the layer version. Call the GetLayerVersion API to retrieve the latest Revision Id" on both operations — it inserts "for the layer version" and points the caller at GetLayerVersion instead of GetLayerPolicy.
Why: the message is the recovery instruction; a client that follows it calls the wrong API and never obtains the policy revision it must echo, so the optimistic-concurrency retry loop cannot converge.
Fix: the PreconditionFailedException message in the AddLayerVersionPermission and RemoveLayerVersionPermission handlers of the lambda service crate.
Traces: lambda.layer-permission-revisionid-precondition, lambda.remove-layer-permission-precondition-failed.

Cause 2 of 6: CreateEventSourceMapping's pre-flight refusals are re-worded for DocumentDB, MSK and Amazon MQ sources.
AWS: (a) a DocumentDB mapping without DocumentDBEventSourceConfig.DatabaseName is refused with InvalidParameterValueException "Invalid parameters: documentDBEventSourceConfig, databaseName"; (b) an MSK mapping whose execution role cannot describe the cluster is refused with InvalidParameterValueException "Cannot access cluster <cluster ARN>. Please ensure the role can perform the DescribeClusterV2 (recommended) or DescribeCluster and GetBootstrapBrokers actions on your cluster in IAM." (variable part: the EventSourceArn from the request); (c) an Amazon MQ mapping whose role cannot describe the broker is refused with InvalidParameterValueException "Cannot access broker eventSourceArn. Please ensure the role can perform the DescribeBroker action on your broker in IAM." Smithy: CreateEventSourceMapping declares InvalidParameterValueException (httpError 400) and DocumentDBEventSourceConfig/AmazonManagedKafkaEventSourceConfig members.
Emulator: refuses with the right code but its own prose — (a) "1 validation error detected: Value null at 'documentDBEventSourceConfig.databaseName' failed to satisfy constraint: Member must not be null.", (b) "The function execution role does not have permissions to call DescribeClusterV2 on MSK", (c) "The function execution role does not have permissions to call DescribeBroker on MQ".
Why: the messages are the only place AWS tells the caller which IAM action to grant and on which resource; clients and runbooks that match AWS's text (or extract the cluster ARN from it) do not recognize the emulator's rewrite, and the DocumentDB case is reported as a generic null-constraint violation instead of the documented "Invalid parameters" form.
Fix: the pre-flight validation and refusal messages in the CreateEventSourceMapping handler of the lambda service crate (the DocumentDB parameter check and the source-access permission checks).
Traces: lambda.docdb-esm-without-database-name-rejected-before-any-access-check, lambda.msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up, lambda.mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up.

Cause 3 of 6: the weighted-alias refusals are re-worded.
AWS: CreateAlias with RoutingConfig.AdditionalVersionWeights is refused with HTTP 400 InvalidParameterValueException and the fixed messages "Specified versions must be associated with the same execution role" when the primary and additional versions have different execution roles, and "$LATEST is not supported for an alias pointing to more than 1 version" when FunctionVersion is $LATEST. Smithy: CreateAlias.RoutingConfig is AliasRoutingConfiguration and the operation declares InvalidParameterValueException (httpError 400).
Emulator: returns the right code and status with invented text: "Weighted alias cannot be mapped to versions with different execution roles" and "Weighted alias cannot be mapped to $LATEST".
Why: deployment tooling keys on these messages to distinguish "split the traffic differently" from "publish a version first"; a reworded refusal breaks that branch even though the status code matches.
Fix: the routing-configuration validation messages in the CreateAlias handler of the lambda service crate.
Traces: lambda.weighted-alias-across-execution-roles-rejected, lambda.weighted-alias-on-latest-primary-rejected.

Cause 4 of 6: a surviving weighted alias never routes to its additional version.
AWS: an alias whose RoutingConfig splits 50/50 between two published versions routes invocations to both; over 30 invocations through the alias both version numbers appear (each invocation reports its own version).
Emulator: every one of the 30 invocations lands on the primary version — the observed set is ['1'] and the additional version is never reached, so the weight is recorded but not honoured at invoke time.
Why: callers use weighted aliases for canaries and blue/green rollouts; with all traffic pinned to the primary, a canary silently never runs and a rollout that "succeeds" has exercised nothing.
Note: the open issue 20261101T210635Z-lambda-weighted-alias-traffic-is-never-observable-across....md touches weighted-alias observability but does not carry this check.
Fix: alias resolution at invoke time in the lambda data plane (the version-selection step that must sample AdditionalVersionWeights).
Traces: lambda.weighted-alias-with-two-additional-versions-rejected.

Cause 5 of 6: the SnapStart refusals are re-worded.
AWS: UpdateFunctionConfiguration setting SnapStart.ApplyOn=PublishedVersions on a runtime without SnapStart support is refused with InvalidParameterValueException "<runtime> is not supported for SnapStart enabled functions." (variable part: the runtime identifier from the function, e.g. nodejs22.x), and SnapStart stays ApplyOn=None. PutProvisionedConcurrencyConfig on a SnapStart-enabled version is refused with ResourceConflictException "The operation cannot be performed at this time. Provisioned Concurrency is not supported for SnapStart enabled functions" (fixed text). Smithy: PutProvisionedConcurrencyConfig declares ResourceConflictException (httpError 409) and InvalidParameterValueException (400).
Emulator: refuses with the correct codes but its own wording — "SnapStart is not supported for the runtime nodejs22.x" and "Provisioned concurrency cannot be configured on a function version with SnapStart enabled".
Why: clients that match the documented text to decide whether to fall back (drop SnapStart, or drop provisioned concurrency) do not recognize these messages.
Fix: the SnapStart compatibility checks and their messages in the UpdateFunctionConfiguration and PutProvisionedConcurrencyConfig handlers of the lambda service crate.
Traces: lambda.snapstart-on-a-runtime-without-snapstart-rejected, lambda.provisioned-concurrency-on-snapstart-version-rejected.

Cause 6 of 6: a SnapStart before-snapshot hook that raises does not fail the published version.
AWS: when SnapStart is enabled and the runtime's before-snapshot (CRaC/beforeCheckpoint) hook raises, the published version does not become usable: it settles in State Failed with a StateReasonCode and a StateReason naming the initialization failure, so the version cannot be invoked.
Emulator: the version settles State 'Active' with StateReasonCode None and StateReason None — the hook's failure is not observed at all (recorded behavior only; the emulator reports a healthy version).
Why: SnapStart's whole contract is that snapshot-time initialization is validated once at publish; a version that claims Active after a failed hook moves the failure to every later invocation and hides it from deployment gates that poll State.
Fix: SnapStart snapshot/restore execution in the lambda runtime and the version state machine that publishes it (the step that must mark the version Failed when the before-snapshot hook raises).
Traces: lambda.snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version.

## Evidence
All 11 checks are replayed failures under the suite's `### lambda` heading, none marked ALREADY FILED or OWNED.

Cause 1 — layer-permission-revisionid-precondition / remove-layer-permission-precondition-failed: expected "The Revision Id provided does not match the latest Revision Id. Call the GetLayerPolicy API to retrieve the latest Revision Id"; actual (both) "The Revision Id provided does not match the latest Revision Id for the layer version. Call the GetLayerVersion API to retrieve the latest Revision Id".
Cause 2 — docdb-esm...: expected ('InvalidParameterValueException', 'Invalid parameters: documentDBEventSourceConfig, databaseName'); actual ('InvalidParameterValueException', "1 validation error detected: Value null at 'documentDBEventSourceConfig.databaseName' failed to satisfy constraint: Member must not be null."). msk-esm...: expected ('InvalidParameterValueException', 'Cannot access cluster <cluster>. Please ensure the role can perform the DescribeClusterV2 (recommended) or DescribeCluster and GetBootstrapBrokers actions on your cluster in IAM.'); actual ('InvalidParameterValueException', 'The function execution role does not have permissions to call DescribeClusterV2 on MSK'). mq-esm...: expected ('InvalidParameterValueException', 'Cannot access broker eventSourceArn. Please ensure the role can perform the DescribeBroker action on your broker in IAM.'); actual ('InvalidParameterValueException', 'The function execution role does not have permissions to call DescribeBroker on MQ').
Cause 3 — weighted-alias-across-execution-roles-rejected: expected ('InvalidParameterValueException', 400, 'Specified versions must be associated with the same execution role'); actual ('InvalidParameterValueException', 400, 'Weighted alias cannot be mapped to versions with different execution roles'). weighted-alias-on-latest-primary-rejected: expected ('InvalidParameterValueException', 400, '$LATEST is not supported for an alias pointing to more than 1 version'); actual ('InvalidParameterValueException', 400, 'Weighted alias cannot be mapped to $LATEST').
Cause 4 — weighted-alias-with-two-additional-versions-rejected: AssertionError "the surviving 50/50 split must still reach both 1 and 2 over 30 invokes, got ['1']".
Cause 5 — snapstart-on-a-runtime-without-snapstart-rejected: expected ('InvalidParameterValueException', 'nodejs22.x is not supported for SnapStart enabled functions.'); actual ('InvalidParameterValueException', 'SnapStart is not supported for the runtime nodejs22.x'). provisioned-concurrency-on-snapstart-version-rejected: expected ResourceConflictException with 'The operation cannot be performed at this time. Provisioned Concurrency is not supported for SnapStart enabled functions'; actual message 'Provisioned concurrency cannot be configured on a function version with SnapStart enabled'.
Cause 6 — snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version: AssertionError "version 1 with a raising hook settled 'Active' None: None" (expected State Failed with a StateReasonCode/StateReason).

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, then re-run these checks under the `lambda` heading: layer-permission-revisionid-precondition, remove-layer-permission-precondition-failed, docdb-esm-without-database-name-rejected-before-any-access-check, msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up, mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up, weighted-alias-across-execution-roles-rejected, weighted-alias-on-latest-primary-rejected, weighted-alias-with-two-additional-versions-rejected, snapstart-on-a-runtime-without-snapstart-rejected, provisioned-concurrency-on-snapstart-version-rejected, snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version.

By hand: (1) PublishLayerVersion, AddLayerVersionPermission, then AddLayerVersionPermission/RemoveLayerVersionPermission again with the first RevisionId — compare the 412 message; (2) CreateEventSourceMapping for a DocumentDB source with no DatabaseName, for an MSK cluster with a role lacking kafka:DescribeClusterV2, and for an MQ broker with a role lacking mq:DescribeBroker; (3) CreateAlias with AdditionalVersionWeights across two versions with different roles, and with FunctionVersion=$LATEST plus a weight; (4) publish two versions, create an alias with a 0.5 weight on the second, invoke the alias 30 times and collect the reported versions; (5) UpdateFunctionConfiguration SnapStart=PublishedVersions on a nodejs22.x function, and PutProvisionedConcurrencyConfig on a SnapStart version; (6) publish a Python SnapStart version whose before-snapshot hook raises and poll GetFunction Configuration.State.
