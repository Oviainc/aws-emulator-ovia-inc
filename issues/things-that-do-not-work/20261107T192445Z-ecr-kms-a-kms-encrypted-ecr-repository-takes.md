Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete
Issue-Key: ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete|_poll_until#17|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: ab92470025dd8eeb501814706ada4aa2b617e26c

# ecr__kms: a KMS-encrypted ECR repository takes no KMS grant on the customer CMK — ListGrants stays empty after CreateRepository (1 cause)

> _Note: this issue was identified against the code base at commit `ab92470025dd8eeb501814706ada4aa2b617e26c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
ONE net-new root cause; no open issue carries this check (the suite report does not mark it OWNED, and no ecr__kms issue exists).

Cause 1 of 1: CreateRepository with encryptionConfiguration{encryptionType:'KMS', kmsKey:<customer CMK ARN>} creates no grant on that key.
AWS: Smithy: CreateRepository accepts encryptionConfiguration and declares KmsException (HTTP 400), i.e. the repository's creation really involves KMS. Recorded behavior (and the ECR encryption-at-rest documentation the check cites): creating a repository encrypted with a customer managed key makes ECR take a grant on that CMK for its own service principal, limited to the data-key operations it needs (Decrypt and GenerateDataKey*/GenerateDataKeyWithoutPlaintext); kms:ListGrants on the key then returns the grant(s) ECR created — in the recording exactly two otherwise identical grants differing only in GrantId and CreationDate — all with the same grantee identity; an AES256 repository on the same key adds none, and deleting the repository retires them.
Emulator: kms:ListGrants on the key returns {} for the whole 60 s eventually-consistent poll after the repository was created; the repository itself is created and DescribeRepositories reports the key, so the ECR side looks healthy while KMS has no record of the authorization. Grant creation is simply not performed (so the retire-on-delete half of the contract is unreachable too).
Why: the KMS-grant trail is how an operator audits which services hold what authorization on a CMK, and how a key owner reasons about revocation. A client reading ListGrants sees an untouched key while ECR is nominally encrypting with it, so grant/RevokeGrant lifecycle testing, key-policy-plus-grant least-privilege reviews, and the expected grant retirement on repository deletion all produce the wrong answer; the emulator also never exercises the KmsException path a key that cannot be granted on would take.
Fix: the CreateRepository handler's KMS integration — create the ECR service grant(s) on the named CMK (Decrypt + GenerateDataKey*) in KMS state as part of creating a KMS-encrypted repository, and retire them in the DeleteRepository path.
Traces: ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete.

## Evidence
[mismatch] ecr__kms.kms-encrypted-repository-creates-exactly-one-ecr-grant-and-retires-it-on-delete — AssertionError: "kms:ListGrants showing the grant(s) ECR must create on arn:aws:kms:<region>:<account>:key/<key-id> for repository '<repo>' (KMS grant listing is eventually consistent) not observed within 60.0s; last saw {}". Preconditions all held: CreateKey returned an Enabled SYMMETRIC_DEFAULT key, DescribeKey agreed on its ARN, ListGrants was empty at baseline, and CreateRepository echoed encryptionConfiguration.encryptionType 'KMS' with DescribeRepositories reporting the same key ARN. Expected: at least one grant (recorded AWS: two identical grants, one grantee identity, operations including Decrypt and GenerateDataKey/GenerateDataKeyWithoutPlaintext), retired when the repository is deleted.

## Steps to Reproduce
kms create_key(KeyUsage='ENCRYPT_DECRYPT', KeySpec='SYMMETRIC_DEFAULT'); kms list_grants(KeyId=<id>) -> empty; ecr create_repository(repositoryName=<repo>, encryptionConfiguration={'encryptionType':'KMS','kmsKey':<key ARN>}); poll kms list_grants(KeyId=<id>) — it stays empty. Then ecr delete_repository(force=True) and observe that there is likewise no grant to retire.
