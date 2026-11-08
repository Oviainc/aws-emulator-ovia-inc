Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.server-side-scram-secret-kms-encryption-required
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.server-side-scram-secret-kms-encryption-required|run#18|AssertionError|assert error_code == "InvalidSecretArn", (
Issue-Commit: a946db3d2c92d3ec27b0f0d3beb3e4ffad06b536

# BatchAssociateScramSecret reports the default-KMS-key rejection under ErrorCode InvalidSecretKmsKeyId instead of AWS's InvalidSecretArn

> _Note: this issue was identified against the code base at commit `a946db3d2c92d3ec27b0f0d3beb3e4ffad06b536`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchAssociateScramSecret

## Description
ONE root cause in the MSK SCRAM-associate per-item outcome: the per-secret rejection of a secret encrypted with the Secrets Manager default (AWS-managed) key is now produced, but it is classified under an ErrorCode that AWS does not use for it.

Note on existing coverage: the open issue 20261104T171616Z-kafka-scram-batch-operations-report-no-per-secret-failures.md lists this check id under its Cause 2 ('BatchAssociateScramSecret associates a secret encrypted with the Secrets Manager default key'), but that cause is about the entry being ABSENT (UnprocessedScramSecrets == []). The emulator now DOES return the entry, so that cause's replay key no longer matches and the suite does not mark this check OWNED. What is left is a different defect — the entry's ErrorCode value — and it is unowned.

Cause 1 of 1: the per-item ErrorCode for a default-KMS-key SCRAM secret is wrong.
AWS: BatchAssociateScramSecret (POST /v1/clusters/{ClusterArn}/scram-secrets, 200) returns BatchAssociateScramSecretResponse{ClusterArn, UnprocessedScramSecrets: list<UnprocessedScramSecret{SecretArn, ErrorCode, ErrorMessage}>}; Smithy documents UnprocessedScramSecret.ErrorCode as 'Error code for associate/disassociate failure' and types it as a free-form __string, so the model does not enumerate the values — recorded behavior only. For a secret whose encryption key is the account default aws/secretsmanager key (MSK requires a customer-managed KMS key), AWS answers HTTP 200 with that SecretArn in UnprocessedScramSecrets carrying ErrorCode exactly 'InvalidSecretArn' and an ErrorMessage that names the default-key restriction (the fixed text is AWS's wording for that error class; the only variable part is the secret ARN echoed in SecretArn, taken from the request). The secret is not added to the cluster's associated set, and a customer-key secret in the same batch is still associated.
Emulator: HTTP 200 with the entry present but classified as ErrorCode 'InvalidSecretKmsKeyId' and ErrorMessage 'The secret must be encrypted with a customer managed AWS KMS key.' — an error code AWS does not emit for this surface.
Why: ErrorCode is the only machine-readable discriminator in a 200 batch response; a client that branches on it (retry vs. re-key vs. abort, or a provisioning flow that matches the documented 'InvalidSecretArn' to report a mis-keyed secret) falls through its match arms and treats a known, actionable rejection as an unknown failure. Code written against AWS never sees 'InvalidSecretKmsKeyId'.
Fix: the per-secret admissibility decision in BatchAssociateScramSecret's handler must label the default/AWS-managed-key outcome with AWS's error code and message wording for that class when it assembles the UnprocessedScramSecrets entry, rather than coining its own code.
Traces: kafka.server-side-scram-secret-kms-encryption-required.

## Evidence
Failing check kafka.server-side-scram-secret-kms-encryption-required, verdict mismatch; not marked ALREADY FILED and not marked OWNED by the suite report.

Request: BatchAssociateScramSecret(ClusterArn=<a PROVISIONED cluster with Sasl.Scram.Enabled=true, ACTIVE>, SecretArnList=[<secret created WITH a customer-managed KmsKeyId>, <secret created with NO KmsKeyId>]).

Expected (AWS): HTTP 200; UnprocessedScramSecrets contains the no-KmsKeyId secret's ARN with ErrorCode == "InvalidSecretArn" and an ErrorMessage naming the default-key restriction; the customer-key secret absent from UnprocessedScramSecrets and present in ListScramSecrets; the default-key secret absent from ListScramSecrets.

Actual (emulator): the entry comes back as
  {'ErrorCode': 'InvalidSecretKmsKeyId', 'ErrorMessage': 'The secret must be encrypted with a customer managed AWS KMS key.', 'SecretArn': 'arn:aws:secretsmanager:us-west-2:000000000000:secret:AmazonMSK_kmsbad<suffix>'}
(the ARN, account and name suffix are this run's generated values, not part of the requirement), producing
  AssertionError: expected the default-key secret rejection to report the specific ErrorCode InvalidSecretArn, got: {'ErrorCode': 'InvalidSecretKmsKeyId', ...}
at check_kafka_server_side_scram_secret_kms_encryption_required.py line 65.

The diverging field is the error code: 'InvalidSecretKmsKeyId' served where 'InvalidSecretArn' is required. The HTTP status (200) and the presence/placement of the entry match. The ErrorMessage's fixed text must be AWS's wording for the InvalidSecretArn/default-key class; its only variable part is the offending secret ARN from the request.

Smithy cited (services/service-com-amazonaws-kafka-kafka/smithy-model/com-amazonaws-kafka-kafka.json): UnprocessedScramSecret{ErrorCode: __string 'Error code for associate/disassociate failure', ErrorMessage: __string, SecretArn: __string}; BatchAssociateScramSecretResponse{ClusterArn, UnprocessedScramSecrets: listOfUnprocessedScramSecret 'List of errors when associating secrets to cluster'}. ErrorCode is unconstrained in the model, so the value is a recorded-behavior requirement.

## Steps to Reproduce
1. Boot the emulator. Create a PROVISIONED MSK cluster via CreateClusterV2 with ClientAuthentication.Sasl.Scram.Enabled=true and wait for State == ACTIVE.
2. Create a KMS customer-managed key. Create a Secrets Manager secret named AmazonMSK_<suffix> WITH KmsKeyId set to that key.
3. Create a second Secrets Manager secret named AmazonMSK_<suffix> with NO KmsKeyId (so it falls back to the account default aws/secretsmanager key).
4. Call BatchAssociateScramSecret(ClusterArn, SecretArnList=[customer-key secret, default-key secret]).
5. Assert HTTP 200 and that the default-key secret's entry in UnprocessedScramSecrets has ErrorCode == "InvalidSecretArn" with an ErrorMessage naming the default-key restriction. The emulator instead reports ErrorCode 'InvalidSecretKmsKeyId'.
6. Also assert the customer-key secret is absent from UnprocessedScramSecrets and present in ListScramSecrets, and the default-key secret is absent from ListScramSecrets.
7. Replay kafka.server-side-scram-secret-kms-encryption-required.
