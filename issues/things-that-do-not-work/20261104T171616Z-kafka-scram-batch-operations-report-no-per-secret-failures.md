Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.batch-disassociate-scram-secret-not-associated-rejected kafka.batch-disassociate-scram-secret-unassociated-secret-rejected kafka.batch-disassociate-unassociated-secret-rejected kafka.scram-disassociate-nonexistent-secret-rejected kafka.scram-secret-batch-operations-partial-failure-isolation kafka.server-side-scram-secret-kms-encryption-required
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.batch-disassociate-scram-secret-not-associated-rejected|run#10|AssertionError|assert len(unprocessed) == 1 and unprocessed[0].get("SecretArn") == secret_b, (
Issue-Key: kafka.batch-disassociate-scram-secret-unassociated-secret-rejected|run#14|AssertionError|assert len(unprocessed) == 1 and unprocessed[0].get("SecretArn") == secret_never_associated, (
Issue-Key: kafka.batch-disassociate-unassociated-secret-rejected|run#15|AssertionError|assert len(unprocessed) == 1 and unprocessed[0]["SecretArn"] == unassociated_arn, (
Issue-Key: kafka.scram-disassociate-nonexistent-secret-rejected|run#15|AssertionError|assert len(matches) == 1, f"expected a per-secret error entry for the bogus ARN, got {unprocessed}"
Issue-Key: kafka.scram-secret-batch-operations-partial-failure-isolation|run#11|AssertionError|assert len(unprocessed) == 1, f"expected exactly one unprocessed entry, got {unprocessed}"
Issue-Key: kafka.server-side-scram-secret-kms-encryption-required|run#15|AssertionError|assert bad_arn in unprocessed, f"expected the default-key secret to be rejected, response was: {resp}"
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# SCRAM batch operations report no per-secret failures: BatchDisassociateScramSecret returns an empty UnprocessedScramSecrets for secrets that are not associated, and BatchAssociateScramSecret accepts a default-KMS-key secret (2 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDisassociateScramSecret

## Description
TWO distinct root causes in the MSK SCRAM-secret batch surface, both in the per-item outcome of a 200 response. They are in two different handlers and must be closed separately; both must be closed before this issue is submitted. The open issue 20261027T231320Z-kafka-scram-secret-operations-are-over-permissive-no.md describes the associate-side per-item gap and mentions 'the symmetric path in BatchDisassociateScramSecret' in its Fix prose, but it carries none of the check ids below (its trace ids are kafka.e2e-scram-secret-rotation-access-flow and kafka.scram-secret-operations-require-scram-enabled-active-cluster), so the disassociate path and the associate KMS rule are unowned here.

Cause 1 of 2: BatchDisassociateScramSecret reports nothing unprocessed for a secret that is not associated with the cluster.
AWS: BatchDisassociateScramSecret (PATCH /v1/clusters/{ClusterArn}/scram-secrets, 200) returns BatchDisassociateScramSecretResponse{ClusterArn, UnprocessedScramSecrets: list<UnprocessedScramSecret{SecretArn, ErrorCode, ErrorMessage}>} — 'List of errors when disassociating secrets to cluster'. A secret the cluster never had associated (and likewise a secret ARN that does not resolve at all) is a PER-ITEM failure, not a modeled exception: the call answers HTTP 200 with that SecretArn present in UnprocessedScramSecrets carrying a non-empty ErrorCode (and an ErrorMessage), the cluster's associated set is unchanged, and a subsequent disassociate of a secret that IS associated succeeds. In a mixed batch the associated secret is removed while the never-associated one comes back unprocessed, so the two outcomes are isolated per item.
Emulator: HTTP 200 with UnprocessedScramSecrets == [] in every case — a never-associated secret, a bogus secret ARN and a mixed batch all report a clean success, so a caller cannot distinguish 'removed' from 'was never there'.
Why: the per-item list is the documented contract of this batch API. Credential-rotation and de-provisioning flows that read UnprocessedScramSecrets to detect a secret they failed to detach see a clean result and move on; code that indexes unprocessed[0]['ErrorCode'] gets an IndexError; and partial-failure isolation (one item fails, the rest apply) is untestable.
Fix: BatchDisassociateScramSecret's handler must decide each requested secret's admissibility against the cluster's associated-secret set (and the secret's resolvability) and assemble the per-item entries in the response, instead of returning an always-empty list.
Traces: kafka.batch-disassociate-scram-secret-not-associated-rejected, kafka.batch-disassociate-scram-secret-unassociated-secret-rejected, kafka.batch-disassociate-unassociated-secret-rejected, kafka.scram-disassociate-nonexistent-secret-rejected, kafka.scram-secret-batch-operations-partial-failure-isolation.

Cause 2 of 2: BatchAssociateScramSecret associates a secret encrypted with the Secrets Manager default (AWS-managed) key.
AWS: MSK requires a SCRAM secret to be encrypted with a CUSTOMER-MANAGED KMS key. A secret created with no KmsKeyId is not associable and is reported as a per-item failure: HTTP 200 with that SecretArn in UnprocessedScramSecrets carrying a non-empty ErrorCode, the secret NOT added to the cluster's associated set, and any customer-key secret in the same batch still associated.
Emulator: HTTP 200 with {'ClusterArn': ..., 'UnprocessedScramSecrets': []} and the default-key secret associated.
Why: a client is told an unusable credential is live; SASL/SCRAM clients then fail to authenticate at runtime with no API-level signal, and a provisioning flow that checks UnprocessedScramSecrets to catch a mis-keyed secret never catches it.
Fix: the per-secret admissibility decision in BatchAssociateScramSecret's handler (does the named secret exist in this endpoint, and is its encryption key customer-managed), surfaced as a per-item entry in the response assembly.
Traces: kafka.server-side-scram-secret-kms-encryption-required.

## Evidence
All six checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

Cause 1 — representative: kafka.batch-disassociate-scram-secret-not-associated-rejected. Secret A is associated, secret B never is; BatchDisassociateScramSecret(ClusterArn, SecretArnList=[B]).
  Expected: HTTP 200 with UnprocessedScramSecrets == [{'SecretArn': B, 'ErrorCode': <non-empty>, 'ErrorMessage': ...}], ListScramSecrets still showing only A, and a later disassociate of A succeeding.
  Actual: AssertionError "expected 'arn:aws:secretsmanager:us-west-2:000000000000:secret:AmazonMSK_sim2real_b_<suffix>' to come back as the sole unprocessed item, got []" (the ARN is this run's value).
  Same empty-list divergence: kafka.batch-disassociate-scram-secret-unassociated-secret-rejected ('expected the never-associated secret to come back unprocessed, got []'), kafka.batch-disassociate-unassociated-secret-rejected (same), kafka.scram-disassociate-nonexistent-secret-rejected ('expected a per-secret error entry for the bogus ARN, got []'), kafka.scram-secret-batch-operations-partial-failure-isolation ('expected exactly one unprocessed entry, got []').

Cause 2 — kafka.server-side-scram-secret-kms-encryption-required: BatchAssociateScramSecret with a secret created with no KmsKeyId.
  Expected: HTTP 200 with that SecretArn in UnprocessedScramSecrets, non-empty ErrorCode, and the secret absent from ListScramSecrets.
  Actual: HTTP 200 {'ClusterArn': 'arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-kmsreq-<id>/<uuid>-1', 'UnprocessedScramSecrets': []} — AssertionError 'expected the default-key secret to be rejected'.

Smithy cited: BatchDisassociateScramSecret PATCH /v1/clusters/{ClusterArn}/scram-secrets code 200; input ClusterArn (httpLabel, required) and SecretArnList (required); output ClusterArn + UnprocessedScramSecrets (list of UnprocessedScramSecret). The 200 success code is what makes the per-item entry, not a modeled exception, the refusal channel.

## Steps to Reproduce
1. Boot the emulator, create a PROVISIONED cluster with Sasl.Scram.Enabled=true and wait for ACTIVE.
2. Create two Secrets Manager secrets named AmazonMSK_<suffix> with a customer-managed KMS key; associate the first with BatchAssociateScramSecret and confirm UnprocessedScramSecrets == [] and ListScramSecrets lists it.
3. Cause 1: call BatchDisassociateScramSecret(ClusterArn, SecretArnList=[the second, never-associated secret]). Assert HTTP 200 with exactly one UnprocessedScramSecrets entry whose SecretArn is that secret and whose ErrorCode is non-empty, and that ListScramSecrets still shows only the first secret. Repeat with a well-formed but nonexistent secret ARN and assert the same per-item shape. Then call it with a mixed list [associated, never-associated] and assert the associated one is removed while only the never-associated one comes back unprocessed. Finally disassociate the associated secret and assert UnprocessedScramSecrets == [].
4. Cause 2: create a third AmazonMSK_<suffix> secret with NO KmsKeyId and call BatchAssociateScramSecret with it. Assert HTTP 200 with that SecretArn in UnprocessedScramSecrets, a non-empty ErrorCode, and that ListScramSecrets does not list it.
5. Replay kafka.batch-disassociate-scram-secret-not-associated-rejected, kafka.batch-disassociate-scram-secret-unassociated-secret-rejected, kafka.batch-disassociate-unassociated-secret-rejected, kafka.scram-disassociate-nonexistent-secret-rejected, kafka.scram-secret-batch-operations-partial-failure-isolation and kafka.server-side-scram-secret-kms-encryption-required.
