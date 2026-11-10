Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-rename-table-workflow
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-rename-table-workflow s3tables-error-operations-on-missing-resources
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|RenameTable|2xx->4xx|status
Issue-Key: s3tables|RenameTable|4xx->4xx|$.message
Issue-Commit: 41bd6c476a5fe40a474aadc7e47c434277d677d7

# s3tables RenameTable diverges: destination-namespace refusal text and a spurious 409 on a follow-up rename

> _Note: this issue was identified against the code base at commit `41bd6c476a5fe40a474aadc7e47c434277d677d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RenameTable

## Description
Two net-new causes in the RenameTable handler, sharing one fault: the handler's pre-move checks diverge from AWS — it refuses a rename AWS accepts, and when it does refuse for a missing destination namespace it states the refusal with the wrong fixed sentence. Both must be closed before this issue is submitted.

Cause 1 of 2: a rename that AWS accepts is refused 409 ConflictException (fixture s3tables-rename-table-workflow, step task#8) — the severe one.
AWS: RenameTable is modeled PUT /tables/{tableBucketARN}/{namespace}/{name}/rename with output Unit and @http code 204 (Smithy: RenameTable, input members tableBucketARN/namespace/name required @httpLabel, newNamespaceName/newName/versionToken optional; errors BadRequestException 400, ConflictException 409, NotFoundException 404). The recording for this step is HTTP 204 with an empty body: AWS performed the move and returned no content. ConflictException is documented as "The request failed because there is a conflict with a previous write. You can retry the request." — AWS raised no such conflict for this request.
Emulator: answers 409 with a ConflictException, so the table is never moved; the same fixture's following steps then read and delete the old coordinates (GetTable 404 at task#9, DeleteTable 404 at teardown#10, DeleteNamespace 409 at teardown#11, DeleteTableBucket 400 at teardown#13 — those four already belong to open issue 20261101T230800Z-s3tables-renametable-does-not-move-the-table-record.md and are not re-filed here). Only two paths in the handler can emit this 409 (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/rename_table.rs): the destination-occupied check at line 270 (state::TABLE_ALREADY_EXISTS_MESSAGE) and the versionToken precondition at lines 279-287 (state::TABLE_VERSION_TOKEN_MISMATCH_MESSAGE). Hypothesis: it is the versionToken precondition — the handler mints a fresh version token on every rename (rename_table.rs line 302, `moved.version_token = store.mint_version_token()`), so a versionToken the caller obtained before an earlier rename of the same table no longer matches and a later conditional rename is refused; a rename only moves control-plane coordinates and must not rotate the token that guards the table's Iceberg metadata. Open issue 20261109T141301Z-s3tables-renametable-rotates-the-table-s-versiontoken-so.md describes that rotation from the behavioral suite but carries no wire replay key for this step, so this wire cause is net-new and is not closed by it. The requirement regardless of mechanism: a rename whose source table, destination namespace and (when presented) current version token are all valid answers 204 with an empty body and re-keys the record.
Why: a client's rename workflow fails at the rename and every later call addresses coordinates that do not exist — the table is readable only under its old name, the namespace and bucket can no longer be deleted, and because 409 is AWS's retryable write-conflict the client retries a rename that will never succeed.
Fix: the RenameTable handler's pre-move checks and the table state it writes — do not rotate versionToken on a move (token rotation belongs to the Iceberg-metadata commit path), and do not treat the resolved destination of a valid move as occupied.
Traces: s3tables-rename-table-workflow#task#8.

Cause 2 of 2: the missing-destination-namespace refusal uses the source-namespace sentence (fixture s3tables-error-operations-on-missing-resources, step task#5).
AWS: 404 NotFoundException (status matches; the recording asserts no x-amzn-errortype here) with message exactly "The specified destination namespace does not exist." — recorded behavior only: the Smithy model documents newNamespaceName merely as "The new name for the namespace." and classifies the failure as NotFoundException(404) without stating the text. The whole sentence is FIXED text: AWS names neither the namespace nor the bucket, so there is no variable part taken from the request or from state.
Emulator: 404 with message "The specified namespace does not exist." — the same sentence it uses when the SOURCE namespace is absent. In rename_table.rs the destination lookup at lines 252-255 raises not_found(state::NAMESPACE_NOT_FOUND_MESSAGE), the identical constant used for the source lookup at line 227.
Why: NotFoundException carries no code that distinguishes the two cases, so the message is the only discriminator. A client that renames a table into a namespace it must create first cannot tell "your table's namespace is gone" from "the namespace you are moving into does not exist", and error-matching on the documented sentence does not fire.
Fix: the RenameTable handler's destination-namespace lookup — its own fixed destination sentence, distinct from the source-namespace constant in the service's message constants.
Traces: s3tables-error-operations-on-missing-resources#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against this checkout (emulator up, /__admin/health 200; 60 fixtures, 572 steps, score 78.5%). The two clusters below are the only ones reported with no owning issue and no blocking stub.
- [mismatch] RenameTable, fixture s3tables-rename-table-workflow step task#8: status expected 204, got 409 (divergence_key service=s3tables|RenameTable|status=204->409|keys=status). No other member diverged — the whole response is the refusal. Downstream in the same fixture (already owned by 20261101T230800Z-s3tables-renametable-does-not-move-the-table-record.md): GetTable task#9 expected 200 got 404, DeleteTable teardown#10 expected 204 got 404, DeleteNamespace teardown#11 expected 204 got 409, DeleteTableBucket teardown#13 expected 204 got 400.
- [mismatch] RenameTable, fixture s3tables-error-operations-on-missing-resources step task#5: status 404 as expected; $.message value_mismatch — expected 'The specified destination namespace does not exist.', got 'The specified namespace does not exist.' (divergence_key service=s3tables|RenameTable|status=404->404|keys=$.message). Both sentences are entirely fixed text; no request or state value appears in either, so the divergence is the wording and not a per-run value.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Cause 1 — fixture s3tables-rename-table-workflow, step task#8: run the fixture's setup (CreateTableBucket, CreateNamespace, CreateTable), its reads (GetTable at task#6) and its earlier rename, then issue the task#8 PUT /tables/{tableBucketARN}/{namespace}/{name}/rename. Expect HTTP 204 with an empty body; the emulator answers 409 ConflictException and the table stays at its old coordinates (visible as the 404/409/400 on task#9, teardown#10, teardown#11, teardown#13).
3. Cause 2 — fixture s3tables-error-operations-on-missing-resources, step task#5: RenameTable an existing table with newNamespaceName set to a namespace that does not exist in the bucket. Expect 404 NotFoundException with message exactly 'The specified destination namespace does not exist.'; the emulator returns 'The specified namespace does not exist.'.
