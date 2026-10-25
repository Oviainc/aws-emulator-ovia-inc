Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-basic-secret-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-basic-secret-lifecycle secretsmanager-secret-version-staging-workflow secretsmanager-list-pagination-invalid-next-token secretsmanager-version-stage-move-conflicts
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|ListSecretVersionIds|2xx->2xx|$.Versions[*]
Issue-Key: secretsmanager|GetSecretValue|2xx->2xx|$.SecretString
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|$.ARN
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|$.CreatedDate
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|$.Name
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|$.SecretString
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|$.VersionId
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|$.VersionStages
Issue-Key: secretsmanager|GetSecretValue|2xx->4xx|status
Issue-Key: secretsmanager|ListSecretVersionIds|2xx->2xx|$.Versions[*].KmsKeyIds
Issue-Key: secretsmanager|GetSecretValue|2xx->2xx|$.VersionId
Issue-Key: secretsmanager|GetSecretValue|2xx->2xx|$.VersionStages[*]
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# Secrets Manager version history is not tracked: staging labels, prior versions and version metadata are lost on every write

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecretVersionIds

## Description
The emulator stores a single, overwritten value per secret instead of a version chain with staging labels. Five distinct causes, all rooted in the missing version store, must each be closed.

Cause 1 of 5: ListSecretVersionIds does not retain prior versions or move staging labels.
AWS: Smithy ListSecretVersionIds -> ListSecretVersionIdsResponse.Versions : SecretVersionsListType of SecretVersionsListEntry. After a second write to a secret, the secret has two versions: the superseded one labelled AWSPREVIOUS and the new one labelled AWSCURRENT, and both appear in Versions. Recorded response (200) shows exactly that: one entry with VersionStages ['AWSPREVIOUS'] and one with VersionStages ['AWSCURRENT'].
Emulator: returns a single entry, still labelled ['AWSCURRENT'], and no second entry at all (Versions[1] is absent).
Why: a client that enumerates versions to roll back, audit, or find AWSPREVIOUS sees a secret that apparently has never been updated; rollback by VersionId is impossible because the prior VersionId no longer exists.
Fix: the service's secret-version state (append a new version and relabel the old one on write) and the ListSecretVersionIds handler.

Cause 2 of 5: SecretVersionsListEntry omits modeled metadata members.
AWS: Smithy SecretVersionsListEntry members VersionId, VersionStages, CreatedDate, LastAccessedDate, KmsKeyIds (KmsKeyIdListType, 'The KMS keys used to encrypt the secret version'). Recorded entries carry KmsKeyIds and LastAccessedDate alongside VersionId/VersionStages/CreatedDate; for a secret using the account default key KmsKeyIds is populated (recorded example value: the default encryption key alias).
Emulator: emits only VersionId, VersionStages and CreatedDate; KmsKeyIds and LastAccessedDate are absent on every entry.
Why: clients that read the encrypting key or last-access date per version get null where AWS always populates them, so key-rotation and staleness reporting break.
Fix: the ListSecretVersionIds serializer plus the version record in state (persist key id and last-accessed per version).

Cause 3 of 5: GetSecretValue returns the superseded value after a newer version is written.
AWS: Smithy GetSecretValue with no VersionId/VersionStage resolves AWSCURRENT, i.e. the most recently staged version. The recording expects the post-write value (the fixture does not assert the plaintext itself, only that it is the current version's value), status 200.
Emulator: status 200 but SecretString is still the value written by the first PutSecretValue/CreateSecret ('initial-value' in the recording), i.e. AWSCURRENT was never advanced to the new version.
Why: this is the core read-your-write contract of the service. A client that writes a rotated credential and reads it back receives the old credential and authenticates with a stale secret.
Fix: the PutSecretValue/UpdateSecret handlers (stage the new version as AWSCURRENT) and the GetSecretValue stage resolution.

Cause 4 of 5: GetSecretValue fails outright for a version created by a later write.
AWS: Smithy GetSecretValue errors are DecryptionFailure, InternalServiceError, InvalidParameterException, InvalidRequestException, ResourceNotFoundException; a request naming an existing, current version succeeds with 200 and ARN, Name, VersionId, SecretString, VersionStages ['AWSCURRENT'] and CreatedDate.
Emulator: returns 400 with no body members at all, so the newly created version is not resolvable by id or stage.
Why: the client's error-handling path fires for a version that exists; the whole post-rotation read is unavailable rather than merely stale.
Fix: GetSecretValue version/stage lookup against the version store.

Cause 5 of 5: staging-label moves (UpdateSecretVersionStage) are not reflected in GetSecretValue.
AWS: a version may carry several staging labels; after moving AWSPENDING onto a version, GetSecretValue for that version returns it with VersionStages containing both its existing label and AWSPENDING, and the VersionId of the version the labels were moved to.
Emulator: status 200 but returns the wrong version (a different VersionId) and VersionStages carries only one label - the second, moved label (AWSPENDING in the recording) is absent.
Why: rotation clients drive the AWSCURRENT/AWSPENDING handshake entirely through staging labels; with labels not moving, a rotation function can never promote or verify a pending version.
Fix: the UpdateSecretVersionStage handler and the shared staging-label map in state.

Traces: secretsmanager-basic-secret-lifecycle#task#6 (cause 1), secretsmanager-list-pagination-invalid-next-token#task#4 (cause 2), secretsmanager-basic-secret-lifecycle#task#5 (cause 3, also affects secretsmanager-secret-version-staging-workflow#task#7), secretsmanager-basic-secret-lifecycle#task#10 (cause 4), secretsmanager-version-stage-move-conflicts#task#6 (cause 5). Cause 1 also affects secretsmanager-secret-version-staging-workflow#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, state_reset_failures=0).

Cause 1 - ListSecretVersionIds, secretsmanager-basic-secret-lifecycle#task#6, expected 200 / got 200: $.Versions[0]: missing (expected VersionStages ['AWSPREVIOUS'] with CreatedDate, KmsKeyIds ['DefaultEncryptionKey'], LastAccessedDate; got VersionStages ['AWSCURRENT'] with VersionId+CreatedDate only); $.Versions[1]: missing (expected the new AWSCURRENT version, got None). affected_count=2 (also secretsmanager-secret-version-staging-workflow#task#5).

Cause 2 - ListSecretVersionIds, secretsmanager-list-pagination-invalid-next-token#task#4, 200/200: $.Versions[0].KmsKeyIds: missing (expected ['DefaultEncryptionKey'], got None).

Cause 3 - GetSecretValue, secretsmanager-basic-secret-lifecycle#task#5, 200/200: $.SecretString: value_mismatch (expected the current version's value, got 'initial-value'). affected_count=2 (also secretsmanager-secret-version-staging-workflow#task#7).

Cause 4 - GetSecretValue, secretsmanager-basic-secret-lifecycle#task#10: status expected 200, got 400; $.ARN, $.CreatedDate, $.Name, $.SecretString, $.VersionId, $.VersionStages all missing (got None).

Cause 5 - GetSecretValue, secretsmanager-version-stage-move-conflicts#task#6, 200/200: $.VersionId: value_mismatch (expected the version the labels were moved to, got a different version id); $.VersionStages[1]: missing (expected 'AWSPENDING', got None); $.SecretString: value_mismatch (got 'current-content').

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus().
2. Causes 1/3/4: fixture secretsmanager-basic-secret-lifecycle - CreateSecret, PutSecretValue a second value, then step task#5 (GetSecretValue), task#6 (ListSecretVersionIds) and task#10 (GetSecretValue for the new version).
3. Cause 2: fixture secretsmanager-list-pagination-invalid-next-token step task#4 - ListSecretVersionIds and inspect Versions[0].KmsKeyIds.
4. Cause 5: fixture secretsmanager-version-stage-move-conflicts step task#6 - move a staging label with UpdateSecretVersionStage, then GetSecretValue and inspect VersionId and VersionStages.
