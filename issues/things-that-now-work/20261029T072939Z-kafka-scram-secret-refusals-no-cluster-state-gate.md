Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.scram-secret-operations-require-scram-enabled-active-cluster
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.scram-secret-operations-require-scram-enabled-active-cluster|run#7|AssertionError|assert got == expected, f"{label} while CREATING: expected {expected!r}, got {got!r}"
Issue-Commit: 87ef04f300fd59d35c2e48965ae7f4a968416820

# kafka SCRAM-secret refusals: no cluster-state gate before the SASL/SCRAM-config gate, and the SASL/SCRAM-disabled message text is wrong

> _Note: this issue was identified against the code base at commit `87ef04f300fd59d35c2e48965ae7f4a968416820`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchAssociateScramSecret

## Description
Two distinct root causes in the kafka (Amazon MSK) SCRAM-secret refusal path, both observed in the same replay of kafka.scram-secret-operations-require-scram-enabled-active-cluster. NOTE: the open issue 20261027T231320Z-kafka-scram-secret-operations-are-over-permissive-no.md describes overlapping ground (its causes 2 and 3) but the replay harness did not mark this check as OWNED by it, so it is re-filed here against the current build; a fixer should treat the two as the same work and close both together.

Cause 1 of 2: SCRAM-secret operations never evaluate the cluster's STATE, so a cluster that is still CREATING is refused on its authentication configuration instead of on its state.
AWS: MSK evaluates cluster state BEFORE the cluster's client-authentication configuration for BatchAssociateScramSecret and ListScramSecrets. On a cluster in state CREATING, BatchAssociateScramSecret is refused with BadRequestException (Smithy: BatchAssociateScramSecret errors include BadRequestException, errorKind client, httpError 400; POST /v1/clusters/{ClusterArn}/scram-secrets, success code 200) carrying the message "Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets." ListScramSecrets (Smithy: same BadRequestException/400 in its error list; GET /v1/clusters/{ClusterArn}/scram-secrets) is refused with the same class and the same sentence ending "... state to list secrets." BatchDisassociateScramSecret is the documented exception: it evaluates the authentication configuration first in every state. Message anatomy: the whole sentence is FIXED text for each operation — "Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to " plus that operation's verb phrase ("associate secrets." / "list secrets."); no request value, cluster ARN, secret ARN or state name from the request is interpolated. The allowed-state list and the sentence are recorded behavior only; the Smithy model contributes only the BadRequestException/400 classification.
Emulator: BatchAssociateScramSecret on a CREATING cluster without SASL/SCRAM returns BadRequestException with the SASL/SCRAM-disabled message — the auth-config branch answered and there is no state branch at all. The handler resolves the cluster and then tests only cluster.sasl_scram_enabled(); no cluster-state precondition exists on the path. ListScramSecrets has neither a state nor a SASL/SCRAM gate and answers 200 with a secret list on a CREATING cluster. Hypothesis: on a SCRAM-ENABLED cluster that is still CREATING, associate therefore SUCCEEDS where AWS refuses it; the check aborted on its first assertion, so only the associate refusal above was directly observed.
Why: a client that provisions a cluster and immediately wires secrets gets a false green (or a mis-classified refusal) against the emulator and a 400 "Invalid cluster state" against AWS. Retry/back-off logic keyed on the "Invalid cluster state" message never fires, so these flows never wait for ACTIVE; code that reads the refusal as a configuration problem will "fix" the authentication config instead of waiting.
Fix: a shared cluster-state precondition for the SCRAM-secret operations, applied in BatchAssociateScramSecret's and ListScramSecrets' handlers BEFORE the SASL/SCRAM-enabled test, with BatchDisassociateScramSecret keeping the auth-config test first in every state; the allowed-state set and the per-operation sentences belong beside the other kafka refusal messages in the service's state module.
Traces: kafka.scram-secret-operations-require-scram-enabled-active-cluster.

Cause 2 of 2: the SASL/SCRAM-disabled refusal carries the wrong message text.
AWS: when a SCRAM-secret operation is refused because the cluster has no SASL/SCRAM client authentication, MSK answers BadRequestException (HTTP 400, the modeled error above) with the message "SASL/SCRAM client authentication must be enabled for the cluster." This sentence is entirely FIXED text — no cluster ARN, secret ARN or other request/state value is interpolated. Recorded behavior only for the wording; the model contributes the BadRequestException/400 classification.
Emulator: the same refusal carries "The cluster does not have SASL/SCRAM authentication enabled." Status (400) and error code (BadRequestException) match; only the message diverges. The text comes from one shared constant raised by both the associate and the disassociate paths, so one constant carries the divergence for every operation that raises it.
Why: the error message is part of the contract. Clients, log scrapers and error-classification tables that match MSK's sentence do not recognise the emulator's wording, so a correctly-classified refusal is mis-routed or fails the call.
Fix: the SASL/SCRAM-not-enabled message constant in the kafka service's state module that both SCRAM-secret handlers raise, so every operation using it changes once.
Traces: kafka.scram-secret-operations-require-scram-enabled-active-cluster (the observed `got` message; the same check also expects this exact sentence from BatchDisassociateScramSecret while CREATING and from both associate and disassociate once the cluster is ACTIVE).

## Evidence
Check: kafka.scram-secret-operations-require-scram-enabled-active-cluster (verdict: mismatch).
Call: BatchAssociateScramSecret(ClusterArn=<a PROVISIONED 2-broker cluster with no ClientAuthentication, DescribeClusterV2 State == CREATING>, SecretArnList=[<one well-formed AmazonMSK_* Secrets Manager ARN>]).

Expected (AWS): ('BadRequestException', 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets.')  [HTTP 400]
Actual (emulator): ('BadRequestException', 'The cluster does not have SASL/SCRAM authentication enabled.')  [HTTP 400]

Failing assertion: `associate while CREATING: expected ('BadRequestException', 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets.'), got ('BadRequestException', 'The cluster does not have SASL/SCRAM authentication enabled.')`

Both causes are visible in this one pair: the emulator answered the auth-config branch where AWS answers a state branch (cause 1), and its auth-config sentence is not AWS's 'SASL/SCRAM client authentication must be enabled for the cluster.' (cause 2). In both expected messages the entire sentence is fixed text; nothing is taken from the request or from emulator state, so no per-run value (cluster ARN, secret ARN, region, account) belongs in either message.

The same check further expects, and the emulator's handlers contain no code to produce:
  - BatchDisassociateScramSecret while CREATING -> ('BadRequestException', 'SASL/SCRAM client authentication must be enabled for the cluster.')
  - ListScramSecrets while CREATING -> ('BadRequestException', 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to list secrets.') — the emulator answers 200 with a list
  - associate and disassociate once ACTIVE -> ('BadRequestException', 'SASL/SCRAM client authentication must be enabled for the cluster.')
  - DescribeClusterV2 State unchanged (still CREATING) across all refused calls.

Code corroboration (for the fixer, not the requirement): services/service-com-amazonaws-kafka-kafka/src/ops/batch_associate_scram_secret.rs lines 230-255 — the handler resolves the cluster and tests only `cluster.sasl_scram_enabled()`, raising `crate::state::SASL_SCRAM_NOT_ENABLED_MESSAGE` (line 251); there is no cluster-state test anywhere on the path. services/service-com-amazonaws-kafka-kafka/src/ops/list_scram_secrets.rs — goes from ARN resolution straight to paging the stored set, with no state and no SASL/SCRAM gate.
Smithy (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): BatchAssociateScramSecret errors = BadRequestException(400), ForbiddenException(403), InternalServerErrorException(500), NotFoundException(404), ServiceUnavailableException(503), TooManyRequestsException(429), UnauthorizedException(401); http POST /v1/clusters/{ClusterArn}/scram-secrets code 200. ListScramSecrets has the identical error set; http GET on the same URI, code 200.

## Steps to Reproduce
1. Boot the emulator. Create a VPC with DNS support and DNS hostnames enabled and two subnets in distinct AZs, plus a self-referencing security group.
2. Call CreateClusterV2 with Provisioned{BrokerNodeGroupInfo{InstanceType=kafka.t3.small, ClientSubnets=<the two subnets>, SecurityGroups=[<sg>], StorageInfo.EbsStorageInfo.VolumeSize=1}, KafkaVersion=3.9.x, NumberOfBrokerNodes=2} and NO ClientAuthentication (SASL/SCRAM off). Confirm the response State == CREATING and do NOT wait.
3. While DescribeClusterV2 still reports CREATING, call BatchAssociateScramSecret(ClusterArn, SecretArnList=[arn:aws:secretsmanager:<region>:<account>:secret:AmazonMSK_<any-suffix>]).
   Observed: BadRequestException 'The cluster does not have SASL/SCRAM authentication enabled.'
   Expected: BadRequestException 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets.'
4. While still CREATING, call BatchDisassociateScramSecret with the same secret ARN.
   Expected: BadRequestException 'SASL/SCRAM client authentication must be enabled for the cluster.' (the auth-config check precedes the state check for disassociate in every state).
5. While still CREATING, call ListScramSecrets(ClusterArn).
   Observed: HTTP 200 with a secret list.
   Expected: BadRequestException 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to list secrets.'
6. Confirm DescribeClusterV2 State is still CREATING (no refused call may mutate it), then wait for ACTIVE and call BatchAssociateScramSecret and BatchDisassociateScramSecret again; both must return BadRequestException 'SASL/SCRAM client authentication must be enabled for the cluster.'
7. Replay kafka.scram-secret-operations-require-scram-enabled-active-cluster.
