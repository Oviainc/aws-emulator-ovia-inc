Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete
Issue-Key: ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# ECR CreateRepository with encryptionType=KMS creates no KMS grant on the customer CMK (1 cause)

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
One root cause, spanning ECR and KMS, found by the behavioral suite. No open issue carries this check's replay key: the ECR bundles 20261024T014332Z-ecr-asynchronous-effects-never-occur-and-several-enforcement.md, 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md and 20261024T080016Z-ecr-image-storage-class-transitions-scan-evaluation-media.md do not mention encryptionConfiguration at all, and the KMS issues on file (e.g. 20261024T102105Z-iam-kms-a-key-policy-s-explicit-deny-does-not.md) concern key-policy evaluation, not grant creation by ECR.

Cause 1 of 1: creating an ECR repository with a customer-managed CMK produces no grant on that key, so the cross-service effect of encryptionConfiguration never happens.

AWS: Smithy (com.amazonaws.ecr): CreateRepositoryRequest.encryptionConfiguration targets EncryptionConfiguration (encryptionType AES256|KMS|KMS_DSSE, kmsKey), and CreateRepository declares KmsException (client error, HTTP 400) — the operation is modelled as doing real work against KMS, not as storing an opaque blob. The model cannot express the grant itself, so this is recorded behavior: on real AWS a CreateRepository with encryptionConfiguration={"encryptionType":"KMS","kmsKey":<CMK ARN>} causes ECR to mint grants on that CMK that are immediately visible through kms:ListGrants — the recording measures exactly two grants, created in the same second as the repository, byte-identical to one another except for GrantId and CreationDate, each with GranteePrincipal naming an ECR service principal, RetiringPrincipal and IssuingAccount set, Operations ["Decrypt","GenerateDataKey"], and Constraints.EncryptionContextSubset naming the repository ARN. Deleting the repository retires them, and an AES256 repository adds none. Grant listing is eventually consistent, so the check polls kms:ListGrants for up to 60 s.

Emulator: kms:ListGrants on the CMK stays empty after the KMS-encrypted repository is created. The check's grant poll (predicate len(found) >= 1, timeout_s = GRANT_TIMEOUT_S = 60.0, interval 3.0 s) is the only unbounded wait reached before the harness cap, and the check is cut off with "Check timed out after 60.0s" — the full poll budget was burned with zero grants observed. Everything before that poll is a handful of fast control-plane calls (kms:CreateKey, kms:DescribeKey, a baseline kms:ListGrants that correctly returns {}, ecr:CreateRepository, ecr:DescribeRepositories), so the 60 s was spent waiting for a grant that never appears. CreateRepository itself returns HTTP 200 and echoes encryptionConfiguration back, so the field is stored and replayed but has no consumer-side effect.

Why: "encrypted with your key" becomes a string with nothing behind it. A client that audits which services hold authorizations on its CMK (kms:ListGrants, key-policy review, CloudTrail) sees nothing and concludes the repository is not key-encrypted; revoking or rotating the key has no observable relationship to the repository; a teardown reconciler that checks for leaked grants after deleting a repository cannot distinguish "ECR retired its grants" from "ECR never made any". Any flow that waits for the grant to exist before treating the repository as usable blocks until its own deadline.

Fix: the CreateRepository handler's encryptionConfiguration path must, for encryptionType KMS (and KMS_DSSE), create the grant(s) in the emulator's KMS state keyed to the named CMK — grantee an ECR service principal, Operations including Decrypt and a GenerateDataKey* operation, Constraints.EncryptionContextSubset naming the repository ARN — and DeleteRepository must retire them; encryptionType AES256 must create none. The companion assertion that DescribeRepositories reports kmsKey as the full key ARN (even when the create call passed a key id or alias) belongs in the same normalisation step.

Traces: ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete.

## Evidence
Cause 1 — check cut off with "Check timed out after 60.0s". The check creates a SYMMETRIC_DEFAULT ENCRYPT_DECRYPT CMK, asserts the baseline kms:ListGrants for it is {} (this assertion passes, so the listing path itself works), creates an ECR repository with encryptionConfiguration={"encryptionType":"KMS","kmsKey":<key ARN>}, then polls _grants(kms, key_id) for len >= 1 with timeout_s=60.0 / interval_s=3.0. The poll never satisfies its predicate and the harness 60 s cap fires inside it. Expected: at least one grant visible within that window (the recording measures exactly two, identical but for GrantId/CreationDate, Operations ["Decrypt","GenerateDataKey"], Constraints.EncryptionContextSubset naming the repository ARN). CreateRepository's own response is HTTP 200 with encryptionConfiguration.encryptionType == "KMS" echoed back, so the divergence is the absent KMS-side effect, not the request's acceptance.

## Steps to Reproduce
Against the emulator's ECR and KMS endpoints in one region:
1. key = kms.create_key(KeyUsage='ENCRYPT_DECRYPT', KeySpec='SYMMETRIC_DEFAULT'); note KeyId and Arn.
2. Confirm kms.list_grants(KeyId=key_id) returns an empty Grants list.
3. ecr.create_repository(repositoryName='<name>', encryptionConfiguration={'encryptionType': 'KMS', 'kmsKey': <key Arn>}) — observe HTTP 200 and the echoed encryptionConfiguration.
4. Poll kms.list_grants(KeyId=key_id) (paginating NextMarker) for up to 60 s: it stays empty, where real AWS shows the ECR grants within seconds.
5. ecr.describe_repositories(repositoryNames=['<name>']) and compare encryptionConfiguration.kmsKey with kms.describe_key(KeyId=key_id)['KeyMetadata']['Arn'] — both services must name the key by the same full ARN.
6. Control: ecr.create_repository with encryptionConfiguration={'encryptionType': 'AES256'} must add no grant to the CMK.
7. ecr.delete_repository(repositoryName='<name>', force=True) and poll kms.list_grants again: the grants created in step 3/4 must be retired and the listing must converge back to empty.
