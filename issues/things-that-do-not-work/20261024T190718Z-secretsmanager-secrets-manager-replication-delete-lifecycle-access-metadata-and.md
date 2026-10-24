Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.cross-region-replica-removed-by-remove-regions-and-promoted-by-stop-replication secretsmanager.live-replica-makes-the-primary-undeletable-and-the-replica-unwritable secretsmanager.e2e-replica-secret-propagation-and-teardown secretsmanager.replicate-secret-refuses-a-name-collision-until-force-overwrite secretsmanager.replica-under-a-missing-kms-key-reports-failed-and-is-recoverable-by-remove-then-replicate secretsmanager.replication-lifecycle secretsmanager.e2e-replica-promotion-after-stop-replication secretsmanager.delete-secret-recovery-window-sets-deletion-date-offset secretsmanager.delete-secret-recovery-window-sets-exact-deletion-date secretsmanager.delete-secret-disables-rotation-and-restore-does-not-reenable-it secretsmanager.force-delete-of-a-missing-secret-succeeds-and-creates-nothing secretsmanager.get-secret-value-sets-last-accessed-date-to-the-day-without-changing-last-changed-date secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively secretsmanager.list-secrets-negated-filter-values-exclude-matches secretsmanager.create-secret-empty-string-description-preserved-vs-absent secretsmanager.description-empty-vs-absent-distinct
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.cross-region-replica-removed-by-remove-regions-and-promoted-by-stop-replication|run#14|AssertionError|assert replica_v1["ARN"] != primary_arn and f":{replica_region}:" in replica_v1["ARN"], (
Issue-Key: secretsmanager.live-replica-makes-the-primary-undeletable-and-the-replica-unwritable|run#17|AssertionError|assert f":{replica_region}:" in seen["ARN"], (
Issue-Key: secretsmanager.e2e-replica-secret-propagation-and-teardown|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: secrets-manager|CreateSecret|ResourceExistsException
Issue-Key: secretsmanager.replica-under-a-missing-kms-key-reports-failed-and-is-recoverable-by-remove-then-replicate|run#14|AssertionError|assert failed.get("Status") == "Failed", (
Issue-Key: secretsmanager.replication-lifecycle|run#15|AssertionError|assert replica[0].get("StatusMessage") == "Replication succeeded", (
Issue-Key: secrets-manager|StopReplicationToReplica|NotImplemented
Issue-Key: secretsmanager.delete-secret-recovery-window-sets-deletion-date-offset|run#20|AssertionError|assert abs((deleted_date - before).total_seconds()) < 300, (
Issue-Key: secretsmanager.delete-secret-recovery-window-sets-exact-deletion-date|run#17|AssertionError|assert -_TOLERANCE_SECONDS < request_window_delta < (after - before).total_seconds() + _TOLERANCE_SECONDS, (
Issue-Key: secretsmanager.delete-secret-disables-rotation-and-restore-does-not-reenable-it|run#20|AssertionError|assert deleted.get("RotationEnabled") is False, (
Issue-Key: secretsmanager.force-delete-of-a-missing-secret-succeeds-and-creates-nothing|run#4|KeyError|assert resp["Name"] == missing, f"forced delete #{attempt} answered Name {resp['Name']!r}, expected {missing!r}"
Issue-Key: secretsmanager.get-secret-value-sets-last-accessed-date-to-the-day-without-changing-last-changed-date|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: secretsmanager.list-secrets-negated-filter-values-exclude-matches|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: secretsmanager.create-secret-empty-string-description-preserved-vs-absent|run#16|AssertionError|assert "Description" not in desc_empty, (
Issue-Key: secretsmanager.description-empty-vs-absent-distinct|run#12|AssertionError|assert desc_empty.get("Description") == desc_absent.get("Description"), (
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Secrets Manager replication, delete lifecycle, access metadata and ListSecrets filters diverge (9 causes)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReplicateSecretToRegions

## Description
Nine root causes. Cause count: 9.

=== Cause 1 of 9: a replica is not a distinct regional secret ===
AWS: ReplicateSecretToRegions creates a SEPARATE secret in each added region: it has its own region-qualified ARN (arn:aws:secretsmanager:<replica-region>:...), it is readable with a client pinned to that region, it is NOT writable there (writes go to the primary), the primary cannot be deleted while a live replica exists, and the name space is per region (Smithy: ReplicateSecretToRegions -> ARN + ReplicationStatus; DescribeSecret exposes PrimaryRegion + ReplicationStatus).
Emulator: replication is bookkeeping over a single global store. The replica ARN handed back IS the primary's ARN in the primary's region, a region-pinned client reads the primary back rather than a replica, and CreateSecret of the same name in a second region collides with ResourceExistsException ('The operation failed because the secret <name> already exists'). The propagation/teardown check polls ReplicationStatus and the replica-region read and runs to the 60 s cap.
Why: multi-region secret topologies are built on the replica being its own regional resource - region-local reads, per-region ARNs in policies, failover. A client gets an ARN it cannot distinguish from the primary's, and a name that is unexpectedly taken in every other region.
Fix: the replication state (per-region secret records keyed by region+name, region-qualified ARNs) and the ReplicateSecretToRegions / CreateSecret(AddReplicaRegions) handlers, plus the region scoping of the secret store.
Traces: secretsmanager.cross-region-replica-removed-by-remove-regions-and-promoted-by-stop-replication, secretsmanager.live-replica-makes-the-primary-undeletable-and-the-replica-unwritable, secretsmanager.e2e-replica-secret-propagation-and-teardown, secretsmanager.replicate-secret-refuses-a-name-collision-until-force-overwrite.

=== Cause 2 of 9: ReplicationStatus is always InSync and carries no StatusMessage ===
AWS: a replica whose target KMS key does not exist settles to Status=Failed with a StatusMessage explaining why, and a healthy replica reports a StatusMessage alongside InSync; the pair is how a client decides whether the replica is usable and recovers it (RemoveRegionsFromReplication then ReplicateSecretToRegions again).
Emulator: the replica is reported {'Region': 'us-east-1', 'KmsKeyId': 'alias/...', 'Status': 'InSync'} even when the named key does not exist, and an InSync replica reports StatusMessage None.
Why: a client cannot detect a broken replica, so a failover reads a secret that was never actually replicated.
Fix: the replication status state machine - validate the per-region KmsKeyId, settle Failed with a StatusMessage, and populate StatusMessage on the healthy path.
Traces: secretsmanager.replica-under-a-missing-kms-key-reports-failed-and-is-recoverable-by-remove-then-replicate, secretsmanager.replication-lifecycle.

=== Cause 3 of 9: StopReplicationToReplica is an unimplemented stub ===
AWS: StopReplicationToReplica, called in the replica's region, promotes the replica to a standalone secret that no longer tracks the primary.
Emulator: 400 NotImplemented - 'operation StopReplicationToReplica promoting a replica to a secret of its own is scaffolded but not implemented in this emulator'.
Why: promotion is the regional-failover step; a client cannot detach a replica at all.
Fix: the StopReplicationToReplica handler plus the per-region replication state from cause 1.
Traces: secretsmanager.e2e-replica-promotion-after-stop-replication.

=== Cause 4 of 9: DeletedDate is set to the future deletion date instead of the deletion-request time ===
AWS: Smithy DescribeSecret returns both DeletedDate and the window-derived date; DeleteSecret's response carries DeletionDate. DeletedDate is when the delete was REQUESTED (now); the permanent-deletion instant (now + RecoveryWindowInDays) is DeletionDate.
Emulator: DescribeSecret.DeletedDate holds the future instant - the replay shows DeletedDate 2026-11-03T18:52 for a delete requested at 2026-10-24T18:52 (default 30/10-day offset) and 2026-10-31T18:51 for a 7-day window requested at 2026-10-24T18:51.
Why: audit and 'how long have I got left' tooling reads DeletedDate as the request time; reporting the future date makes a secret look deleted days before it was, and the recovery window cannot be computed.
Fix: the DeleteSecret handler's recovery-window state - stamp DeletedDate with the request time and expose the scheduled instant as the deletion date.
Traces: secretsmanager.delete-secret-recovery-window-sets-deletion-date-offset, secretsmanager.delete-secret-recovery-window-sets-exact-deletion-date.

=== Cause 5 of 9: a recoverable DeleteSecret does not disable rotation ===
AWS: scheduling a secret for deletion turns rotation off - DescribeSecret reports RotationEnabled=false (Smithy DescribeSecret.RotationEnabled) - and RestoreSecret does not turn it back on.
Emulator: after DeleteSecret with a recovery window, DescribeSecret still reports RotationEnabled=True.
Why: a client reading RotationEnabled believes a scheduled-for-deletion secret is still being rotated, and restore semantics ('rotation must be re-enabled explicitly') are inverted.
Fix: the DeleteSecret handler's state transition (clear RotationEnabled, keep it cleared across RestoreSecret).
Traces: secretsmanager.delete-secret-disables-rotation-and-restore-does-not-reenable-it.

=== Cause 6 of 9: force-deleting a name that does not exist answers a response without Name/ARN/DeletionDate ===
AWS: DeleteSecret(ForceDeleteWithoutRecovery=true) on a never-created name succeeds idempotently with 200 and a full body - Name, an ARN of the shape secret:<name>-XXXXXX in the caller's region/account, and DeletionDate (Smithy DeleteSecretResponse: ARN, Name, DeletionDate) - and leaves no tombstone; the same call WITHOUT the force flag is ResourceNotFoundException.
Emulator: the response omits Name, so a client reading resp['Name'] raises KeyError.
Why: teardown code force-deletes secrets it may never have created and reads the echoed Name/ARN; an incomplete body breaks every such cleanup path.
Fix: the DeleteSecret handler's force-delete-of-absent-secret path and its response serializer.
Traces: secretsmanager.force-delete-of-a-missing-secret-succeeds-and-creates-nothing.

=== Cause 7 of 9: GetSecretValue never sets LastAccessedDate ===
AWS: a data-plane read stamps DescribeSecret.LastAccessedDate with the current UTC day truncated to midnight, metadata reads (DescribeSecret, ListSecretVersionIds) do not, and the read leaves LastChangedDate untouched (Smithy DescribeSecret.LastAccessedDate).
Emulator: LastAccessedDate never appears; the check polls DescribeSecret for it and runs to the 60 s cap.
Why: unused-secret audits ('not accessed in 90 days') are built entirely on LastAccessedDate; a secret that is read daily looks never-accessed.
Fix: the GetSecretValue handler's access stamp (day-truncated UTC) in the secret's metadata state.
Traces: secretsmanager.get-secret-value-sets-last-accessed-date-to-the-day-without-changing-last-changed-date.

=== Cause 8 of 9: ListSecrets Filters do not select the documented sets ===
AWS: the description filter matches a case-insensitive prefix of any word of the description; all searches name words, description, tag keys and tag values case-insensitively and ANDs the words of one value; name stays case-sensitive; and a value prefixed with '!' excludes matches.
Emulator: the filtered queries never converge on the expected sets - both filter checks poll ListSecrets and run to the 60 s cap.
Why: the description/all filters are the search box of every secrets console and inventory script; an emulator that answers a different set makes those scripts silently miss or over-select secrets.
Fix: the ListSecrets handler's filter evaluation (per-key matching rules, case folding, word ANDing and negation).
Traces: secretsmanager.list-secrets-description-and-all-filters-match-words-case-insensitively, secretsmanager.list-secrets-negated-filter-values-exclude-matches.

=== Cause 9 of 9: an explicitly empty Description is stored instead of being dropped ===
AWS: CreateSecret with Description='' is indistinguishable on read-back from a create that omitted Description - DescribeSecret carries no Description key in either case (Smithy CreateSecret.Description length min 0, so '' is accepted on the wire but not echoed).
Emulator: the empty string surfaces as Description='' while the omitted case is absent, so the two creates are distinguishable.
Why: clients diff DescribeSecret output to decide whether an update is needed; a phantom empty Description produces a spurious change on every reconcile.
Fix: the CreateSecret/DescribeSecret representation of an empty Description.
Traces: secretsmanager.create-secret-empty-string-description-preserved-vs-absent, secretsmanager.description-empty-vs-absent-distinct.

## Evidence
Cause 1: 'the replica must be a distinct, region-qualified ARN, not the primary\'s: arn:aws:secretsmanager:us-west-2:...-6PVuld (primary arn:aws:secretsmanager:us-west-2:...-6PVuld)'; 'the replica must be a region-qualified secret in us-east-1, not the primary read back; its ARN is arn:aws:secretsmanager:us-west-2:...'; ResourceExistsException on CreateSecret for the second-region name; e2e-replica-secret-propagation-and-teardown 'Check timed out after 60.0s'.
Cause 2: 'a replica under a nonexistent key must settle to Failed; got {Region: us-east-1, KmsKeyId: alias/..., Status: InSync}'; 'an InSync replica reported StatusMessage None'.
Cause 3: ClientError (NotImplemented) calling StopReplicationToReplica.
Cause 4: 'DescribeSecret DeletedDate 2026-11-03 18:52:19 is not close to the delete-request time 2026-10-24 18:52:19'; 'DescribeSecret DeletedDate (2026-10-31 18:51:12) is not close to the deletion request time (2026-10-24 18:51:12 ..)'.
Cause 5: 'a recoverable DeleteSecret must disable rotation; got RotationEnabled=True'.
Cause 6: KeyError: 'Name' at resp['Name'] after delete_secret(SecretId=<never created>, ForceDeleteWithoutRecovery=True).
Cause 7 and 8: 'Check timed out after 60.0s' while polling DescribeSecret for LastAccessedDate / ListSecrets for the filtered set.
Cause 9: 'explicit empty-string Description unexpectedly surfaced as a key in DescribeSecret (got \'\')' and 'expected Description=\'\' and omitted Description to be indistinguishable on read-back, got \'\' vs None'.

## Steps to Reproduce
Cause 1-3: CreateSecret in us-west-2; ReplicateSecretToRegions(AddReplicaRegions=[{Region:us-east-1}]); compare the replica ARN with the primary's; GetSecretValue/PutSecretValue with a client pinned to us-east-1; DeleteSecret the primary while the replica is live; CreateSecret the same name in us-east-1; ReplicateSecretToRegions with KmsKeyId=alias/<nonexistent> and poll ReplicationStatus; StopReplicationToReplica in the replica region.
Cause 4-6: DeleteSecret(RecoveryWindowInDays=7) then DescribeSecret and compare DeletedDate with the request time; DescribeSecret.RotationEnabled after a recoverable delete of a rotating secret; DeleteSecret(SecretId=<never created>, ForceDeleteWithoutRecovery=True) and read Name/ARN/DeletionDate.
Cause 7: CreateSecret; DescribeSecret (no LastAccessedDate); GetSecretValue; poll DescribeSecret for LastAccessedDate.
Cause 8: create three secrets with descriptions and tags, then ListSecrets with Filters [{Key:description,Values:[payment]}], [{Key:all,Values:[GATEWAY]}], [{Key:all,Values:[payment login]}] and a negated value.
Cause 9: CreateSecret(Description='') vs CreateSecret with no Description, then DescribeSecret both.
