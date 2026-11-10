Issue-Source: trace-sweep-divergence
Issue-Trace: s3-bucket-configuration-crud
Issue-Step: teardown#22
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-bucket-configuration-crud s3-bucket-lifecycle-notifications-replication s3-bucket-monitoring-and-analytics-configurations s3-bulk-object-delete-and-bucket-service-settings s3-object-core-crud-and-storage-management s3-versioning-and-object-lock s3-versioning-and-object-versions s3-bucket-lifecycle-and-discovery s3-object-copy-and-rename s3-object-tagging-acl-and-versioning s3-object-tagging-and-acl-management s3-object-versioning-and-delete-markers s3-object-versioning-and-lock
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.KeyMarker
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Prefix
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.VersionIdMarker
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.DeleteMarker.Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version.ChecksumAlgorithm
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version.ChecksumType
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version.Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[*]
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[*].ChecksumAlgorithm
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[*].ChecksumType
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[*].ETag
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[*].Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.DeleteMarker.Key
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[*].Key
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 ListObjectVersions omits markers, Owner and checksum members

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectVersions

## Description
Nine net-new root causes in the ListObjectVersions response. The emulator omits the echo-back marker elements, omits Owner on both Version and DeleteMarker entries, omits the per-version checksum members, echoes a percent-encoded prefix and key, and on one fixture returns the wrong version set entirely. A fixer must close all nine.

AWS (common): the <ListVersionsResult> document always echoes <Prefix>, <KeyMarker> and <VersionIdMarker> — as empty elements when the request did not supply them — and each <Version> carries ETag, ChecksumAlgorithm, ChecksumType, Size, StorageClass, Key, VersionId, IsLatest, LastModified and Owner; each <DeleteMarker> carries Owner, Key, VersionId, IsLatest and LastModified. Smithy: ListObjectVersionsOutput (xmlName ListVersionsResult) models Prefix, KeyMarker and VersionIdMarker; ObjectVersion models ChecksumAlgorithm (flattened list), ChecksumType and Owner; DeleteMarkerEntry models Owner. Prefix/KeyMarker/Key are returned percent-encoded only when the request asked for encoding-type=url.

Why (common): a paginating client reads KeyMarker/VersionIdMarker out of the response to build the next page, so absent elements break pagination; a client that filters by owner or verifies integrity from ChecksumAlgorithm/ChecksumType has nothing to read; and an encoded Prefix/Key that was never requested encoded makes every returned key wrong (the client then issues GetObject on a key containing a literal '%2F').

Cause 1 of 9: the three marker elements are omitted on a plain listing.
Emulator: Prefix, KeyMarker and VersionIdMarker all absent (expected empty elements).
Fix: the ListObjectVersions serializer, which must always emit the echoed request parameters.
Traces: s3-bucket-configuration-crud#teardown#22 (also s3-bucket-lifecycle-notifications-replication#teardown#17, #teardown#19, s3-bucket-monitoring-and-analytics-configurations#task#22, s3-bulk-object-delete-and-bucket-service-settings#teardown#19, s3-object-core-crud-and-storage-management#task#12, s3-versioning-and-object-lock#teardown#12, s3-versioning-and-object-versions#task#4).

Cause 2 of 9: Version and DeleteMarker entries omit Owner, and Version entries omit ChecksumAlgorithm/ChecksumType.
Emulator: DeleteMarker.Owner, Version.Owner, Version.ChecksumAlgorithm and Version.ChecksumType all absent.
Fix: the ListObjectVersions serializer and the object-version state it reads (owner identity and the stored checksum algorithm/type must be carried per version).
Traces: s3-bucket-lifecycle-and-discovery#teardown#12.

Cause 3 of 9: after a multipart part-copy the version list is wrong — entries are missing, ordered differently and carry the source object's ETag.
Emulator: Version[1] holds the source key instead of the part-copy destination, Version[2]'s ETag is absent, and the checksum/Owner members are missing throughout.
Fix: the CopyObject/UploadPartCopy write path's version bookkeeping plus the ListObjectVersions serializer.
Traces: s3-object-copy-and-rename#teardown#9.

Cause 4 of 9: a prefix-scoped listing omits Prefix and Version.ChecksumType.
Fix: as causes 1 and 2, on the prefix-filtered path.
Traces: s3-object-tagging-acl-and-versioning#task#6.

Cause 5 of 9: a listing after tagging/ACL work omits the three markers plus Version Owner/ChecksumAlgorithm/ChecksumType.
Fix: as causes 1 and 2.
Traces: s3-object-tagging-and-acl-management#task#13.

Cause 6 of 9: a two-version listing omits KeyMarker/VersionIdMarker and both versions' Owner/ChecksumAlgorithm/ChecksumType.
Fix: as causes 1 and 2.
Traces: s3-object-versioning-and-delete-markers#task#6 (also #task#12).

Cause 7 of 9: a listing that includes a delete marker omits the markers, the delete marker's Owner and both versions' checksum members.
Fix: as causes 1 and 2, covering DeleteMarkerEntry.
Traces: s3-object-versioning-and-delete-markers#task#10.

Cause 8 of 9: a listing under object lock omits Prefix and both versions' ChecksumType.
Fix: as causes 1 and 2.
Traces: s3-object-versioning-and-lock#task#6.

Cause 9 of 9: a prefix-scoped listing returns the prefix and every key percent-encoded although encoding-type was not requested.
AWS: Prefix, DeleteMarker.Key and Version.Key are the decoded values (e.g. a prefix containing '/'), and are encoded only when encoding-type=url was sent.
Emulator: '%2F' appears in Prefix and in every Key. The open issue 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md records the same fault for ListObjectsV2 and does not carry this ListObjectVersions replay key.
Fix: the ListObjectVersions request-parameter decoding and key serialization.
Traces: s3-versioning-and-object-versions#task#8

## Evidence
All nine clusters are 'mismatch' with matching status 200.
s3-bucket-configuration-crud#teardown#22: $.ListVersionsResult.Prefix / .KeyMarker / .VersionIdMarker all missing (expected '')
s3-bucket-lifecycle-and-discovery#teardown#12: markers missing; DeleteMarker.Owner missing; Version.ChecksumAlgorithm missing (expected 'CRC32'); Version.ChecksumType missing (expected 'FULL_OBJECT'); Version.Owner missing
s3-object-copy-and-rename#teardown#9: Version[1] missing (expected the part-copy key with its own ETag/ChecksumAlgorithm/Owner, got the source key); Version[0].ETag value_mismatch; Version[2].ETag/ChecksumAlgorithm/ChecksumType/Owner missing; markers missing
s3-object-tagging-acl-and-versioning#task#6: Prefix missing; Version.ChecksumType missing
s3-object-tagging-and-acl-management#task#13: Prefix/KeyMarker/VersionIdMarker missing; Version.ChecksumAlgorithm/ChecksumType/Owner missing
s3-object-versioning-and-delete-markers#task#6: KeyMarker/VersionIdMarker missing; Version[0] and Version[1] ChecksumAlgorithm/ChecksumType/Owner missing
s3-object-versioning-and-delete-markers#task#10: KeyMarker/VersionIdMarker missing; DeleteMarker.Owner missing; both Versions' ChecksumAlgorithm/ChecksumType/Owner missing
s3-object-versioning-and-lock#task#6: Prefix missing; Version[0].ChecksumType and Version[1].ChecksumType missing
s3-versioning-and-object-versions#task#8: Prefix value_mismatch (expected 'versioned/report.txt', got 'versioned%2Freport.txt'); DeleteMarker.Key and Version[0]/[1].Key likewise percent-encoded; markers, Owners and checksum members missing

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand, on a versioning-enabled bucket: PutObject twice for the same key with a checksum algorithm, DeleteObject to create a delete marker, then GET /<bucket>?versions with no parameters and with prefix=<a prefix containing '/'> (no encoding-type). The <ListVersionsResult> must contain empty <Prefix>, <KeyMarker> and <VersionIdMarker>, each <Version> must carry Owner, ChecksumAlgorithm and ChecksumType, each <DeleteMarker> must carry Owner, and Prefix/Key must be the decoded values. Also UploadPartCopy into a new key and re-list to check the version set.
