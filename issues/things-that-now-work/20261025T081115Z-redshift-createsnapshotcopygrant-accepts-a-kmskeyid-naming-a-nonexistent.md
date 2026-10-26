Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected|_error#5|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Commit: 870f326825aa89e9aec352c0ac8db3065d945cf4

# Redshift CreateSnapshotCopyGrant accepts a KmsKeyId naming a nonexistent KMS key and creates the grant (HTTP 200) where AWS rejects with InvalidParameterValue

> _Note: this issue was identified against the code base at commit `870f326825aa89e9aec352c0ac8db3065d945cf4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshotCopyGrant

## Description
One root cause, in the redshift service.

Cause 1 of 1: CreateSnapshotCopyGrant performs no existence/accessibility resolution of KmsKeyId, so a grant is minted for a key that does not exist.

AWS: CreateSnapshotCopyGrant resolves the supplied KmsKeyId against KMS before creating anything. A KmsKeyId whose ARN names no real (or no accessible) key is refused with HTTP 400 and error code `InvalidParameterValue`, and NO grant is created — a subsequent DescribeSnapshotCopyGrants for the same SnapshotCopyGrantName fails with `SnapshotCopyGrantNotFoundFault` (HTTP 400, awsQueryError code `SnapshotCopyGrantNotFoundFault`). Smithy (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model): CreateSnapshotCopyGrant input members SnapshotCopyGrantName (required) and KmsKeyId (optional String); modeled errors are SnapshotCopyGrantAlreadyExistsFault, SnapshotCopyGrantQuotaExceededFault, LimitExceededFault, InvalidTagFault, TagLimitExceededFault, DependentServiceRequestThrottlingFault (all httpError 400) — the model does NOT model the KMS-resolution error, so its code `InvalidParameterValue` is recorded behavior only. DescribeSnapshotCopyGrants models SnapshotCopyGrantNotFoundFault (400). The recorded rejection message is built from the request — "Unable to create snapshot copy grant. The KMS key '<kms-key-arn>' does not exist or is inaccessible." — so the RULE (name the offending KmsKeyId value) is the requirement; that exact text is an example.

Emulator: the call SUCCEEDS with HTTP 200 and returns SnapshotCopyGrant {SnapshotCopyGrantName: <name>, KmsKeyId: <the bogus ARN echoed verbatim>, Tags: []} for a KmsKeyId of the form arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000, which resolves to no key. The grant is persisted under that name, so the check never reaches the follow-up DescribeSnapshotCopyGrants assertion.

Why: a client's negative tests for cross-region snapshot copy pass against the emulator and then fail against AWS. Worse, the emulator lets snapshot-copy configuration be built on a key reference that can never be used to encrypt a copied snapshot, and a caller can no longer distinguish "bad key" (InvalidParameterValue at create time) from "grant missing" (SnapshotCopyGrantNotFoundFault at describe time) — both branches of create-if-absent logic look identical.

Fix: KMS key resolution/validation in the CreateSnapshotCopyGrant handler (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_snapshot_copy_grant.rs) — resolve KmsKeyId against the kms service's key registry before writing the snapshot-copy-grant registry entry, and refuse with InvalidParameterValue (400) naming the supplied key when it does not resolve; the registry write must not happen on the refused path.

Related already-open issue: 20261024T024248Z-redshift-createcluster-skips-request-validation-and-publishes-an.md describes this check under its "Cause 7 of 7" while the operation was still a NotImplemented scaffold; the scaffold has since been implemented (see fixed issue 20261024T024248Z-redshift-createsnapshotcopygrant-stub.md) and the operation now answers 200, so the observed divergence and its replay signature are different from the one that issue records (it keys on both calls answering NotImplemented). Filing separately so the missing KmsKeyId validation is not closed with the NotImplemented family.

## Evidence
Check redshift.create-snapshot-copy-grant-invalid-kms-key-rejected [mismatch]:

  File "check_redshift_create_snapshot_copy_grant_invalid_kms_key_rejected.py", line 73, in run
    code, bad_message = _error(lambda: rs.create_snapshot_copy_grant(
        SnapshotCopyGrantName=bad_name, KmsKeyId=bogus_key_arn))
  File "check_redshift_create_snapshot_copy_grant_invalid_kms_key_rejected.py", line 51, in _error
    raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
  AssertionError: expected a rejection, but it succeeded: {'SnapshotCopyGrant': {'SnapshotCopyGrantName': 'sim2real-behav-rs-scg-badkey-<id>', 'KmsKeyId': 'arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000', 'Tags': []}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected (AWS): CreateSnapshotCopyGrant -> HTTP 400, Error.Code `InvalidParameterValue`, message naming the unresolvable KmsKeyId (example: "Unable to create snapshot copy grant. The KMS key '<arn>' does not exist or is inaccessible.").
Then DescribeSnapshotCopyGrants(SnapshotCopyGrantName=<same name>) -> HTTP 400, Error.Code `SnapshotCopyGrantNotFoundFault` (example message: "Snapshot copy grant '<name>' not found."), proving the refused create left nothing behind.

Observed (emulator): HTTP 200 with a created SnapshotCopyGrant echoing the bogus ARN; the grant exists under that name, so the absence-of-effect assertion is unreachable.

Generated names, account ids, region and request ids in the above are environment-specific examples; the requirements are the 400/`InvalidParameterValue` refusal and the absence of the grant afterwards.

## Steps to Reproduce
1. Boot the emulator; point redshift and kms clients at it.
2. Call redshift CreateSnapshotCopyGrant with SnapshotCopyGrantName=<fresh unique name> and KmsKeyId="arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000" (an ARN naming no key that exists in kms). Expect HTTP 400 with Error.Code `InvalidParameterValue`; observe HTTP 200 and a created SnapshotCopyGrant echoing that ARN.
3. Call redshift DescribeSnapshotCopyGrants(SnapshotCopyGrantName=<that same name>). Expect HTTP 400 `SnapshotCopyGrantNotFoundFault` because the create was refused; observe the grant listed (the refused create left state behind).
4. Positive control that must keep working: kms CreateKey, then CreateSnapshotCopyGrant with that key's Arn — expect success with KmsKeyId echoed verbatim as the full ARN, and DescribeSnapshotCopyGrants returning exactly that (name, KmsKeyId) pair.
5. Replay redshift.create-snapshot-copy-grant-invalid-kms-key-rejected to confirm.
