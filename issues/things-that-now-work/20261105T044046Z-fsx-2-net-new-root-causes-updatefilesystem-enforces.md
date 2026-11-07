Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fsx.storage-capacity-update-below-ten-percent-floor-refused fsx.data-repository-association-invalid-repository-path-rejected
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx.storage-capacity-update-below-ten-percent-floor-refused|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: fsx.data-repository-association-invalid-repository-path-rejected|run#10|AssertionError|assert bad_code == "BadRequest", (
Issue-Commit: c3e4a7d13e4fc7f37ece0835bec9cc006759acbc

# fsx: 2 net-new root causes — UpdateFileSystem enforces no storage-capacity floor and applies the target immediately, and CreateDataRepositoryAssociation answers 501 NotImplemented instead of validating the S3 repository path

> _Note: this issue was identified against the code base at commit `c3e4a7d13e4fc7f37ece0835bec9cc006759acbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFileSystem

## Description
Two independent, net-new root causes in the bootstrapped `fsx` emulator, both grounded in failing behavioral checks. No open issue covers either (a listing filtered on `fsx` and on `datarepository` returns nothing).

=== Cause 1 of 2: UpdateFileSystem accepts a storage-capacity target AWS refuses, and applies it synchronously ===
Failing check: fsx.storage-capacity-update-below-ten-percent-floor-refused

AWS: Smithy model, UpdateFileSystemRequest.StorageCapacity documentation: "For FSx for OpenZFS file systems, the storage capacity target value must be at least 10 percent greater than the current storage capacity value." The same rule is documented for Windows and for ONTAP increases, and for Lustre the target "must be greater than the current storage capacity". A target that is a decrease, equal to the current value, or an increase inside that 10% band is therefore refused, not queued. UpdateFileSystem models `BadRequest` (HTTP 400, "A generic error indicating a failure with a client request"); the recorded refusal for this class is HTTP 400, Error.Code `BadRequest`, with the message template `Invalid desired storage capacity provided: <requested> GiB. Desired storage capacity must be at least 10% higher than the existing value of <current> GiB.` — the two sentences are fixed text; `<requested>` is the request's StorageCapacity and `<current>` is the file system's stored StorageCapacity, both taken from the request and from state, never from a single recording. Additionally, AWS does not apply an accepted capacity target in-line: the UpdateFileSystem response still reports the OLD StorageCapacity and carries one `AdministrativeActions` entry of AdministrativeActionType `FILE_SYSTEM_UPDATE` whose `TargetFileSystemValues.StorageCapacity` is the requested target; the reported StorageCapacity only changes when that action reaches `COMPLETED`, and the file system stays `AVAILABLE` throughout a capacity increase. A refused update records no administrative action at all.

Emulator: for a 64 GiB OpenZFS SINGLE_AZ_1 file system, UpdateFileSystem(StorageCapacity=32) — a decrease — is ACCEPTED with HTTP 200 instead of being refused. The returned FileSystem already reports `'StorageCapacity': 32` and `'Lifecycle': 'UPDATING'`, with no `AdministrativeActions` member at all. The check's assertion is: "expected the call to be refused, but it succeeded and returned {'FileSystem': {... 'Lifecycle': 'UPDATING', 'StorageCapacity': 32 ...}}". The same handler would likewise accept an equal target (64) and an increase inside the floor (65). The handler has no capacity validation whatsoever — it only pre-flights the route-table merge bound — then writes the requested value straight onto the stored record, flips lifecycle to UPDATING, and schedules a timed return to AVAILABLE; it never mints a FILE_SYSTEM_UPDATE administrative action.

Why: a client that relies on AWS refusing an out-of-range capacity target silently gets a shrunk or no-op-sized file system instead of a 400. Worse, the three observable facts a capacity-management client polls on are all wrong: the response reports the new capacity before the work is done, there is no FILE_SYSTEM_UPDATE action to poll for COMPLETED, and the file system leaves AVAILABLE for a change AWS performs in place. Any resize workflow written against AWS either loops forever waiting for an administrative action that never appears, or concludes the resize finished instantly and proceeds against a file system whose capacity it never actually validated.

Fix: UpdateFileSystem's validation and state in the operation's handler — add the per-file-system-type storage-capacity target rule (OpenZFS/Windows/ONTAP-increase: at least 10% above current; Lustre: greater than current and on the documented multiples; refuse decreases where the type does not allow them) as a pre-flight that mutates nothing and returns the modeled `BadRequest`, and move an accepted target off the synchronous write onto a pending FILE_SYSTEM_UPDATE administrative action that the state's transition settling completes, leaving the reported StorageCapacity and the AVAILABLE lifecycle untouched until then.

Traces: fsx.storage-capacity-update-below-ten-percent-floor-refused.

=== Cause 2 of 2: CreateDataRepositoryAssociation is an unimplemented stub, so the S3 repository-path validation never happens ===
Failing check: fsx.data-repository-association-invalid-repository-path-rejected

AWS: Smithy model, CreateDataRepositoryAssociation creates a link between a file-system directory and an S3 bucket or prefix; `DataRepositoryPath` (ArchivePath, len 3..4357) is required and "The path can be an S3 bucket or prefix in the format s3://bucket-name/prefix/". The operation models `BadRequest` (HTTP 400), `FileSystemNotFound` (400), `IncompatibleParameterError` (400), `InternalServerError` (500), `ServiceLimitExceeded` (400) and `UnsupportedOperation` (400). FSx validates the path against the real bucket before accepting the association: a `s3://<bucket>` naming a bucket that does not exist is refused HTTP 400 with Error.Code `BadRequest` and the message `Amazon FSx is unable to validate the access to the S3 bucket prefix. S3 bucket does not exist.` — that message is entirely fixed text for this error class, with no variable part. The refused association is never created, so DescribeDataRepositoryAssociations filtered on `file-system-id` returns an empty list. The same call against a bucket that does exist succeeds with an Association whose `Lifecycle` is `CREATING` and whose `DataRepositoryPath` echoes the request.

Emulator: CreateDataRepositoryAssociation answers Error.Code `NotImplemented` ("operation CreateDataRepositoryAssociation is scaffolded but not implemented in this emulator") for every request, well-formed or not. The check's assertion is: "CreateDataRepositoryAssociation against a bucket that does not exist returned 'NotImplemented', not BadRequest". Note the suite did NOT mark this check ALREADY FILED — the check catches the ClientError and asserts on its code, so it is reported as a mismatch and no stub issue was filed for this operation; a listing on `datarepository` confirms none exists.

Why: the entire FSx for Lustre data-repository surface is unreachable. A client cannot create an association at all, so neither the negative path (an absent bucket must be refused 400 BadRequest) nor the positive path (a CREATING association visible through DescribeDataRepositoryAssociations) can be exercised, and a caller that treats `NotImplemented` as a retryable server condition will retry forever.

Fix: the operation's handler — services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/create_data_repository_association.rs currently returns `Scaffolded::not_implemented(...)` and carries the SCAFFOLDED marker. Implement the handler: resolve FileSystemId out of the FSx state, validate DataRepositoryPath's bucket against the S3 service state and refuse an absent bucket with the modeled `BadRequest`, then persist the association in CREATING so DescribeDataRepositoryAssociations can serve it.

Traces: fsx.data-repository-association-invalid-repository-path-rejected.

## Evidence
Cause 1 — fsx.storage-capacity-update-below-ten-percent-floor-refused [mismatch]
Setup: OpenZFS SINGLE_AZ_1 file system, StorageCapacity 64 GiB, ThroughputCapacity 64 MBps, waited to AVAILABLE.
Call: fsx.update_file_system(FileSystemId=<fs>, StorageCapacity=32)   # a decrease
Expected (AWS): HTTP 400, Error.Code 'BadRequest', Error.Message 'Invalid desired storage capacity provided: 32 GiB. Desired storage capacity must be at least 10% higher than the existing value of 64 GiB.' — nothing mutated, no AdministrativeActions recorded. Same for StorageCapacity=64 (equal) and StorageCapacity=65 (inside the 64 -> 70.4 floor). Only 71 GiB and above is accepted, and then the response reports StorageCapacity 64 with exactly one FILE_SYSTEM_UPDATE administrative action whose TargetFileSystemValues.StorageCapacity is 71, Lifecycle staying AVAILABLE until the action is COMPLETED.
Actual (emulator): HTTP 200.
  AssertionError: expected the call to be refused, but it succeeded and returned
  {'FileSystem': {'FileSystemId': 'fs-...', 'FileSystemType': 'OPENZFS', 'Lifecycle': 'UPDATING',
   'StorageCapacity': 32, 'StorageType': 'SSD', ...,
   'OpenZFSConfiguration': {'DeploymentType': 'SINGLE_AZ_1', 'ThroughputCapacity': 64, ...}},
   'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Three divergences in one response: the refusal is missing, the requested capacity is already applied in the response body, and no AdministrativeActions member is present. (Host, ids, timestamps and request ids in the quoted body are per-run values, not requirements.)
Code: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/update_file_system.rs lines 275-277 assign `record.storage_capacity = input.storage_capacity` unconditionally; lines 240-271 pre-flight only the route-table bound; lines 333-336 set LIFECYCLE_UPDATING with a timed return to AVAILABLE and mint no administrative action.

Cause 2 — fsx.data-repository-association-invalid-repository-path-rejected [mismatch]
Setup: LUSTRE 2.12 SCRATCH_2 file system, 1200 GiB, Lifecycle CREATING.
Call: fsx.create_data_repository_association(FileSystemId=<fs>, FileSystemPath='/missing', DataRepositoryPath='s3://<a bucket that does not exist>', BatchImportMetaDataOnCreate=False)
Expected (AWS): HTTP 400, Error.Code 'BadRequest', Error.Message 'Amazon FSx is unable to validate the access to the S3 bucket prefix. S3 bucket does not exist.' (fixed text, no variable part), and DescribeDataRepositoryAssociations(Filters=[{'Name':'file-system-id','Values':[<fs>]}]).Associations == [].
Actual (emulator): Error.Code 'NotImplemented', message 'operation CreateDataRepositoryAssociation is scaffolded but not implemented in this emulator'.
  AssertionError: CreateDataRepositoryAssociation against a bucket that does not exist returned 'NotImplemented', not BadRequest
Code: services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/ops/create_data_repository_association.rs line 4 (SCAFFOLDED marker) and line 211 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDataRepositoryAssociation"))`.

## Steps to Reproduce
Cause 1 (UpdateFileSystem storage-capacity floor):
1. Create a VPC, a security group admitting the file system's data-plane traffic, and a subnet in an available AZ.
2. fsx.create_file_system(FileSystemType='OPENZFS', StorageCapacity=64, SubnetIds=[<subnet>], SecurityGroupIds=[<sg>], OpenZFSConfiguration={'DeploymentType':'SINGLE_AZ_1','ThroughputCapacity':64,'AutomaticBackupRetentionDays':0,'RootVolumeConfiguration':{'DataCompressionType':'ZSTD','RecordSizeKiB':128}}).
3. Poll describe_file_systems until Lifecycle == 'AVAILABLE'.
4. For each target in (32, 64, 65): fsx.update_file_system(FileSystemId=<fs>, StorageCapacity=target). Each must raise ClientError with Code 'BadRequest' and the +10%-floor message; the emulator returns HTTP 200 with the target already applied and Lifecycle 'UPDATING'.
5. fsx.update_file_system(FileSystemId=<fs>, StorageCapacity=71) — the smallest target clearing the floor. The response must report StorageCapacity 64 plus one FILE_SYSTEM_UPDATE administrative action targeting 71; poll describe_file_systems and confirm the capacity changes only once that action reports COMPLETED and that Lifecycle stays 'AVAILABLE'.

Cause 2 (CreateDataRepositoryAssociation):
1. Create a VPC, security group and subnet as above.
2. fsx.create_file_system(FileSystemType='LUSTRE', FileSystemTypeVersion='2.12', StorageCapacity=1200, SubnetIds=[<subnet>], SecurityGroupIds=[<sg>], LustreConfiguration={'DeploymentType':'SCRATCH_2'}).
3. fsx.create_data_repository_association(FileSystemId=<fs>, FileSystemPath='/missing', DataRepositoryPath='s3://<name of a bucket that was never created>', BatchImportMetaDataOnCreate=False) — expect ClientError Code 'BadRequest' with message 'Amazon FSx is unable to validate the access to the S3 bucket prefix. S3 bucket does not exist.'; the emulator raises Code 'NotImplemented'.
4. fsx.describe_data_repository_associations(Filters=[{'Name':'file-system-id','Values':[<fs>]}]) — must be empty.
5. s3.create_bucket(Bucket=<b>) then repeat step 3 with DataRepositoryPath='s3://<b>' and FileSystemPath='/data' — must return an Association with Lifecycle 'CREATING' and DataRepositoryPath echoing the request, and step 4 must then list exactly that AssociationId.

Replay: run_behavioral_suite and confirm fsx.storage-capacity-update-below-ten-percent-floor-refused and fsx.data-repository-association-invalid-repository-path-rejected both pass.
