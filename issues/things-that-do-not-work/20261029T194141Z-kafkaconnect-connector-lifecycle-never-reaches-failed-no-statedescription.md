Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafkaconnect.connector-create-lifecycle-reaches-failed-state kafkaconnect.update-connector-stale-version-rejected kafkaconnect.restart-connector-state-conflict-rejected kafkaconnect.delete-custom-plugin-in-use-by-connector-rejected kafkaconnect.worker-configuration-delete-removes-resource kafkaconnect.kafka-connect-native-data-plane-vpc-only-skip
Issue-Shape-Id: com.amazonaws.kafkaconnect#KafkaConnect
Issue-Key: kafkaconnect.connector-create-lifecycle-reaches-failed-state|run#9|AssertionError|assert final_state == "FAILED", final_state
Issue-Key: kafkaconnect.update-connector-stale-version-rejected|run#8|AssertionError|assert resting_state == "FAILED", resting_state
Issue-Key: kafkaconnect.restart-connector-state-conflict-rejected|run#11|AssertionError|assert message == state_gate_message, message
Issue-Key: kafkaconnect.delete-custom-plugin-in-use-by-connector-rejected|run#10|AssertionError|assert message == (
Issue-Key: kafkaconnect.worker-configuration-delete-removes-resource|run#12|AssertionError|assert tag_message == "Resource was not found", tag_message
Issue-Key: kafkaconnect.kafka-connect-native-data-plane-vpc-only-skip|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# kafkaconnect: connector lifecycle never reaches FAILED (no stateDescription), three refusal messages diverge from AWS wording, and logDelivery emits no worker log events (5 causes)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Five independent root causes in the kafkaconnect emulator, all grounded in failing behavioral checks of this service. They are numbered Cause 1 of 5 .. Cause 5 of 5; each must be closed before this issue is submitted, and each names the failing check ids a fixer should re-replay.

NOTE on neighbouring issues: the open issue 20261028T173425Z-kafkaconnect-restartconnector-wrong-behavior--r2.md (connector-operation ARN mis-derivation) and the fixed issues 20261028T114947Z-kafkaconnect-updateconnector-maintainability.md (de-duplicate the "connector is not RUNNING" refusal) and 20261029T043059Z-kafkaconnect-deletecustomplugin-wrong-behavior.md (keep an in-use plugin) touch the same code paths but none of them covers the divergences below: the first two are about where the shared refusal LIVES, not what it SAYS, and the third established that the in-use delete is refused at all, which is correct — only its message is wrong.

=== Cause 1 of 5: a connector always settles into RUNNING; it can never reach FAILED and never reports a stateDescription ===
Failing checks: kafkaconnect.connector-create-lifecycle-reaches-failed-state, kafkaconnect.update-connector-stale-version-rejected

AWS: ConnectorState is modelled with the values RUNNING | CREATING | UPDATING | DELETING | FAILED | RESTARTING (Smithy: com.amazonaws.kafkaconnect#ConnectorState). DescribeConnector's output carries both connectorState (ConnectorState) and stateDescription (StateDescription{code,message} — "Details about the state of a resource"). Recorded behavior: a connector created against a Kafka cluster whose bootstrap servers do not resolve, with a connector.class the plugin does not provide, leaves CREATING and comes to rest in FAILED, and DescribeConnector then answers a stateDescription whose message is non-empty. FAILED is a resting state: it is not transient and the connector stays there.

Emulator: every connector leaves CREATING for RUNNING unconditionally after the chosen TRANSITION_SPAN, whatever its kafkaCluster, plugins or serviceExecutionRoleArn name, and stateDescription is never populated. Both checks poll DescribeConnector until connectorState != "CREATING" and then assert the resting state is FAILED; both got "RUNNING" (AssertionError: RUNNING).

Why: a client cannot observe a failed connector at all against this emulator. Every provisioning-failure path a real caller must handle — detect FAILED, read stateDescription.message to find out why, then delete and recreate — is unreachable, and code written against it will read a broken connector as healthy. It also makes FAILED-state gating untestable: the two checks above, plus the control halves of the restart and update checks, depend on a connector resting in FAILED.

Fix: the connector lifecycle in the service state — the settle step that drives a pending Settle change (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs, ComAmazonawsKafkaconnectKafkaconnectState::settle_due, which unconditionally assigns CONNECTOR_RUNNING) and the CreateConnector handler that decides which resting state the clock owes. The state record needs a FAILED resting state plus a stored StateDescription, and the create path needs a deterministic rule for which connectors settle FAILED rather than RUNNING (a kafkaCluster whose bootstrap servers cannot be resolved is the condition the recordings use).

=== Cause 2 of 5: the state-gate refusal of RestartConnector/UpdateConnector carries the emulator's own sentence instead of AWS's "only valid for resources that are in one of the following states" message ===
Failing checks: kafkaconnect.restart-connector-state-conflict-rejected (also asserted by kafkaconnect.update-connector-stale-version-rejected's control half)

AWS: RestartConnector and UpdateConnector both model BadRequestException (HTTP 400). Recorded behavior: a connector that is not in one of the states the operation accepts is refused 400 BadRequestException with the message
  "Invalid parameter connectorArn: This operation is only valid for resources that are in one of the following states: [RUNNING, PAUSED]"
Fixed text: "Invalid parameter ", ": This operation is only valid for resources that are in one of the following states: [RUNNING, PAUSED]". Variable parts: the member name (connectorArn, from the request member that carried the ARN) and the bracketed state list (the states THIS operation accepts — the message names RUNNING and PAUSED, note that it names PAUSED even though PAUSED is not a ConnectorState value). No request value, id or timestamp appears in it.

Emulator: answers 400 BadRequestException — the status and code are right — with
  "Invalid parameter connectorArn: The connector is not RUNNING."
The check asserted the message for a connector still in CREATING and failed with "AssertionError: Invalid parameter connectorArn: The connector is not RUNNING."

Why: a client that classifies a retryable "wrong state, wait and retry" refusal by matching the message text (the only discriminator AWS gives it — BadRequestException covers every bad input for these operations) does not recognise the emulator's sentence, and it reports to its user a reason AWS never produces. It also hides the acceptable-state SET: AWS tells the caller which states would work, the emulator only says the connector is not RUNNING.

Fix: the shared state-gate refusal in the service state — connector_is_not_running() in services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs (around line 2101) and ConnectorRecord::not_running_refusal (around line 934), which both RestartConnector and UpdateConnector call. The reason sentence must render the accepted-state list of the operation rather than a single state name.

=== Cause 3 of 5: DeleteCustomPlugin's in-use refusal uses the parameter-naming form instead of AWS's "You can't delete a custom plugin being used by one or more connectors" message ===
Failing check: kafkaconnect.delete-custom-plugin-in-use-by-connector-rejected

AWS: DeleteCustomPlugin models BadRequestException (HTTP 400) and no conflict channel. Recorded behavior: deleting a custom plugin a connector still references is refused 400 BadRequestException with the message
  "You can't delete a custom plugin being used by one or more connectors. This custom plugin is used by the connector with arn <connectorArn>, and possibly others. Please delete connectors using this custom plugin and try again."
Fixed text: everything outside the ARN — "You can't delete a custom plugin being used by one or more connectors. This custom plugin is used by the connector with arn ", ", and possibly others. Please delete connectors using this custom plugin and try again.". The ONE variable part is <connectorArn>: the ARN of a connector in the emulator's own state that references the plugin (the recording's literal ARN fills the template, it is not part of it). Note that this message does NOT use this service's "Invalid parameter <member>: ..." form and does not name a request member at all.

Emulator: answers 400 BadRequestException with
  "Invalid parameter customPluginArn: The custom plugin is in use by a connector."
The check asserted the message and failed with "AssertionError: Invalid parameter customPluginArn: The custom plugin is in use by a connector."

Why: the AWS message tells the caller WHICH connector is blocking the delete, which is the whole value of the refusal for a cleanup script: it can delete that connector and retry. The emulator's message carries no ARN, so such a script has no way forward, and text-matching clients mis-classify the refusal.

Fix: the in-use refusal in the service state — resource_is_in_use() (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs, around line 2094) and dependency_in_use_refusal (around line 1664), reached from the DeleteCustomPlugin handler (src/ops/delete_custom_plugin.rs, around line 191/216). The refusal must be built from the referencing connector's ARN and must not go through invalid_parameter_because, since AWS does not blame a member here.

=== Cause 4 of 5: TagResource on an ARN the service does not hold answers NotFoundException with the connector-style parameter message instead of the tagging API's "Resource was not found" ===
Failing check: kafkaconnect.worker-configuration-delete-removes-resource

AWS: TagResource models NotFoundException (HTTP 404). Recorded behavior: TagResource against an ARN of a resource that has been deleted is refused 404 NotFoundException with the message
  "Resource was not found"
That whole string is fixed text; it interpolates nothing from the request and names no member. It is deliberately different from the read path's message: in the same recording DescribeWorkerConfiguration on the same dead ARN answers "Invalid parameter workerConfigurationArn: The requested resource doesn\u2019t exist." (the emulator already matches that one), so the two forms are per-operation, not per-service.

Emulator: answers NotFoundException — the code is right — with
  "Invalid parameter resourceArn: The requested resource doesn\u2019t exist."
The check failed with "AssertionError: Invalid parameter resourceArn: The requested resource doesn\u2019t exist."

Why: a client that asserts on, logs or branches on the tagging API's not-found message gets a different sentence from the emulator, and a test suite written against AWS fails against the emulator for a resource state that is otherwise correct. UntagResource/ListTagsForResource should be checked for the same wording while fixing this.

Fix: the TagResource handler's not-found branch (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/tag_resource.rs), which currently renders invalid_parameter_because("resourceArn", RESOURCE_DOES_NOT_EXIST) from src/state.rs (RESOURCE_DOES_NOT_EXIST around line 2070, invalid_parameter_because around line 2124). The tagging operations need their own not-found reason rather than the shared parameter-naming one.

=== Cause 5 of 5: a connector configured with logDelivery.workerLogDelivery.cloudWatchLogs never delivers any log events to the named log group ===
Failing check: kafkaconnect.kafka-connect-native-data-plane-vpc-only-skip

AWS: CreateConnector takes LogDelivery{workerLogDelivery: WorkerLogDelivery{cloudWatchLogs: CloudWatchLogsLogDelivery{enabled, logGroup}}} — "Whether log delivery to Amazon CloudWatch Logs is enabled" / "The name of the CloudWatch log group that is the destination for log delivery" (Smithy: com.amazonaws.kafkaconnect#CloudWatchLogsLogDelivery). The model states the destination but not the content, so: recorded behavior only — with cloudWatchLogs enabled against a log group the caller owns, the connector's worker writes log events into that log group, so DescribeLogStreams/GetLogEvents on it return at least one event with a non-empty message while the connector is coming up and failing.

Emulator: accepts and echoes the logDelivery configuration but never writes anything to the log group. The check creates the log group, creates the connector with cloudWatchLogs enabled, waits for the connector to leave CREATING, then polls for log events; no event ever appears and the check is killed by the runner's 60s budget ("Error: Check timed out after 60.0s"). The emulator's silence, not the budget, is the cause: a single delivered event would satisfy the poll immediately.

Why: logDelivery is the only way a caller outside the connector's VPC can observe what the worker did, so every client that relies on it — operational dashboards, failure triage, any test that asserts a connector actually started — gets an empty log group and cannot distinguish a working connector from one that never ran. Combined with Cause 1 (the connector always reads RUNNING) there is no observable signal of a connector's real outcome at all.

Fix: the CreateConnector handler / connector lifecycle in the service state (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/create_connector.rs and src/state.rs), which must emit worker log events to the configured destination as the connector walks its lifecycle (at minimum a worker-startup line and a line for the bootstrap/connect failure that drives Cause 1's FAILED transition), rather than storing logDelivery as inert echoed configuration.

## Evidence
Behavioral suite: 12/18 passed (66.7%), kafkaconnect 12/18 (67%), 6 failed, 0 errored, 0 skipped. None of the six is marked ALREADY FILED or OWNED by an open issue.

1) [mismatch] kafkaconnect.connector-create-lifecycle-reaches-failed-state
   check_kafkaconnect_connector_create_lifecycle_reaches_failed_state.py:37  assert final_state == "FAILED", final_state
   AssertionError: RUNNING
   Expected: after CreateConnector answers connectorState == "CREATING", DescribeConnector polled until the state is not CREATING rests at "FAILED" and carries a non-empty stateDescription.message.
   Actual: rests at "RUNNING"; stateDescription is absent.

2) [mismatch] kafkaconnect.update-connector-stale-version-rejected
   check_kafkaconnect_update_connector_stale_version_rejected.py:37  assert resting_state == "FAILED", resting_state
   AssertionError: RUNNING
   Same cause as (1); the stale-currentVersion assertions behind it ('Invalid parameter currentVersion.', 400 BadRequestException) were never reached.

3) [mismatch] kafkaconnect.restart-connector-state-conflict-rejected
   check_kafkaconnect_restart_connector_state_conflict_rejected.py:39  assert message == state_gate_message, message
   AssertionError: Invalid parameter connectorArn: The connector is not RUNNING.
   Expected (400 BadRequestException): "Invalid parameter connectorArn: This operation is only valid for resources that are in one of the following states: [RUNNING, PAUSED]"
   Actual   (400 BadRequestException): "Invalid parameter connectorArn: The connector is not RUNNING."
   The error code and HTTP status matched; only the message diverged.

4) [mismatch] kafkaconnect.delete-custom-plugin-in-use-by-connector-rejected
   check_kafkaconnect_delete_custom_plugin_in_use_by_connector_rejected.py:33  assert message == (...)
   AssertionError: Invalid parameter customPluginArn: The custom plugin is in use by a connector.
   Expected (400 BadRequestException): "You can't delete a custom plugin being used by one or more connectors. This custom plugin is used by the connector with arn {connector_arn}, and possibly others. Please delete connectors using this custom plugin and try again."
   Actual   (400 BadRequestException): "Invalid parameter customPluginArn: The custom plugin is in use by a connector."
   The error code and HTTP status matched; the message names no referencing connector.

5) [mismatch] kafkaconnect.worker-configuration-delete-removes-resource
   check_kafkaconnect_worker_configuration_delete_removes_resource.py:52  assert tag_message == "Resource was not found", tag_message
   AssertionError: Invalid parameter resourceArn: The requested resource doesn\u2019t exist.
   Expected (404 NotFoundException on TagResource against the deleted worker-configuration ARN): "Resource was not found"
   Actual   (404 NotFoundException): "Invalid parameter resourceArn: The requested resource doesn\u2019t exist."
   The preceding assertion in the same check — DescribeWorkerConfiguration on the dead ARN answering "Invalid parameter workerConfigurationArn: The requested resource doesn\u2019t exist." — PASSED, which is what shows the two messages are per-operation.

6) [mismatch] kafkaconnect.kafka-connect-native-data-plane-vpc-only-skip
   Error: Check timed out after 60.0s
   Expected: with logDelivery.workerLogDelivery.cloudWatchLogs {enabled: true, logGroup: <caller's group>}, DescribeLogStreams/GetLogEvents on that group return at least one event with a non-empty message.
   Actual: the log group stays empty for the whole run; the poll (600s budget, 15s interval) never sees an event and the runner's 60s cap ends the check.

Emulator source corroboration (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/):
  state.rs ~1573  settle_due(): connector.state = CONNECTOR_RUNNING.to_string();  — the only resting state a connector can take; no FAILED branch, no stored StateDescription (Cause 1).
  state.rs ~2101  connector_is_not_running() -> format!("The connector is not {CONNECTOR_RUNNING}.")  and ~934 ConnectorRecord::not_running_refusal() -> invalid_parameter_because("connectorArn", ...)  (Cause 2).
  state.rs ~2094  resource_is_in_use(resource) -> format!("The {resource} is in use by a connector."), reached via dependency_in_use_refusal ~1664 from ops/delete_custom_plugin.rs ~191/216  (Cause 3).
  state.rs ~2070  RESOURCE_DOES_NOT_EXIST + ~2124 invalid_parameter_because, used by the TagResource not-found branch  (Cause 4).
  logDelivery is stored and echoed only; nothing in the crate writes CloudWatch Logs events  (Cause 5).

## Steps to Reproduce
Boot the emulator and point a kafkaconnect client at it (region us-east-1). Prereqs for every step: an EC2 VPC with two subnets and a security group, an IAM role trusted by kafkaconnect.amazonaws.com, an S3 bucket holding a plugin zip, a CreateCustomPlugin polled to ACTIVE, and a CreateWorkerConfiguration.

Cause 1 (checks connector-create-lifecycle-reaches-failed-state, update-connector-stale-version-rejected):
1. CreateConnector with a kafkaCluster whose apacheKafkaCluster.bootstrapServers names a host that cannot resolve and a connectorConfiguration whose connector.class the plugin does not provide. Observe connectorState == "CREATING" (correct).
2. Poll DescribeConnector until connectorState != "CREATING".
   Expected: "FAILED", with a non-empty stateDescription.message.
   Actual: "RUNNING", no stateDescription.

Cause 2 (check restart-connector-state-conflict-rejected):
3. Immediately after CreateConnector (while the connector is still CREATING), call RestartConnector(connectorArn=...).
   Expected: 400 BadRequestException, message "Invalid parameter connectorArn: This operation is only valid for resources that are in one of the following states: [RUNNING, PAUSED]".
   Actual: 400 BadRequestException, message "Invalid parameter connectorArn: The connector is not RUNNING."
   The same wording is expected from UpdateConnector when the version gate passes but the state gate does not.

Cause 3 (check delete-custom-plugin-in-use-by-connector-rejected):
4. With a connector that references the custom plugin, call DeleteCustomPlugin(customPluginArn=<that plugin>).
   Expected: 400 BadRequestException, message "You can't delete a custom plugin being used by one or more connectors. This custom plugin is used by the connector with arn <the referencing connector's ARN>, and possibly others. Please delete connectors using this custom plugin and try again."
   Actual: 400 BadRequestException, message "Invalid parameter customPluginArn: The custom plugin is in use by a connector."

Cause 4 (check worker-configuration-delete-removes-resource):
5. CreateWorkerConfiguration, then DeleteWorkerConfiguration, then poll DescribeWorkerConfiguration until it raises NotFoundException (this message is already correct).
6. Call TagResource(resourceArn=<the deleted worker-configuration ARN>, tags={"probe": "dead-arn"}).
   Expected: 404 NotFoundException, message "Resource was not found".
   Actual: 404 NotFoundException, message "Invalid parameter resourceArn: The requested resource doesn\u2019t exist."

Cause 5 (check kafka-connect-native-data-plane-vpc-only-skip):
7. CreateLogGroup in CloudWatch Logs, then CreateConnector with logDelivery={"workerLogDelivery": {"cloudWatchLogs": {"enabled": true, "logGroup": <that group>}}} and the same unresolvable cluster as step 1.
8. Wait for the connector to leave CREATING, then poll DescribeLogStreams + GetLogEvents on that log group.
   Expected: at least one log event with a non-empty message.
   Actual: the log group stays empty indefinitely.
