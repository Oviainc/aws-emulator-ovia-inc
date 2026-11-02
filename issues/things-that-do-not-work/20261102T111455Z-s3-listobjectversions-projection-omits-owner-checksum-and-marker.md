Issue-Source: trace-sweep-divergence
Issue-Trace: s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions s3-mvb-delete-wrong-key-mints-marker s3-mvs3-delete-folder-key-is-not-recursive s3-mbes-traj-hand-rolled-version-paging
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent-persistent.sh"].ChecksumAlgorithm
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent-persistent.sh"].ChecksumType
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent-persistent.sh"].Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent-professional.sh"].ChecksumAlgorithm
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent-professional.sh"].ChecksumType
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent-professional.sh"].Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent.sh"].ChecksumAlgorithm
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent.sh"].ChecksumType
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.Version[Key="flint-agent.sh"].Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.DeleteMarker[*]
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.DeleteMarker[*].Owner
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.DeleteMarker[*].IsLatest
Issue-Key: s3|ListObjectVersions|2xx->2xx|$.ListVersionsResult.DeleteMarker[*].Key
Issue-Key: s3|ListObjectVersions|4xx->2xx|$.Error
Issue-Key: s3|ListObjectVersions|4xx->2xx|status
Issue-Related: 20261030T053639Z-s3-listobjectversions-omits-markers-owner-and-checksum-members.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3 ListObjectVersions projection omits Owner checksum and marker members

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectVersions

## Description
Cause 1 of 4: Version and DeleteMarker entries omit Owner. AWS: ListObjectVersions always projects <Owner><ID> on every Version and DeleteMarker entry (there is no fetch-owner switch on this operation). Smithy: ObjectVersion declares Owner, and DeleteMarkerEntry likewise. Emulator: Owner is absent from every entry. Why: a caller reconciling ownership (or feeding the id into an ACL/ownership decision) sees an unowned object. Fix: the ListObjectVersions projection in the object-listing serializer, populating Owner from the bucket owner held in state. Traces: s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions#task#7, s3-mvb-delete-wrong-key-mints-marker#task#9, s3-mvs3-delete-folder-key-is-not-recursive#task#12.

Cause 2 of 4: Version entries omit ChecksumAlgorithm/ChecksumType. AWS: an object written with a CRC32 full-object checksum is projected with <ChecksumAlgorithm>CRC32</ChecksumAlgorithm> and <ChecksumType>FULL_OBJECT</ChecksumType>. Smithy: ObjectVersion declares both members. Emulator: both absent, although the object was stored with that checksum. Why: an integrity-checking client cannot tell which algorithm to verify against. Fix: the same projection, reading the checksum recorded on the version in state. Traces: s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions#task#7, s3-mvs3-delete-folder-key-is-not-recursive#task#12.

Cause 3 of 4: the empty-valued echo members are dropped. AWS: when no prefix/marker was supplied, ListObjectVersions still emits <Prefix></Prefix>, <KeyMarker></KeyMarker> and <VersionIdMarker></VersionIdMarker> as present-but-empty elements. Recorded behavior only. Emulator: the elements are omitted entirely. Why: XML/SDK consumers that read these as required strings get null where AWS gives ''. Fix: serialize the echo members unconditionally. Traces: s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions#task#7, s3-lsd-versions-view-delimiter-paging#task#5.

Cause 4 of 4: a delete marker minted for a directory-style key loses its trailing slash and its IsLatest flag. AWS: deleting the key 'sourc/' mints a delete marker whose Key is 'sourc/' with IsLatest true. Recorded behavior only. Emulator: the entry is keyed 'sourc' (trailing slash stripped) and IsLatest is 'false', so the marker names a different object and is not reported as current. Why: a client purging a prefix never sees the marker it just created and the key it must purge is wrong. Fix: the DeleteObject delete-marker minting path in state (preserve the key verbatim and set IsLatest on the newest version of that key). Traces: s3-mvs3-delete-folder-key-is-not-recursive#task#12, s3-mvb-delete-wrong-key-mints-marker#task#9.

Also bundled: ListObjectVersions accepts version-id-marker without key-marker. AWS: that combination is refused 400 InvalidArgument 'A version-id marker cannot be specified without a key marker.' with ArgumentName 'version-id-marker' and ArgumentValue the supplied marker. Recorded behavior only. Emulator: answers 200 with a listing. Why: a hand-rolled pager silently gets an unanchored page instead of an error. Fix: the ListObjectVersions request validator. Traces: s3-mbes-traj-hand-rolled-version-paging#task#8.

## Evidence
Replay of simparity-wire-aws-v1. Representative s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions#task#7 (ListObjectVersions, 200/200): $.ListVersionsResult.Prefix / KeyMarker / VersionIdMarker missing (expected ''), and for each of three keys ChecksumAlgorithm missing (expected 'CRC32'), ChecksumType missing (expected 'FULL_OBJECT'), Owner missing. s3-mvs3-delete-folder-key-is-not-recursive#task#12: DeleteMarker[1].Key value_mismatch (expected 'sourc/', got 'sourc'), DeleteMarker[1].IsLatest (expected 'true', got 'false'), Owner missing on all four marker entries. s3-mbes-traj-hand-rolled-version-paging#task#8: status expected 400, got 200; expected Error InvalidArgument 'A version-id marker cannot be specified without a key marker.'

## Steps to Reproduce
1. Replay s3-agent-scripts-seed-reads-back-byte-identical-and-names-each-scripts-actions and compare step task#7's ListVersionsResult with the recording. 2. Replay s3-mvs3-delete-folder-key-is-not-recursive step task#12 and inspect the delete-marker key/IsLatest. 3. Replay s3-mbes-traj-hand-rolled-version-paging step task#8 with version-id-marker and no key-marker.
