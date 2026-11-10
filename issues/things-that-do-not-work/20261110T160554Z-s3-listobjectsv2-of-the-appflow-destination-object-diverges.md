Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-s3-to-s3-flow-run-transfers-records
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-s3-to-s3-flow-run-transfers-records
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents.ChecksumAlgorithm
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents.ChecksumType
Issue-Related: 20261102T111455Z-s3-listings-percent-encode-the-key-delimiter-under.md
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# s3 ListObjectsV2 of the AppFlow destination object diverges: extra key segment and no per-object checksum fields

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
ListObjectsV2 over the bucket/prefix that an AppFlow S3-to-S3 flow run wrote returns a listing that diverges from the recorded AWS listing in two independent ways. Both are observed on the same replayed step (fixture appflow-s3-to-s3-flow-run-transfers-records, step task#9), which lists the flow's destination prefix after a successful run.

Cause 1 of 2: the destination object is listed under an extra key segment.
AWS: recorded behavior only. The recording shows an AppFlow S3 destination run depositing exactly ONE object whose key is <destinationPrefix>/<flowName>/<executionId> — the prefix, the flow name and the flow-execution id, with NO further path segment and NO file-name suffix appended (the recorded Contents.Key ends at the execution-id segment). Smithy: ListObjectsV2 -> Object.Key (com.amazonaws.s3#ObjectKey, @length min 1) is echoed verbatim from the stored key, so the listing simply reports whatever key the writer chose.
Emulator: the listed key carries an extra segment and a suffix — it repeats the execution id as a child path element with a '.json' extension, i.e. <destinationPrefix>/<flowName>/<executionId>/<executionId>.json instead of <destinationPrefix>/<flowName>/<executionId>. Replay reports $.ListBucketResult.Contents.Key value_mismatch, expected the three-segment key, got the four-segment key with the .json suffix.
Why: a client that derives the output object key from the flow name and the execution id it got back from DescribeFlowExecutionRecords/StartFlow — the documented way to locate a run's output — does a GetObject/HeadObject on the AWS-shaped key and gets NoSuchKey, and any consumer keyed on the exact object path (a bucket-notification route, a downstream Glue/Athena location, a key-equality policy condition) never matches. Listings also show one more directory level than AWS, so prefix/delimiter walks differ in depth.
Fix: the AppFlow flow-run S3 destination writer's output-key composition (the component that stores the run's object into S3 state) — it must store the object at the prefix/flow-name/execution-id key rather than nesting a per-execution file under it; the ListObjectsV2 handler then reports it unchanged.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#9.

Cause 2 of 2: listing entries omit the object's checksum algorithm and checksum type.
AWS: Smithy: ListObjectsV2 output Object carries ChecksumAlgorithm (com.amazonaws.s3#ChecksumAlgorithmList, xmlFlattened, values include CRC64NVME) — 'The algorithm that was used to create a checksum of the object' — and ChecksumType (com.amazonaws.s3#ChecksumType, enum COMPOSITE | FULL_OBJECT) — 'The checksum type that is used to calculate the object's checksum value'. The recording shows AWS reporting ChecksumAlgorithm CRC64NVME and ChecksumType FULL_OBJECT for the single-part object written by the flow run, which matches S3's default full-object CRC64NVME checksum for a normal PutObject-style write.
Emulator: both members are absent from every Contents entry. Replay reports $.ListBucketResult.Contents.ChecksumAlgorithm missing (expected 'CRC64NVME', got None) and $.ListBucketResult.Contents.ChecksumType missing (expected 'FULL_OBJECT', got None). The emulator either does not compute/retain a checksum for service-written objects or does not project it into the listing.
Why: a client that uses the listing to decide whether an object needs integrity verification, or to pick the checksum algorithm to request on GetObject, sees an object with no checksum at all; integrity-aware tooling (SDK checksum validation, data-pipeline verification steps) treats the object as unchecksummed and silently skips validation, and code branching on ChecksumType FULL_OBJECT vs COMPOSITE takes the wrong branch.
Fix: S3 object state must record the write's checksum algorithm and type (full-object CRC64NVME by default for a single-part write, including objects written by service integrations such as AppFlow), and the ListObjectsV2 serializer must emit the flattened ChecksumAlgorithm list and the ChecksumType element for each Contents entry.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#9.

Not covered elsewhere: the open issues 20261109T175957Z-s3-getobject-serves-the-appflow-written-object-with, 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata and 20261110T030759Z-s3-getobject-omits-the-crc32-full-object-checksum all concern GetObject response headers/body, not the ListObjectsV2 listing projection or the stored key layout; 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix concerns the echoed Prefix element only.

## Evidence
Replayed step: fixture appflow-s3-to-s3-flow-run-transfers-records, step task#9, service s3, operation ListObjectsV2, verdict mismatch, expected_status 200, actual_status 200 (status matches; the body diverges).

Expected (recorded AWS) vs actual (emulator):
- $.ListBucketResult.Contents.Key: expected '<prefix>/<flowName>/<executionId>' (recorded: 'output/sim2real-probe-appflow-flow-de056d02/d5b02e1e-a5a8-4c41-ad4c-42c012a5058f'); actual the same key plus an extra '/<executionId>.json' segment (recorded: '.../d5b02e1e-a5a8-4c41-ad4c-42c012a5058f/d5b02e1e-a5a8-4c41-ad4c-42c012a5058f.json'). The bucket name, prefix, flow name and execution id are per-run values; the requirement is the SHAPE of the key: exactly <destinationPrefix>/<flowName>/<executionId>, with no additional segment and no appended extension.
- $.ListBucketResult.Contents.ChecksumAlgorithm: expected 'CRC64NVME', actual absent.
- $.ListBucketResult.Contents.ChecksumType: expected 'FULL_OBJECT', actual absent.

Smithy citations: Object.Key -> ObjectKey (@length min 1); Object.ChecksumAlgorithm -> ChecksumAlgorithmList (xmlFlattened; CRC32|CRC32C|SHA1|SHA256|CRC64NVME|...); Object.ChecksumType -> ChecksumType enum (COMPOSITE|FULL_OBJECT).

## Steps to Reproduce
1. Create the source and destination buckets and seed the source object(s) as the fixture does.
2. Create an AppFlow S3-to-S3 flow whose destination connector properties set a bucket prefix (recorded: 'output'), then StartFlow and wait for the execution to finish.
3. Call S3 ListObjectsV2 on the destination bucket with prefix '<destinationPrefix>/<flowName>' (fixture appflow-s3-to-s3-flow-run-transfers-records, step task#9).
4. Compare the single Contents entry with the recorded response: the emulator's Contents.Key carries an extra '/<executionId>.json' segment beyond '<prefix>/<flowName>/<executionId>', and the entry has no ChecksumAlgorithm (expected CRC64NVME) and no ChecksumType (expected FULL_OBJECT).
5. Re-replay the fixture step to confirm: the Contents.Key must equal the three-segment key and the entry must carry both checksum members.
