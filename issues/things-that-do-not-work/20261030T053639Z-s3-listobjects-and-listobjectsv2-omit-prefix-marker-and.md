Issue-Source: trace-sweep-divergence
Issue-Trace: s3-list-objects-v1-marker-pagination
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-list-objects-v1-marker-pagination s3w-list-objects-v1-marker-pagination s3-multi-object-delete-and-bucket-emptying s3-object-core-crud-and-storage-management s3w-list-objects-v2-delimiter-rollup s3w-list-objects-v2-continuation-pagination s3w-list-objects-v2-special-character-keys
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListObjects|2xx->2xx|$.ListBucketResult.Contents[*].ChecksumAlgorithm
Issue-Key: s3|ListObjects|2xx->2xx|$.ListBucketResult.Contents[*].ChecksumType
Issue-Key: s3|ListObjects|2xx->2xx|$.ListBucketResult.Marker
Issue-Key: s3|ListObjects|2xx->2xx|$.ListBucketResult.Contents.ChecksumAlgorithm
Issue-Key: s3|ListObjects|2xx->2xx|$.ListBucketResult.Contents.ChecksumType
Issue-Key: s3|ListObjects|2xx->2xx|$.ListBucketResult.Prefix
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents.ChecksumAlgorithm
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents.ChecksumType
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[*].ChecksumAlgorithm
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents[*].ChecksumType
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.MaxKeys
Issue-Related: 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md
Issue-Commit: 90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1

# S3 ListObjects and ListObjectsV2 omit Prefix, Marker and checksum members

> _Note: this issue was identified against the code base at commit `90f6d8f18d4862cb01da8533cfc445c7cc8c7cb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjects

## Description
Ten net-new root causes in the ListObjects (v1) and ListObjectsV2 responses: the echoed <Prefix>/<Marker> elements are dropped, every <Contents> entry loses ChecksumAlgorithm and ChecksumType, and a requested max-keys above 1000 is silently clamped and reported as 1000. A fixer must close all ten.

AWS (common): the <ListBucketResult> document echoes the request's Prefix (and, for v1, Marker) as elements, emitting empty elements when the request did not supply them, and each <Contents> entry carries Key, LastModified, ETag, ChecksumAlgorithm, ChecksumType, Size, StorageClass. MaxKeys echoes the value the caller asked for. Smithy: ListObjectsV2Output (xmlName ListBucketResult) models Prefix and MaxKeys; com.amazonaws.s3#Object models ChecksumAlgorithm (a flattened list) and ChecksumType.

Why (common): a client that pages with Marker reads it out of the response; a client that verifies object integrity from the listing reads ChecksumAlgorithm/ChecksumType; and a client that asks for more than 1000 keys and is told MaxKeys=1000 cannot tell a clamped request from a server-side default, so it mis-sizes its paging loop. All of these silently change the meaning of a successful listing.

Cause 1 of 10: ListObjects omits Marker and the Contents checksum members on a marker-paginated page.
Emulator: $.ListBucketResult.Marker absent (expected an empty element); Contents[0]/[1].ChecksumAlgorithm and .ChecksumType absent.
Fix: the ListObjects (v1) serializer and the object metadata it reads.
Traces: s3-list-objects-v1-marker-pagination#task#7 (also s3w-list-objects-v1-marker-pagination#task#7).

Cause 2 of 10: ListObjects omits the Contents checksum members on a mid-page listing.
Fix: as cause 1.
Traces: s3-list-objects-v1-marker-pagination#task#8 (also s3w-list-objects-v1-marker-pagination#task#8).

Cause 3 of 10: ListObjects omits the checksum members on a single-entry listing.
Fix: as cause 1.
Traces: s3-list-objects-v1-marker-pagination#task#9 (also s3w-list-objects-v1-marker-pagination#task#9).

Cause 4 of 10: ListObjects omits both Prefix and Marker on an unfiltered listing.
Fix: as cause 1 — both echoed elements must always be present.
Traces: s3-multi-object-delete-and-bucket-emptying#task#2.

Cause 5 of 10: ListObjects omits Prefix, Marker and the checksum members together.
Fix: as cause 1.
Traces: s3-object-core-crud-and-storage-management#task#6.

Cause 6 of 10: ListObjectsV2 omits Prefix and the Contents checksum members.
Fix: the ListObjectsV2 serializer. The open issue 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md concerns a different Prefix fault (encoding, not omission) and does not carry these replay keys.
Traces: s3-object-core-crud-and-storage-management#task#7 (also s3w-list-objects-v2-delimiter-rollup#task#6).

Cause 7 of 10: ListObjectsV2 omits the Contents checksum members on a continuation page.
Fix: as cause 6.
Traces: s3w-list-objects-v2-continuation-pagination#task#7 (also #task#8, s3w-list-objects-v2-delimiter-rollup#task#7).

Cause 8 of 10: ListObjectsV2 omits the checksum members on the final single-entry page.
Fix: as cause 6.
Traces: s3w-list-objects-v2-continuation-pagination#task#9.

Cause 9 of 10: ListObjectsV2 reports MaxKeys 1000 for a request that asked for more.
AWS: MaxKeys echoes the requested value (the response still returns at most 1000 keys, but the echoed MaxKeys is the caller's number).
Emulator: clamps the echoed value to 1000.
Fix: the ListObjectsV2 max-keys handling — clamp the page size, echo the request.
Traces: s3w-list-objects-v2-continuation-pagination#task#10.

Cause 10 of 10: ListObjectsV2 omits the checksum members for special-character keys.
Fix: as cause 6.
Traces: s3w-list-objects-v2-special-character-keys#task#7

## Evidence
All ten clusters are 'mismatch' with matching status 200.
s3-list-objects-v1-marker-pagination#task#7: $.ListBucketResult.Marker missing (expected ''); Contents[0]/[1].ChecksumAlgorithm missing (expected 'CRC32'); Contents[0]/[1].ChecksumType missing (expected 'FULL_OBJECT')
s3-list-objects-v1-marker-pagination#task#8: Contents[0]/[1].ChecksumAlgorithm and .ChecksumType missing
s3-list-objects-v1-marker-pagination#task#9: Contents.ChecksumAlgorithm and .ChecksumType missing
s3-multi-object-delete-and-bucket-emptying#task#2: $.ListBucketResult.Prefix and .Marker missing (expected '')
s3-object-core-crud-and-storage-management#task#6: Prefix and Marker missing; Contents.ChecksumAlgorithm and .ChecksumType missing
s3-object-core-crud-and-storage-management#task#7 (ListObjectsV2): Prefix missing; Contents.ChecksumAlgorithm and .ChecksumType missing
s3w-list-objects-v2-continuation-pagination#task#7 (ListObjectsV2): Contents[0]/[1] ChecksumAlgorithm and ChecksumType missing
s3w-list-objects-v2-continuation-pagination#task#9 (ListObjectsV2): Contents.ChecksumAlgorithm and .ChecksumType missing
s3w-list-objects-v2-continuation-pagination#task#10 (ListObjectsV2): $.ListBucketResult.MaxKeys value_mismatch (expected '5000', got '1000'); five Contents entries missing ChecksumAlgorithm/ChecksumType
s3w-list-objects-v2-special-character-keys#task#7 (ListObjectsV2): five Contents entries missing ChecksumAlgorithm/ChecksumType

## Steps to Reproduce
Replay the wire corpus and re-run the fixture#step bindings in Issue-Trace/Issue-Step and trace_ids. By hand: PutObject several keys with x-amz-sdk-checksum-algorithm CRC32; GET /<bucket> (v1) with and without marker=/prefix= and check that <Prefix> and <Marker> are always present and each <Contents> carries <ChecksumAlgorithm> and <ChecksumType>; GET /<bucket>?list-type=2 plainly, with continuation-token, and with max-keys=5000 and check the echoed <Prefix>, the checksum members and that <MaxKeys> echoes 5000.
