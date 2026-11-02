Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-branch-and-commit-workflow
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-branch-and-commit-workflow codecommit-delete-repository-and-verify-gone codecommit-file-folder-blob-content-operations codecommit-put-file-parent-commit-errors codecommit-repo-binary-file-diff codecommit-repo-lifecycle-with-commit-and-branch
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|CreateRepository|2xx->2xx|$.repositoryMetadata.kmsKeyId
Issue-Commit: dfbd73ef7a7cd913c8e0967b143e464d80e7368c

# CodeCommit repositoryMetadata.kmsKeyId reports the alias path instead of a resolved KMS key identifier

> _Note: this issue was identified against the code base at commit `dfbd73ef7a7cd913c8e0967b143e464d80e7368c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Cause 1 of 1: CreateRepository answers 200 with `repositoryMetadata.kmsKeyId` set to the literal KMS *alias path* for the service-managed key instead of a KMS key *identifier*, so every repository the emulator creates without an explicit `kmsKeyId` advertises an alias where AWS advertises a key.

AWS: `CreateRepository` returns `repositoryMetadata`, whose `kmsKeyId` member is documented as "The ID of the Key Management Service encryption key used to encrypt and decrypt the repository." (Smithy: `RepositoryMetadata.kmsKeyId`, target `KmsKeyId`, pattern `^[a-zA-Z0-9:/_-]+$`; `CreateRepositoryInput.kmsKeyId` documents the default selection: "If no key is specified, the default aws/codecommit Amazon Web Services managed key is used"). The model documents which KEY is used by default; what the member must CARRY is a key identifier. Recorded behavior: for a CreateRepository call that names no `kmsKeyId`, the response's `repositoryMetadata.kmsKeyId` is an account-specific KMS key identifier — the recording templates it as `{{kms_key}}` precisely because it is a per-account/per-run value — i.e. a key ARN of the form `arn:aws:kms:<region>:<account-id>:key/<key-uuid>` (or the bare key UUID), never the alias path. The default key's *selection* is fixed (`aws/codecommit`); the identifier it resolves to is a value of the account and region, so the requirement is the FORM (a resolved key ARN/UUID for the repository's own account and region), not any one recorded key id.

Emulator: the same step answers 200 with `$.repositoryMetadata.kmsKeyId` = `alias/aws/codecommit` — the alias selector itself, stored verbatim. `canonical_encryption_key` resolves only a bare key UUID (and `mrk-` ids) to `arn:aws:kms:<region>:<account>:key/<id>` and returns every other selector, including the default alias, unchanged; `Repository::new` seeds `kms_key_id` from `DEFAULT_ENCRYPTION_KEY_ALIAS` (`"alias/aws/codecommit"`) when the request names no key, and `Repository::metadata` projects that stored string straight into `kmsKeyId`. Because `metadata()` is the single projection, `GetRepository`, `BatchGetRepositories` and `UpdateRepositoryEncryptionKey` report the same alias path, and an alias passed in by a caller is likewise echoed rather than resolved.

Why: a client that reads `repositoryMetadata.kmsKeyId` to identify the CMK protecting a repository gets a selector, not an identity. Code that compares the value against a key id/ARN it manages (drift detection, a Config/CFN comparison, a tag-or-policy audit that joins repositories to `kms:DescribeKey`/`ListAliases` output by key id or ARN) never matches, and anything that parses the value as an ARN to recover the key's account, region or key id fails outright — against real CodeCommit all of these succeed. The alias also hides WHICH key is in force: two repositories on different keys that both resolve through an alias are indistinguishable.

Fix: the repository-metadata projection and the encryption-key canonicalization in the service's state — `canonical_encryption_key` must resolve every accepted KMS selector form (key id, key ARN, alias name, alias ARN) to the key ARN of the selected key in the repository's own account and region before it is stored/reported, with the default `aws/codecommit` managed key resolving to a stable managed-key ARN for the account and region, so `Repository::metadata` reports a key identifier for every repository. The acceptance rule in `is_kms_key_selector` stays as-is; only the stored/reported canonical form changes.

Traces: `codecommit-branch-and-commit-workflow#setup#2` (representative) and the same CreateRepository cluster in `codecommit-delete-repository-and-verify-gone#setup#1`, `codecommit-file-folder-blob-content-operations#setup#2`, `codecommit-put-file-parent-commit-errors#setup#1`, `codecommit-repo-binary-file-diff#setup#1`, `codecommit-repo-lifecycle-with-commit-and-branch#setup#1` (6 steps, one cause). Code: `services/service-com-amazonaws-codecommit-codecommit-20150413/src/state.rs:4434` (`DEFAULT_ENCRYPTION_KEY_ALIAS`), `:4444` (`canonical_encryption_key`), `:2056` (`Repository::new`), `:2255` (`Repository::metadata`).

No open CodeCommit issue covers this field: the nearest ones concern error-message wording (`20261029T222310Z-codecommit-git-object-and-commit-specifier-not-found.md`), merge/conflict validation (`20261029T222310Z-codecommit-branch-merge-and-conflict-detection-accept-unmergeable.md`) and CMK *disablement* enforcement (`20261022T213415Z-codecommit-putcommentreaction-is-a-501-stub-cmk-disablement.md`) — none of them is about the form of the reported `kmsKeyId`.

## Evidence
Replay of `codecommit-branch-and-commit-workflow`, step `setup#2` (CreateRepository), verdict `mismatch`, status 200 expected and 200 observed; the body diverges on one key:

  $.repositoryMetadata.kmsKeyId: value_mismatch (expected '{{kms_key}}', got 'alias/aws/codecommit')

Expected: a resolved KMS key identifier for the repository's account and region (`arn:aws:kms:<region>:<account-id>:key/<key-uuid>`, or a bare key UUID) — the recording holds a per-account value, which is why it is templated `{{kms_key}}`; that template, not its one recorded value, is the requirement.
Actual: the fixed alias selector string `alias/aws/codecommit`.

The cluster `service=codecommit|CreateRepository|status=200->200|keys=$.repositoryMetadata.kmsKeyId` covers 6 steps across 6 fixtures (every CreateRepository in the corpus that names no `kmsKeyId`): `codecommit-branch-and-commit-workflow#setup#2`, `codecommit-delete-repository-and-verify-gone#setup#1`, `codecommit-file-folder-blob-content-operations#setup#2`, `codecommit-put-file-parent-commit-errors#setup#1`, `codecommit-repo-binary-file-diff#setup#1`, `codecommit-repo-lifecycle-with-commit-and-branch#setup#1`. No status, error code or error message is involved — this is a success-path value divergence. Source of the alias: `state.rs:4434` `pub const DEFAULT_ENCRYPTION_KEY_ALIAS: &str = "alias/aws/codecommit";`, stored by `Repository::new` (`state.rs:2056`) through `canonical_encryption_key` (`state.rs:4444`), which returns any non-UUID selector unchanged, and projected verbatim by `Repository::metadata` (`state.rs:2255`).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Call `codecommit CreateRepository` with only `repositoryName=<any valid name>` (no `kmsKeyId`), e.g. `aws codecommit create-repository --repository-name demo-repo`.
3. Read `repositoryMetadata.kmsKeyId` in the 200 response: the emulator returns `alias/aws/codecommit`; real CodeCommit returns a KMS key identifier for the account/region (a `arn:aws:kms:<region>:<account-id>:key/<key-uuid>` ARN, or a bare key UUID).
4. Call `GetRepository` and `BatchGetRepositories` for the same repository — both project the same alias path, confirming the defect is in the stored canonical form rather than one handler.
5. Re-replay fixture `codecommit-branch-and-commit-workflow` step `setup#2` (and the five sibling CreateRepository steps listed in the evidence) to confirm the fix.
