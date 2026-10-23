Issue-Source: trace-sweep-divergence
Issue-Trace: efs-filesystem-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-filesystem-lifecycle efs-tags-workflow
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|UpdateFileSystem|2xx->2xx|$.KmsKeyId
Issue-Key: elasticfilesystem|UpdateFileSystem|2xx->2xx|$.SizeInBytes.Timestamp
Issue-Key: elasticfilesystem|UpdateFileSystem|2xx->2xx|$.SizeInBytes.ValueInArchive
Issue-Key: elasticfilesystem|UpdateFileSystem|2xx->2xx|$.ThroughputMode
Issue-Key: elasticfilesystem|DescribeTags|2xx->2xx|$.Tags[Key="team"]
Issue-Key: elasticfilesystem|ListTagsForResource|2xx->2xx|$.Tags[Key="team"]
Issue-Commit: eca724180e57b7e4ce6848ede7699bb9bf86a037

# EFS mutation results diverge: UpdateFileSystem payload and dropped tag writes

> _Note: this issue was identified against the code base at commit `eca724180e57b7e4ce6848ede7699bb9bf86a037`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFileSystem

## Description
Three net-new divergence clusters on service elasticfilesystem (EFS, Smithy sdkId EFS / sigv4Name elasticfilesystem, api 2015-02-01, restJson1), all sharing one fault family: the result of an accepted EFS mutation is served wrongly on the very next response that is supposed to report it. A throughput-mode update is reported as already applied in the 202 that accepts it, and a tag write never shows up on either tag read path. THIS ISSUE HAS 3 CAUSES; all three must be fixed before it is closed.

NOTE ON OVERLAP: the open issue 20261022T151224Z-elasticfilesystem-efs-file-system-description-diverges-omitted-members-wrong.md carries replay keys for CreateFileSystem and DescribeFileSystems only (including the throughput mode missing from the LATER DescribeFileSystems at efs-filesystem-lifecycle#task#5). It does NOT carry the UpdateFileSystem response path or either tag-read operation, and the replay report confirms this by reporting all three clusters below with an empty owned_by and an empty stub_path. They are filed here as net-new.

Cause 1 of 3 - UpdateFileSystem's 202 response reports the NEW throughput mode immediately, where AWS still reports the pre-update mode, and the same response re-serves the FileSystemDescription defects on the update path (fixture efs-filesystem-lifecycle, step task#4).
AWS: Smithy UpdateFileSystem = PUT /2015-02-01/file-systems/{FileSystemId}, http code 202, output_shape FileSystemDescription; input members ThroughputMode (enum bursting|provisioned|elastic) and ProvisionedThroughputInMibps (@range min 1.0); errors BadRequest 400, FileSystemNotFound 404, IncorrectFileSystemLifeCycleState 409, InsufficientThroughputCapacity 503, ThroughputLimitExceeded 400, TooManyRequests 429, InternalServerError 500. LifeCycleState is an enum that includes 'updating'. Recorded behavior only: in this fixture the file system reads as bursting at task#2, the update to elastic is accepted at task#4 with a 202 whose ThroughputMode is STILL 'bursting', and only the later read at task#5 reports 'elastic'. Hypothesis: the 202 is an acceptance, not a completion - AWS moves the file system to LifeCycleState 'updating' and keeps serving the current throughput mode until the transition finishes. Also on this response, Smithy FileSystemDescription.KmsKeyId (@length 0..2048, pattern admitting a bare key id, mrk-, alias/NAME or a kms ARN) is recorded as a concrete resolved key identifier (shape-asserted), FileSystemSize.ValueInArchive is a number (recorded 0), and FileSystemSize.Timestamp ('the time at which the size ... was determined') is absent when no metering has occurred.
Emulator: 202 with ThroughputMode = 'elastic' (value_mismatch, expected 'bursting'), KmsKeyId = 'arn:aws:kms:us-west-2:000000000000:alias/aws/elasticfilesystem' (an alias ARN where the recording shape-asserts a resolved key identifier), SizeInBytes.ValueInArchive missing (expected 0), SizeInBytes.Timestamp present (type_mismatch, expected absent).
Why: the emulator inverts the visibility of the mutation - eager in the accept, then absent from the next read (the task#5 half of that inversion is owned by the issue named above). A client that treats the 202 as proof of completion is told the mode changed before any transition exists, while a client that polls DescribeFileSystems for the new mode never sees it, so read-after-write loops either exit a transition early or never terminate. The alias ARN in KmsKeyId cannot be resolved or compared by a client that feeds it to KMS or pins it against a configured CMK, and ValueInArchive reads as unknown instead of zero.
Fix: the EFS UpdateFileSystem handler plus the shared FileSystemDescription serializer - the handler must record the requested throughput mode as pending and serialize the file system's CURRENT mode (with LifeCycleState reflecting the transition) in the 202, and the serializer used by update must be the same corrected one used by create/describe (always emit the full FileSystemSize member set including ValueInArchive, emit Timestamp only when a metering time exists, and serve the resolved key identifier rather than the default-key alias ARN).
Traces: efs-filesystem-lifecycle#task#4.

Cause 2 of 3 - DescribeTags omits a tag that the fixture wrote to the file system (fixture efs-tags-workflow, step task#5).
AWS: Smithy DescribeTags = GET /2015-02-01/tags/{FileSystemId}, http code 200, output DescribeTagsResponse{Marker, Tags (required, list<Tag>), NextMarker}, paginated on Marker/NextMarker with items=Tags; Tag = {Key (required, @length 1..128), Value (required, @length 0..256)}; errors BadRequest 400, FileSystemNotFound 404, InternalServerError 500. Tag writes arrive via TagResource (POST /2015-02-01/resource-tags/{ResourceId}, 200) or the deprecated CreateTags (POST /2015-02-01/create-tags/{FileSystemId}, 204), and the recording shows the tag set read back containing the tag that was written - here Tags includes {'Key': 'team', 'Value': 'storage'}.
Emulator: 200 with the correct status and an otherwise-matching tag list, but $.Tags[Key="team"] missing (expected {'Key': 'team', 'Value': 'storage'}, got None) - the tag the fixture wrote is simply not in the returned set.
Why: a tag write is acknowledged and then lost, so every consumer of EFS tags gets a short list: cost-allocation and ownership lookups keyed on the tag find nothing, tag-based resource selection skips the file system, and an add-then-verify sequence fails against the emulator even though the write returned success.
Fix: EFS tag state and the tag-write handlers (TagResource / CreateTags) - persist every submitted tag against the file-system record (merging by key rather than replacing or dropping), and have DescribeTags read that one stored set.
Traces: efs-tags-workflow#task#5.

Cause 3 of 3 - ListTagsForResource omits the same tag, so no read path can observe it (fixture efs-tags-workflow, step task#7).
AWS: Smithy ListTagsForResource = GET /2015-02-01/resource-tags/{ResourceId}, http code 200, output ListTagsForResourceResponse{Tags (list<Tag>), NextToken}, paginated on NextToken/MaxResults; ResourceId accepts a file-system or access-point id/ARN; errors AccessPointNotFound 404, BadRequest 404/400 family (FileSystemNotFound 404, BadRequest 400), InternalServerError 500. DescribeTags is modeled @deprecated with message 'Use ListTagsForResource.', so the two operations are two views of ONE tag set and must agree; the recording shows the written tag present here too.
Emulator: 200 with $.Tags[Key="team"] missing (expected {'Key': 'team', 'Value': 'storage'}, got None).
Why: the loss is in the stored tag set rather than in one serializer, so migrating from the deprecated DescribeTags to the recommended ListTagsForResource does not recover the tag - a client has no way at all to read back what it tagged.
Fix: same EFS tag state as Cause 2 - ListTagsForResource must resolve the resource id to the same stored tag set that the tag-write handlers populate, so the deprecated and current read paths cannot diverge from each other or from the write.
Traces: efs-tags-workflow#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=65, steps_passed=35, score_pct=53.8, divergence_count=7). Of the 7 clusters, 4 report owned_by the open issue 20261022T151224Z-elasticfilesystem-efs-file-system-description-diverges-omitted-members-wrong.md and are skipped; the 3 below report empty owned_by and empty stub_path and are the content of this issue. All three are verdict=mismatch with the EXPECTED HTTP status, so the divergence is purely in payload/state, not routing or status.

1) Cause 1 - efs-filesystem-lifecycle#task#4, UpdateFileSystem, expected 202 / actual 202:
   $.ThroughputMode: value_mismatch (expected 'bursting', got 'elastic')
   $.KmsKeyId: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:kms:us-west-2:000000000000:alias/aws/elasticfilesystem')
   $.SizeInBytes.Timestamp: type_mismatch (expected None, got 1792751054)
   $.SizeInBytes.ValueInArchive: missing (expected 0, got None)
   Context from the same fixture: task#2 DescribeFileSystems records ThroughputMode 'bursting' and task#5 records 'elastic', so the recorded order is bursting -> accept(202, still bursting) -> elastic, while the emulator reports elastic at task#4.

2) Cause 2 - efs-tags-workflow#task#5, DescribeTags, expected 200 / actual 200:
   $.Tags[Key="team"]: missing (expected {'Key': 'team', 'Value': 'storage'}, got None)

3) Cause 3 - efs-tags-workflow#task#7, ListTagsForResource, expected 200 / actual 200:
   $.Tags[Key="team"]: missing (expected {'Key': 'team', 'Value': 'storage'}, got None)

Model consulted: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json - UpdateFileSystem (PUT /2015-02-01/file-systems/{FileSystemId}, 202, output FileSystemDescription), DescribeTags (GET /2015-02-01/tags/{FileSystemId}, 200, @deprecated 'Use ListTagsForResource.'), ListTagsForResource (GET /2015-02-01/resource-tags/{ResourceId}, 200), TagResource (POST /2015-02-01/resource-tags/{ResourceId}, 200), CreateTags (POST /2015-02-01/create-tags/{FileSystemId}, 204), shapes ThroughputMode (bursting|provisioned|elastic), LifeCycleState (creating|available|updating|deleting|deleted|error), Tag{Key,Value}, FileSystemSize{Value,Timestamp,ValueInIA,ValueInStandard,ValueInArchive}. Account ids, region, generated file-system ids, probe names and epoch values quoted above are environment-specific examples, not requirements; the requirements are the statuses (202/200/200), the member presence/absence and the mutation-visibility ordering.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Cause 1 (representative): fixture efs-filesystem-lifecycle, step task#4 - after the file system reads as ThroughputMode 'bursting' at task#2, issue UpdateFileSystem (PUT /2015-02-01/file-systems/{FileSystemId}) switching it to elastic. The 202 body comes back with ThroughputMode 'elastic' where the recording has 'bursting'; the same body also carries the alias-ARN KmsKeyId, a SizeInBytes.Timestamp that should be absent, and no SizeInBytes.ValueInArchive.
4. Cause 2: fixture efs-tags-workflow, step task#5 - after the fixture's tag write, call DescribeTags (GET /2015-02-01/tags/{FileSystemId}); the returned Tags list has no entry with Key 'team' (recording: {'Key':'team','Value':'storage'}).
5. Cause 3: fixture efs-tags-workflow, step task#7 - call ListTagsForResource (GET /2015-02-01/resource-tags/{ResourceId}) for the same file system; the same tag is missing there too, confirming the loss is in stored tag state rather than in one serializer.
6. All three clusters replay with matching HTTP status (202, 200, 200) and verdict=mismatch; re-replay efs-filesystem-lifecycle#task#4, efs-tags-workflow#task#5 and efs-tags-workflow#task#7 to confirm all three causes are fixed.
