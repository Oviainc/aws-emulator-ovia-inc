Issue-Source: trace-sweep-divergence
Issue-Trace: fsx-backup-lifecycle-guards
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fsx-backup-lifecycle-guards fsx-data-repository-association-guards
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx|CreateFileSystemFromBackup|4xx->4xx|$.Message
Issue-Key: fsx|CreateFileSystemFromBackup|4xx->4xx|$.__type
Issue-Key: fsx|CreateDataRepositoryAssociation|4xx->4xx|$.__type
Issue-Key: fsx|DescribeDataRepositoryAssociations|2xx->4xx|$.Associations
Issue-Key: fsx|DescribeDataRepositoryAssociations|2xx->4xx|status
Issue-Key: fsx|DeleteDataRepositoryAssociation|4xx->4xx|$.Message
Issue-Commit: 6bba0ea93be6d4db7a2caa629527597bd59ab221

# FSx guard checks return wrong error code, message, or status

> _Note: this issue was identified against the code base at commit `6bba0ea93be6d4db7a2caa629527597bd59ab221`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystemFromBackup

## Description
The emulator's FSx request guards evaluate existence/validation checks in the wrong order, pick the wrong modeled error shape, and in one case reject a legitimate read. Four distinct causes, all in the fsx service, all on the pre-condition ("guard") path of an operation. A fixer must close ALL FOUR before submitting.

Cause 1 of 4: CreateFileSystemFromBackup reports a network error instead of BackupNotFound when the referenced backup does not exist.
AWS: Smithy: CreateFileSystemFromBackup takes required BackupId (pattern ^(backup-[0-9a-f]{8,})$) and required SubnetIds, and models both BackupNotFound ("No Amazon FSx backups were found based upon the supplied parameters.", HTTP 400) and InvalidNetworkSettings (HTTP 400). The recorded AWS response for a request naming a non-existent backup is HTTP 400 with __type 'BackupNotFound' and Message "Backup '<BackupId>' does not exist." — fixed text "Backup '" + "' does not exist." with the BackupId value taken verbatim from the request. AWS resolves the backup referenced by BackupId BEFORE it validates the networking members (SubnetIds/SecurityGroupIds), so an unknown backup is reported as BackupNotFound even when the subnets are also unknown.
Emulator: returns HTTP 400 with __type 'InvalidNetworkSettings' and Message "The subnet ID '<SubnetId>' does not exist", i.e. it validates SubnetIds first and never reaches the backup lookup.
Why: a client that branches on the error code (retry/recreate on BackupNotFound, fix VPC wiring on InvalidNetworkSettings) is steered to the wrong remediation entirely, and the backup id it asked about never appears in the message, so the real cause is invisible.
Fix: the CreateFileSystemFromBackup handler's guard ordering — resolve BackupId against backup state and raise BackupNotFound before running the network-settings validation.
Traces: fsx-backup-lifecycle-guards#task#2.

Cause 2 of 4: CreateDataRepositoryAssociation returns FileSystemNotFound where AWS returns BadRequest.
AWS: Smithy: CreateDataRepositoryAssociation models both BadRequest ("A generic error indicating a failure with a client request.", HTTP 400) and FileSystemNotFound ("No Amazon FSx file systems were found based upon supplied parameters.", HTTP 400), plus UnsupportedOperation and IncompatibleParameterError. The recorded AWS response for this step is HTTP 400 with __type 'BadRequest' — the request is rejected as malformed/unsupported for the target file system, NOT as a missing file system. Recorded behavior only for the exact discriminator; FileSystemNotFound is reserved for the case where no file system matches FileSystemId (pattern ^(fs-[0-9a-f]{8,})$). Hypothesis: the emulator's DRA state is keyed only by file systems it has registered as DRA-capable, so any other file system id falls through to the not-found branch.
Emulator: returns HTTP 400 with __type 'FileSystemNotFound' (status matches, error code does not).
Why: FileSystemNotFound tells a client the file system is gone, so it will stop retrying or try to recreate the file system, when the correct signal is that the request itself must be corrected. Error-code-driven control flow and SDK retry classification both diverge.
Fix: the CreateDataRepositoryAssociation handler's validation — only raise FileSystemNotFound when FileSystemId resolves to nothing in state, and raise BadRequest for a request that is invalid or unsupported against an existing file system.
Traces: fsx-data-repository-association-guards#task#1.

Cause 3 of 4: DescribeDataRepositoryAssociations errors instead of returning an empty list.
AWS: Smithy: DescribeDataRepositoryAssociations has output Associations (list, length 0..100) and NextToken, and is paginated. A describe/list call that matches nothing is a successful empty page, not an error: the recorded AWS response is HTTP 200 with Associations: []. DataRepositoryAssociationNotFound/FileSystemNotFound are modeled for an explicitly named identifier that does not exist, not for an unfiltered or filtered query with no matches.
Emulator: returns HTTP 400 and no Associations member at all (expected [], got absent).
Why: every client (SDK paginators included) treats a describe as a read that yields zero or more results; a 400 turns "nothing exists yet" into a hard failure, so poll/converge loops and "create if absent" logic abort instead of proceeding.
Fix: the DescribeDataRepositoryAssociations handler — return 200 with an empty Associations list when no association matches, and always serialize the Associations member.
Traces: fsx-data-repository-association-guards#task#2 (cluster also covers fsx-data-repository-association-guards#task#3).

Cause 4 of 4: DeleteDataRepositoryAssociation's not-found message interpolates the association id.
AWS: Smithy: DeleteDataRepositoryAssociation takes required AssociationId (pattern ^(dra-[0-9a-f]{8,})$) and models DataRepositoryAssociationNotFound ("No data repository associations were found based upon the supplied parameters.", HTTP 400) with a Message of length 1..256. The recorded AWS response for deleting an unknown association is HTTP 400 with Message "Data repository association does not exist." — that whole sentence is FIXED text with NO variable part; AWS does not echo the requested AssociationId in this message.
Emulator: returns HTTP 400 with Message "Data repository association '<AssociationId>' does not exist.", inserting the requested id into the sentence.
Why: the status and error code are right but the message text is not AWS's, so assertions and log/alarm patterns matched against the documented message text fail, and the emulator reveals a field AWS deliberately omits.
Fix: the DeleteDataRepositoryAssociation error serializer/handler — emit the fixed message text without interpolating AssociationId.
Traces: fsx-data-repository-association-guards#task#5.

No existing open issue covers any of these (no open fsx issues are filed).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, 45 steps, 21 passed, score 46.7%). Four fsx divergence clusters, none carrying stub_path or owned_by:

1) fsx-backup-lifecycle-guards#task#2 — CreateFileSystemFromBackup, verdict mismatch, expected_status 400 / actual_status 400.
   expected $.__type = 'BackupNotFound', actual 'InvalidNetworkSettings'
   expected $.Message = "Backup 'backup-04c8e1b7fa63d9502' does not exist.", actual "The subnet ID 'subnet-0d4f81b6c2e97a350' does not exist"
   (ids above are this recording's values filling the template; the requirement is the fixed text plus the request's own BackupId.)

2) fsx-data-repository-association-guards#task#1 — CreateDataRepositoryAssociation, verdict mismatch, 400 -> 400.
   expected $.__type = 'BadRequest', actual 'FileSystemNotFound'

3) fsx-data-repository-association-guards#task#2 (cluster also affects #task#3) — DescribeDataRepositoryAssociations, verdict mismatch.
   status: expected 200, got 400
   $.Associations: missing (expected [], got None)

4) fsx-data-repository-association-guards#task#5 — DeleteDataRepositoryAssociation, verdict mismatch, 400 -> 400.
   expected $.Message = 'Data repository association does not exist.', actual "Data repository association 'dra-07b9c3e51d8a64f20' does not exist."

Model evidence (services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/smithy-model): CreateFileSystemFromBackup errors include BackupNotFound and InvalidNetworkSettings (both 400); CreateDataRepositoryAssociation errors include BadRequest, FileSystemNotFound, UnsupportedOperation; DescribeDataRepositoryAssociations output Associations is a list with length min 0 and is paginated; DeleteDataRepositoryAssociation errors include DataRepositoryAssociationNotFound with Message length 1..256.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay fixture fsx-backup-lifecycle-guards, step task#2 — CreateFileSystemFromBackup naming a backup id that is not in state together with subnet ids that are not in state. Observe __type 'InvalidNetworkSettings' instead of 'BackupNotFound'.
4. Cause 2: replay fixture fsx-data-repository-association-guards, step task#1 — CreateDataRepositoryAssociation. Observe __type 'FileSystemNotFound' instead of 'BadRequest'.
5. Cause 3: replay fixture fsx-data-repository-association-guards, steps task#2 and task#3 — DescribeDataRepositoryAssociations with no matching association. Observe HTTP 400 with no Associations member instead of HTTP 200 with Associations: [].
6. Cause 4: replay fixture fsx-data-repository-association-guards, step task#5 — DeleteDataRepositoryAssociation for an association id that is not in state. Observe the Message with the association id interpolated instead of the fixed sentence 'Data repository association does not exist.'
