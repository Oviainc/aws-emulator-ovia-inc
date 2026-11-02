Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-fresh-repository-has-no-branches-or-default
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-fresh-repository-has-no-branches-or-default
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetRepository|2xx->2xx|$.repositoryMetadata.kmsKeyId
Issue-Commit: 48155f868f031d161db2772517918769f1926511

# CodeCommit GetRepository reports kmsKeyId as the alias selector

> _Note: this issue was identified against the code base at commit `48155f868f031d161db2772517918769f1926511`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepository

## Description
Cause 1 of 1: GetRepository answers 200 with repositoryMetadata.kmsKeyId set to the literal KMS alias path of the service-managed key instead of a resolved KMS key identifier, so a read of an existing repository advertises a selector where AWS advertises a key.

AWS: Smithy RepositoryMetadata.kmsKeyId (target KmsKeyId, pattern ^[a-zA-Z0-9:/_-]+$) is documented as 'The ID of the Key Management Service encryption key used to encrypt and decrypt the repository', and CreateRepositoryInput.kmsKeyId documents only the default SELECTION ('If no key is specified, the default aws/codecommit Amazon Web Services managed key is used'). What the member must CARRY is a key identifier. Recorded behavior: for a repository created without an explicit kmsKeyId, GetRepository's repositoryMetadata.kmsKeyId holds an account- and region-specific KMS key identifier — a key ARN of the form arn:aws:kms:<region>:<account-id>:key/<key-uuid> (or a bare key UUID). The recording does not assert a literal here precisely because that identity is a per-account value; the requirement is the FORM (a resolved key ARN/UUID in the repository's own account and region), never the alias path, and never any one recorded key id.

Emulator: the same step answers 200 with $.repositoryMetadata.kmsKeyId = 'alias/aws/codecommit' — the alias selector itself, stored verbatim at creation and projected unchanged on read. Because the repository-metadata projection is shared, GetRepository reports whatever string creation stored, so the defect is visible on the read path independently of the create path.

Why: a client that reads repositoryMetadata.kmsKeyId to identify the CMK protecting an existing repository gets a selector, not an identity. Drift detection, a Config/CloudFormation comparison, or an audit that joins repositories to kms:DescribeKey/ListAliases output by key id or ARN never matches; anything parsing the value as an ARN to recover the key's account, region or key id fails outright. The alias also hides WHICH key is in force, so two repositories on different keys are indistinguishable.

Fix: the encryption-key canonicalization and repository-metadata projection in the codecommit service's state (services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs) — every accepted KMS selector form (key id, key ARN, alias name, alias ARN) must be resolved to the key ARN of the selected key in the repository's own account and region before it is stored and reported, with the default aws/codecommit managed key resolving to a stable managed-key ARN for that account and region. The acceptance rule for selectors stays as-is; only the stored/reported canonical form changes.

Note: open issue 20261102T022028Z-codecommit-repositorymetadata-kmskeyid-reports-the-alias-path-instead.md reports the same projection fault on the CreateRepository response and names the shared state as the fix site, but it does not carry this GetRepository step's replay key, so the read path is not exercised by its replay pointer and would be closed unrun.

Traces: codecommit-fresh-repository-has-no-branches-or-default#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true). Cluster service=codecommit|GetRepository|status=200->200|keys=$.repositoryMetadata.kmsKeyId, fixture codecommit-fresh-repository-has-no-branches-or-default, step task#6, verdict mismatch, no owning issue and no stub path.

Status 200 expected and 200 observed; the body diverges on exactly one key:
  $.repositoryMetadata.kmsKeyId: value_mismatch (expected '<shaped-not-asserted>', got 'alias/aws/codecommit')

Expected: a resolved KMS key identifier for the repository's account and region (arn:aws:kms:<region>:<account-id>:key/<key-uuid>, or a bare key UUID) — the recording holds a per-account value and therefore asserts the shape rather than a literal; that shape, not any one key id, is the requirement.
Actual: the fixed alias selector string 'alias/aws/codecommit'.

No status, error code or error message is involved — this is a success-path value-form divergence on a read.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Create a repository without naming a key: codecommit CreateRepository with only repositoryName=<any valid name>.
3. Call codecommit GetRepository for that repository and read repositoryMetadata.kmsKeyId: the emulator returns 'alias/aws/codecommit'; real CodeCommit returns a KMS key identifier for the account/region (an arn:aws:kms:<region>:<account-id>:key/<key-uuid> ARN, or a bare key UUID).
4. Re-replay fixture codecommit-fresh-repository-has-no-branches-or-default step task#6 to confirm the fix on the read path.
