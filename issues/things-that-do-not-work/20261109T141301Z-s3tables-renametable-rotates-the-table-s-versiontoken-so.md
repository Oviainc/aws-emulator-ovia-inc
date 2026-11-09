Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.s3t-delete-table-with-a-stale-version-token-rejected
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.s3t-delete-table-with-a-stale-version-token-rejected|run#16|AssertionError|assert _token(renamed) == stale, (
Issue-Commit: e0dfe85212dfb0c22b04d41b86c8cbe201f3bba8

# s3tables RenameTable rotates the table's versionToken, so a control-plane rename invalidates a caller's Iceberg-metadata precondition token

> _Note: this issue was identified against the code base at commit `e0dfe85212dfb0c22b04d41b86c8cbe201f3bba8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RenameTable

## Description
Cause 1 of 1: RenameTable changes the table's versionToken. The version token is the optimistic-concurrency token over the table's Iceberg METADATA; a rename only moves the table's name, which is control-plane state outside that metadata, so the token must be unchanged after a rename.

AWS: Smithy models versionToken on GetTable output as required (VersionToken, length 1..2048) and as the optional precondition input of RenameTable, DeleteTable (httpQuery versionToken) and the other mutators, documented only as "The version token of the table." — the model does not state what rotates it, so this is recorded behavior: the token returned by CreateTable stays identical across GetTable re-reads, across PutTableMaintenanceConfiguration, across RenameTable and across TagResource, and rotates only when the table's Iceberg metadata is committed (e.g. an Iceberg REST add-sort-order / set-default-sort-order commit). A DeleteTable carrying a token that no longer matches is then refused with HTTP 409 ConflictException and the message "Provided version token does not match the table version token." (fixed text, no variable part).
Emulator: the rename rotates the token — immediately after rename_table(newName=...), GetTable reports a different versionToken than the one CreateTable returned (a freshly minted UUID), while the earlier GetTable re-read and the maintenance put had left it unchanged. The check fails at `assert _token(renamed) == stale` with "a rename rotated the token to '<new-token>'; the name is control-plane state and lives outside the Iceberg metadata the token covers".
Why: a client that holds a token for a table and renames it (or has it renamed) loses its precondition for no reason: its next conditional DeleteTable/RenameTable/Put is refused 409 ConflictException even though the Iceberg metadata it asserted against never changed, and conversely the token stops being a signal that the metadata changed — so the whole stale-writer protection the token exists for becomes unreliable. It also hides a real metadata commit behind a rotation that a rename already caused.
Fix: the RenameTable handler and the table state it writes — the rename must update only the table's name (and modifiedAt/modifiedBy), leaving versionToken as it was; token rotation belongs to the Iceberg-metadata commit path only.
Traces: s3tables.s3t-delete-table-with-a-stale-version-token-rejected. Related but distinct open issues: 20261101T230800Z-s3tables-renametable-does-not-move-the-table-record.md (the rename not reaching stored state) and 20261106T132944Z-s3tables-renametable-stub.md (RenameTable unimplemented) — neither covers token rotation, and the rename here does take effect.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.
[mismatch] s3tables.s3t-delete-table-with-a-stale-version-token-rejected, line 180: `assert _token(renamed) == stale` -> AssertionError: "a rename rotated the token to '6d4c5d50-7bba-4b52-89dd-77d2f67e333d'; the name is control-plane state and lives outside the Iceberg metadata the token covers".
Sequence up to the failure, all of it passing: CreateTable returned versionToken T; GetTable re-read reported T; put_table_maintenance_configuration(type="icebergCompaction", value={"status":"disabled"}) left it at T; rename_table(tableBucketARN=<arn>, namespace=<ns>, name=<old>, newName=<new>) then made GetTable report a token != T.
Expected: GetTable(name=<new name>).versionToken == T after the rename (and after a subsequent TagResource), rotating only on the later Iceberg REST metadata commit; the concrete token value in this run is a per-run identifier and is not part of the requirement. The checks that depend on it — the stale-token DeleteTable being refused 409 ConflictException "Provided version token does not match the table version token." and the current-token DeleteTable succeeding — were never reached.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. s3tables.create_table_bucket, create_namespace, then create_table(..., format="ICEBERG", metadata=<iceberg schema>) and keep the returned versionToken T.
3. get_table(...) — versionToken is T.
4. put_table_maintenance_configuration(type="icebergCompaction", value={"status": "disabled"}) and get_table again — still T.
5. rename_table(tableBucketARN=<arn>, namespace=<ns>, name=<old>, newName=<new>), then get_table(tableBucketARN=<arn>, namespace=<ns>, name=<new>). Expect versionToken == T; the emulator returns a newly minted token.
