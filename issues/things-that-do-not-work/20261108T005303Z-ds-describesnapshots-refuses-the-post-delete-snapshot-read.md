Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.snapshot-create-delete-lifecycle ds.concurrent-add-ip-routes-conflict-detected ds.concurrent-add-ip-routes-duplicate-conflict
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: directory-service|DescribeSnapshots|InvalidParameterException
Issue-Key: ds.concurrent-add-ip-routes-conflict-detected|run#14|AssertionError|assert loser.get("Message") == expected_message, (
Issue-Key: ds.concurrent-add-ip-routes-duplicate-conflict|run#14|AssertionError|assert any(p in loser_message.lower() for p in _busy_phrases), (
Issue-Commit: 09ede02538826dd718336321255b799d2c318d86

# ds: DescribeSnapshots refuses the post-delete snapshot read and AddIpRoutes takes no directory-update lock (2 causes, both reopened)

> _Note: this issue was identified against the code base at commit `09ede02538826dd718336321255b799d2c318d86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
REOPENS 20261104T062008Z-ds-ten-refusal-paths-answer-the-wrong-error.md: its fix left the call refused.

That report's cause 1 (AddIpRoutes duplicate-CIDR class/message) and cause 8 (DescribeSnapshots classification of an unresolvable snapshot id) were closed, and both refusal sites now answer the new wording — but the calls the checks below make are still refused, on the same two operations, so neither path can be walked end to end. TWO causes, both in service ds; a fixer must close BOTH before submitting.

Cause 1 of 2: DescribeSnapshots refuses a directory-scoped read that names a snapshot the caller has just deleted, so the snapshot lifecycle can never be completed.
AWS: Smithy — DescribeSnapshots(DirectoryId ^d-[0-9a-f]{10}$, SnapshotIds list of SnapshotId ^s-[0-9a-f]{10}$) models ClientException, EntityDoesNotExistException, InvalidNextTokenException, InvalidParameterException (all client, HTTP 400). Recorded behavior for the lifecycle: CreateSnapshot answers 200 with a SnapshotId; DescribeSnapshots(DirectoryId=<dir>, SnapshotIds=[<that id>]) answers 200 and reports the snapshot until it is Completed; after DeleteSnapshot the same directory-scoped read settles either as HTTP 200 with an empty Snapshots list or as 400 EntityDoesNotExistException — it is NEVER InvalidParameterException; and a second DeleteSnapshot of the id is 400 EntityDoesNotExistException with the message 'Snapshot: <snapshot id> does not exist' (fixed text, the only variable part being the snapshot id taken from the request).
Emulator: every SnapshotIds entry the store cannot resolve is refused 400 InvalidParameterException 'Invalid snapshot ID: <snapshot id>.' (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/describe_snapshots.rs, the SnapshotIds resolution loop at lines 236-244 calling crate::state::invalid_snapshot_id). The lifecycle check therefore dies with an unhandled botocore.errorfactory.InvalidParameterException: 'Invalid snapshot ID: s-003ba5fe5a. : RequestId: <id>' on a DescribeSnapshots of the snapshot it just created and deleted, and never reaches the post-delete assertions (empty inventory, and the re-delete not-found message).
Why: a client cannot complete the documented snapshot workflow — poll a snapshot to Completed, delete it, confirm it is gone — because the confirming read is a hard parameter error instead of an empty page or the entity-not-found class the recording shows; code that treats InvalidParameterException as 'you sent a malformed id' aborts a workflow in which nothing was malformed.
Hypothesis: AWS's parameter error for a snapshot id applies more narrowly than the emulator's blanket 'any id the store cannot resolve'. ds.get-snapshot-limits-reflects-snapshot-count (now passing) pins InvalidParameterException for a deleted snapshot id, so the distinguishing factor is most likely the DirectoryId filter: a directory-scoped listing drops an id it cannot resolve and answers an empty page, while an unscoped read reports the id as a parameter it would not accept. A fixer should establish which read the parameter error belongs to rather than refusing both.
Fix: DescribeSnapshots' SnapshotIds resolution guard in its handler (describe_snapshots.rs) — the directory-scoped listing must answer a page rather than refuse, and the re-delete not-found message belongs in DeleteSnapshot's resolution branch.
Traces: ds.snapshot-create-delete-lifecycle.

Cause 2 of 2: AddIpRoutes holds no directory-update lock, so the loser of two concurrent AddIpRoutes for one CIDR is refused as a duplicate route instead of as an update already in progress.
AWS: Smithy — AddIpRoutes(DirectoryId required, IpRoutes required) models ClientException, DirectoryUnavailableException, EntityAlreadyExistsException, EntityDoesNotExistException, InvalidParameterException, IpRouteLimitExceededException (all client, HTTP 400). Recorded behavior: an accepted AddIpRoutes puts the directory under an update (the route itself passes through the Adding status before Added), so of two CONCURRENT AddIpRoutes for the same CIDR exactly one succeeds and the other is refused 400 ClientException with 'An update is already in progress for directory <directory id>. Please wait until this update is complete before making another change.' — fixed text, the only variable part the directory id taken from the request; afterwards the surviving route reaches Added and ListIpRoutes holds exactly one entry for the CIDR.
Emulator: it applies the first call synchronously and answers the second out of its sequential duplicate-route branch — 400 ClientException 'The IP route(s) [<cidr>] already exist. If you want to recreate them, please delete the existing routes first.' That text is correct for a duplicate issued AFTER the first add settled (it is what the reopened issue's cause 1 installed), but it is not the refusal AWS gives while the directory's update is in flight, and it leaves the in-progress condition unobservable. The second check accepts EntityAlreadyExistsException or a ClientException naming an in-progress / under-maintenance / unavailable-for-updates condition, and the emulator's answer is neither.
Why: a client that serializes its own mutations on this refusal (back off and retry while the directory is updating, versus stop because the route already exists) takes the wrong branch, and no caller can observe that AWS serializes directory mutations at all.
Fix: AddIpRoutes' handler together with the directory's update-in-flight state in src/state.rs — an accepted AddIpRoutes must mark the directory as updating for the transition span the route's Adding status already uses, and a concurrent mutation arriving in that window must be refused with the update-in-progress ClientException rather than evaluated against the already-applied route set.
Traces: ds.concurrent-add-ip-routes-conflict-detected, ds.concurrent-add-ip-routes-duplicate-conflict.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout (Total 113/178; ds 109/155, 46 failed). Per-run values (request ids, the d-/s- ids, host and port) are not requirements.

Cause 1 — ds.snapshot-create-delete-lifecycle [error]:
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the DescribeSnapshots operation: Invalid snapshot ID: s-003ba5fe5a. : RequestId: 00000000-0000-4000-8000-000000000001
  The check's flow is CreateSnapshot -> poll DescribeSnapshots(DirectoryId, SnapshotIds=[id]) to Completed -> DeleteSnapshot -> poll DescribeSnapshots(DirectoryId, SnapshotIds=[id]) until the id is gone (it tolerates 200-with-empty-Snapshots or EntityDoesNotExistException) -> DeleteSnapshot again, expecting EntityDoesNotExistException 'Snapshot: <id> does not exist'. The InvalidParameterException escapes that poll, so the check errors out and the final two assertions are never reached.
  Emulator source: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/describe_snapshots.rs lines 236-244 refuse ANY unresolved SnapshotIds entry with InvalidParameterException; create_snapshot.rs line 197 does persist the record (store.insert_manual_snapshot), and state.rs mints snapshot ids through mint_snapshot_id (never repeating in a process), so the refused id is one the service itself issued and then dropped on delete.

Cause 2 — ds.concurrent-add-ip-routes-conflict-detected [mismatch]:
  AssertionError: loser was ClientException but not the directory-update-lock rejection: {'Message': 'The IP route(s) [198.51.100.0/24] already exist. If you want to recreate them, please delete the existing routes first. : RequestId: <id>', 'Code': 'ClientException', 'RequestId': '<id>'}
  expected Message: 'An update is already in progress for directory <directory id>. Please wait until this update is complete before making another change. : RequestId: <id>' (Code ClientException matches).
Cause 2 — ds.concurrent-add-ip-routes-duplicate-conflict [mismatch]:
  AssertionError: ClientException on the losing racer was not a directory-busy contention error: The IP route(s) [198.51.100.0/24] already exist. If you want to recreate them, please delete the existing routes first. : RequestId: <id>
  This check accepts either EntityAlreadyExistsException or a ClientException whose message names an in-progress / under-maintenance / unavailable-for-updates condition; both racers are issued from a threading.Barrier, so the two calls genuinely overlap.
  Model evidence: AddIpRoutes' modelled errors (ClientException, DirectoryUnavailableException, EntityAlreadyExistsException, EntityDoesNotExistException, InvalidParameterException, IpRouteLimitExceededException, ServiceException) in services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite() — the emulator boots and ds answers 109/155 checks, so these are fidelity divergences, not a bootstrap gap.
2. Cause 1: on an Active Managed Microsoft AD call CreateSnapshot, poll DescribeSnapshots(DirectoryId=<dir>, SnapshotIds=[<new id>]) until Status=Completed, DeleteSnapshot(SnapshotId=<id>), then repeat DescribeSnapshots(DirectoryId=<dir>, SnapshotIds=[<id>]) — expect 200 with an empty Snapshots list (or EntityDoesNotExistException); observe 400 InvalidParameterException 'Invalid snapshot ID: <id>.'. Then DeleteSnapshot the id again and compare the message to 'Snapshot: <id> does not exist'.
3. Cause 2: on an Active Managed Microsoft AD fire two AddIpRoutes(DirectoryId=<dir>, IpRoutes=[{CidrIp:'198.51.100.0/24'}]) concurrently from two threads released by one barrier — expect exactly one 200 and one 400 ClientException 'An update is already in progress for directory <dir>. Please wait until this update is complete before making another change.'; observe the duplicate-route ClientException instead. Then confirm the route settles at Added and ListIpRoutes holds exactly one entry.
4. Re-replay ds.snapshot-create-delete-lifecycle, ds.concurrent-add-ip-routes-conflict-detected and ds.concurrent-add-ip-routes-duplicate-conflict.
