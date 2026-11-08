Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.e2e-scram-secret-rotation-access-flow kafka.scram-secret-operations-require-scram-enabled-active-cluster
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.e2e-scram-secret-rotation-access-flow|run#14|AssertionError|assert bad_secret in unprocessed, f"expected the default-key secret to be rejected, response was: {resp}"
Issue-Key: kafka.scram-secret-operations-require-scram-enabled-active-cluster|run#4|AssertionError|assert got == expected, f"associate while CREATING: expected {expected!r}, got {got!r}"
Issue-Commit: 4147ccdd09c013bbc0cd91f761518d1fbeb30ab5

# kafka SCRAM-secret operations are over-permissive: no per-secret UnprocessedScramSecrets, no cluster-state gate, and wrong SASL/SCRAM-disabled message

> _Note: this issue was identified against the code base at commit `4147ccdd09c013bbc0cd91f761518d1fbeb30ab5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchAssociateScramSecret

## Description
Three distinct root causes in the kafka (Amazon MSK) SCRAM-secret surface. All three are in this service and all three were observed in this replay. (Note: the open issue 20261025T025102Z-kafka-listscramsecrets-stub.md covers ListScramSecrets being an unimplemented scaffold; ListScramSecrets now answers in this build, and none of the three causes below is that stub — they are fidelity defects in the implemented handlers.)

Cause 1 of 3: BatchAssociateScramSecret never reports a per-secret failure in UnprocessedScramSecrets; it associates a secret AWS refuses.
AWS: Smithy BatchAssociateScramSecret -> BatchAssociateScramSecretResponse.UnprocessedScramSecrets : list<UnprocessedScramSecret{SecretArn, ErrorCode, ErrorMessage}> ("List of errors when associating secrets to cluster"; UnprocessedScramSecret doc: "Error info for scram secret associate/disassociate failure", ErrorCode: "Error code for associate/disassociate failure"). MSK requires a SCRAM secret to be encrypted with a CUSTOMER-MANAGED KMS key; a secret encrypted with the Secrets Manager AWS-managed default key is not associable. AWS reports this as a PER-ITEM failure: HTTP 200, with the offending SecretArn present in UnprocessedScramSecrets carrying a non-empty ErrorCode (and an ErrorMessage), and the secret is NOT added to the cluster's associated set. The modeled operation has a 200 success code, so the refusal class here is the per-item entry, not a modeled exception.
Emulator: for a secret created with no KmsKeyId (Secrets Manager default key), the call returns HTTP 200 with `UnprocessedScramSecrets: []` and the secret associated. The handler's own doc states this outright (services/service-com-amazonaws-kafka-kafka/src/ops/batch_associate_scram_secret.rs lines 20-26: "UnprocessedScramSecrets is therefore empty on every accepted call ... it cannot decide those and associates the secret instead of reporting it unprocessed") and the handler unconditionally returns `unprocessed_scram_secrets: Some(Vec::new())`.
Why: a client that relies on per-item rejection (the documented contract of this batch API) is told an unusable secret is live. Credential-rotation and provisioning flows that read UnprocessedScramSecrets to detect a mis-keyed secret see a clean association, then SASL/SCRAM clients fail to authenticate at runtime with no API-level signal. Code that asserts ErrorCode is non-empty for a rejected secret gets a KeyError/empty list instead.
Fix: BatchAssociateScramSecret's handler plus the per-secret admissibility decision it needs from Secrets Manager state (whether the named secret exists in this endpoint and whether its encryption key is customer-managed), and the symmetric path in BatchDisassociateScramSecret. The per-secret outcome belongs in the response assembly, not as a whole-batch modeled error.
Traces: kafka.e2e-scram-secret-rotation-access-flow (assertion `expected the default-key secret to be rejected, response was: {... 'HTTPStatusCode': 200 ... 'UnprocessedScramSecrets': []}`).

Cause 2 of 3: SCRAM-secret operations do not check cluster STATE, so a cluster that is still CREATING is not refused on state.
AWS: MSK validates cluster state BEFORE the cluster's authentication config for BatchAssociateScramSecret and ListScramSecrets. On a cluster that is CREATING, BatchAssociateScramSecret is refused with BadRequestException (HTTP 400, modeled error of this operation) and the message "Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets."; ListScramSecrets is refused with the same class and the same sentence ending "... state to list secrets." Message anatomy: the whole sentence is FIXED text per operation — "Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to " + the operation's verb phrase ("associate secrets." / "list secrets."); no request or state value is interpolated. BatchDisassociateScramSecret is the exception: it checks the authentication config first in every state. (Recorded behavior: the check encodes exactly these three refusals; the model contributes only the BadRequestException/400 classification.)
Emulator: BatchAssociateScramSecret on a CREATING, non-SCRAM cluster returns BadRequestException with the SASL/SCRAM-disabled message, i.e. the auth-config branch answered and no state branch exists at all. The handler resolves the cluster and then tests only `cluster.sasl_scram_enabled()` (batch_associate_scram_secret.rs lines 244-255); there is no cluster-state test anywhere in it. ListScramSecrets likewise has no state (and no SASL/SCRAM) gate — it goes from ARN resolution straight to paging the stored set (services/service-com-amazonaws-kafka-kafka/src/ops/list_scram_secrets.rs lines 197-280), so on a CREATING cluster it answers 200 with a list instead of refusing. Hypothesis: for a SCRAM-ENABLED cluster that is still CREATING, associate therefore succeeds where AWS refuses it — the check aborted on the first assertion so only the associate refusal above was directly observed.
Why: callers that provision a cluster and immediately wire secrets get a false green against the emulator and a 400 against AWS (or the reverse: they classify a state problem as a configuration problem and "fix" the authentication config). Retry logic keyed on the "Invalid cluster state" message never triggers, so these flows never wait for ACTIVE.
Fix: a shared cluster-state precondition for the SCRAM-secret operations, applied in BatchAssociateScramSecret's and ListScramSecrets' handlers BEFORE the SASL/SCRAM-enabled test, with BatchDisassociateScramSecret keeping the auth-config test first in every state; the allowed-state set and the per-operation sentence belong next to the other kafka refusal messages in the service's state module.
Traces: kafka.scram-secret-operations-require-scram-enabled-active-cluster (assertion `associate while CREATING: expected ('BadRequestException', 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets.'), got ('BadRequestException', 'The cluster does not have SASL/SCRAM authentication enabled.')`).

Cause 3 of 3: the SASL/SCRAM-disabled refusal uses the wrong message text.
AWS: when a SCRAM-secret operation is refused because the cluster has no SASL/SCRAM client authentication, MSK answers BadRequestException (HTTP 400) with the message "SASL/SCRAM client authentication must be enabled for the cluster." This sentence is entirely FIXED text — no cluster ARN, secret ARN or other request/state value is interpolated into it. (Recorded behavior for the wording; the model contributes the BadRequestException/400 classification.)
Emulator: the same refusal carries "The cluster does not have SASL/SCRAM authentication enabled." The status (400) and error code (BadRequestException) match; only the message diverges. The text comes from the single constant `crate::state::SASL_SCRAM_NOT_ENABLED_MESSAGE`, used by the associate path (batch_associate_scram_secret.rs line 251) and the disassociate path, so one constant carries the divergence for every operation that raises it.
Why: an error message is part of the contract. Clients and test suites that match on the MSK sentence (log scrapers, error-classification tables, assertions on Error.Message) do not recognise the emulator's wording and mis-route or fail the call, even though the code is right.
Fix: the message constant in the kafka service's state module that both SCRAM-secret handlers raise, so every operation using it changes once.
Traces: kafka.scram-secret-operations-require-scram-enabled-active-cluster (the `got` message in the assertion above; the same check also expects this sentence from BatchDisassociateScramSecret in CREATING and from both associate and disassociate while ACTIVE).

## Evidence
Cause 1 — kafka.e2e-scram-secret-rotation-access-flow:
  Expected (AWS): HTTP 200 with the default-key secret present in UnprocessedScramSecrets, its ErrorCode non-empty, and the secret absent from ListScramSecrets.
  Actual (emulator): HTTP 200, body `{'ClusterArn': 'arn:aws:kafka:<region>:<account>:cluster/<name>/<uuid>-1', 'UnprocessedScramSecrets': []}` — the secret was associated. Assertion: `expected the default-key secret to be rejected, response was: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'UnprocessedScramSecrets': []}`.
  (The ARN/name/uuid above are per-run values from this recording, not part of the requirement.)

Causes 2 and 3 — kafka.scram-secret-operations-require-scram-enabled-active-cluster, BatchAssociateScramSecret on a non-SCRAM cluster in state CREATING:
  Expected (AWS): ('BadRequestException', 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets.')
  Actual (emulator): ('BadRequestException', 'The cluster does not have SASL/SCRAM authentication enabled.')
  Both divergences are visible in this one pair: the emulator answered the auth-config branch instead of a state branch (cause 2), and its auth-config sentence is not AWS's 'SASL/SCRAM client authentication must be enabled for the cluster.' (cause 3).
  The same check further expects, and the emulator's handlers contain no code to produce: ('BadRequestException', 'SASL/SCRAM client authentication must be enabled for the cluster.') from BatchDisassociateScramSecret while CREATING; ('BadRequestException', 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to list secrets.') from ListScramSecrets while CREATING; and the same SASL/SCRAM sentence from associate and disassociate once the cluster is ACTIVE.

Code corroboration (for the fixer, not the requirement): services/service-com-amazonaws-kafka-kafka/src/ops/batch_associate_scram_secret.rs lines 20-26 (doc: associates instead of reporting unprocessed), 244-255 (only sasl_scram_enabled() is tested; no state test), 279-286 (unprocessed_scram_secrets always empty); services/service-com-amazonaws-kafka-kafka/src/ops/list_scram_secrets.rs lines 197-280 (no state or SASL/SCRAM gate).

## Steps to Reproduce
Cause 1:
1. Boot the emulator and create a PROVISIONED MSK cluster via CreateClusterV2 with Provisioned.ClientAuthentication.Sasl.Scram.Enabled = true; wait for DescribeClusterV2 State == ACTIVE.
2. Create a Secrets Manager secret named AmazonMSK_<suffix> WITH a customer-managed KMS key (CreateKey + CreateSecret KmsKeyId=<key>); BatchAssociateScramSecret it and confirm UnprocessedScramSecrets == [] and ListScramSecrets lists it.
3. Create a second secret named AmazonMSK_<suffix> with NO KmsKeyId (Secrets Manager default key).
4. Call BatchAssociateScramSecret(ClusterArn, SecretArnList=[that second secret]).
   Observed: 200 with UnprocessedScramSecrets == [] and the secret associated.
   Expected: 200 with that SecretArn in UnprocessedScramSecrets, non-empty ErrorCode, and the secret absent from ListScramSecrets while the first secret stays associated.
5. Replay kafka.e2e-scram-secret-rotation-access-flow.

Causes 2 and 3:
1. Create a PROVISIONED 2-broker cluster via CreateClusterV2 with NO ClientAuthentication (so SASL/SCRAM is off). Do not wait: the cluster is CREATING.
2. While it is CREATING, call BatchAssociateScramSecret(ClusterArn, SecretArnList=[any well-formed AmazonMSK_* secret ARN]).
   Observed: BadRequestException 'The cluster does not have SASL/SCRAM authentication enabled.'
   Expected: BadRequestException 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to associate secrets.'
3. While still CREATING, call ListScramSecrets(ClusterArn).
   Observed: 200 with a secret list.
   Expected: BadRequestException 'Invalid cluster state. Cluster must be in the ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER state to list secrets.'
4. While still CREATING, call BatchDisassociateScramSecret with the same secret ARN; expect BadRequestException 'SASL/SCRAM client authentication must be enabled for the cluster.' (the auth-config check precedes the state check for disassociate in every state).
5. Confirm DescribeClusterV2 State is still CREATING (no refused call may mutate it), wait for ACTIVE, then call BatchAssociateScramSecret and BatchDisassociateScramSecret again; both must return BadRequestException 'SASL/SCRAM client authentication must be enabled for the cluster.'
6. Replay kafka.scram-secret-operations-require-scram-enabled-active-cluster.
