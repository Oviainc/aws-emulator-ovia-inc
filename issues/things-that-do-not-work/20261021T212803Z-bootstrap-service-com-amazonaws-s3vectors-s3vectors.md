Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the S3 Vectors (s3vectors) emulator

_Filed: 2026-10-21T21:28:03Z_
_Operation: service-com-amazonaws-s3vectors-s3vectors | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 21 operations in `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the crate holds no state. Nothing works end-to-end.

Implement a FIRST, SMALL, stateful slice: the create -> use -> read -> delete lifecycle of the primary resources (vector bucket -> vector index -> vectors). Protocol is `restJson1`, sigv4 signing name `s3vectors` (OPERATIONS.md 'Wire protocol').

Happy-path operations, in order:

1. **CreateVectorBucket** (`POST /CreateVectorBucket`) — input `vectorBucketName` (required, len 3..63), optional `encryptionConfiguration`, `tags`. Must register a new bucket in state with a creation timestamp and return the generated `vectorBucketArn` (required output) in the modeled form `arn:aws:s3vectors:<region>:<account>:bucket/<name>`. Re-creating an existing name must raise `ConflictException` (409), not a second bucket.
2. **CreateIndex** (`POST /CreateIndex`) — input `vectorBucketName` or `vectorBucketArn`, plus required `indexName` (3..63), `dataType` (`float32`), `dimension` (1..4096), `distanceMetric` (`cosine`|`euclidean`); optional `metadataConfiguration`, `encryptionConfiguration`, `tags`. Must resolve the parent bucket from state (`NotFoundException` 404 when it does not exist), store the index under that bucket, and return the generated `indexArn` (`.../bucket/<bucket>/index/<index>`). Duplicate index name -> `ConflictException` (409).
3. **PutVectors** (`POST /PutVectors`) — input bucket+index (by name or `indexArn`) and `vectors` (list<PutInputVector>, len 1..500, each `key` 1..1024 required, `data` (VectorData union, member `float32`: list<float>) required, optional `metadata` document). Must UPSERT each vector by `key` into the index's vector map; a put with an existing key replaces data and metadata. Unknown bucket/index -> `NotFoundException` (404). Vector length not equal to the index's stored `dimension` must be rejected (ValidationException/400), since the model documents that vector dimensions must match the index dimension count.
4. **GetVectors** (`POST /GetVectors`) — input bucket+index, `keys` (len 1..100, required), `returnData` (default false), `returnMetadata` (default false). Must return `vectors` (list<GetOutputVector>: `key`, optional `data`, optional `metadata`) read back from the stored vectors; `data`/`metadata` must be populated ONLY when the corresponding flag is true, and keys that do not exist must simply be absent from the result list.
5. **QueryVectors** (`POST /QueryVectors`) — input bucket+index, required `topK` (>=1) and `queryVector` (VectorData), optional `filter` (document), `queryMode`, `returnMetadata`/`returnDistance` (default false). Must COMPUTE similarity against the vectors persisted in the index using the index's `distanceMetric` (cosine distance = 1 - cosine similarity; euclidean = L2 distance), sort ascending by distance, truncate to `topK`, and return `vectors` (list<QueryOutputVector>: `key`, optional `distance` only when `returnDistance`, optional `metadata` only when `returnMetadata`) plus the required `distanceMetric` echoed FROM the stored index definition (not from the request). Query against a dimension-mismatched `queryVector` -> 400.
6. **DeleteIndex** / **DeleteVectorBucket** (`POST /DeleteIndex`, `POST /DeleteVectorBucket`) — accept name or ARN, remove the index (with all its vectors) / the bucket from state, and make subsequent `GetIndex`/`GetVectorBucket`/`QueryVectors` on it return `NotFoundException` (404). Deleting a non-empty bucket must fail with `ConflictException` (409) rather than silently dropping its indexes.

Supporting reads needed to verify the loop (implement with the slice, they are a few lines once state exists): **GetVectorBucket** (returns `vectorBucket`: VectorBucket{vectorBucketName, vectorBucketArn, creationTime, encryptionConfiguration, defaultIndexMode}), **GetIndex** (returns `index`: Index{vectorBucketName, indexName, indexArn, creationTime, dataType, dimension, distanceMetric, metadataConfiguration, encryptionConfiguration, indexMode}), and **ListIndexes** / **ListVectorBuckets** / **ListVectors** honouring `prefix`, `maxResults` and a real `nextToken` cursor derived from stored order.

NEVER STUBS: every operation in this slice must compute its response from persisted state. Hardcoded ARNs without a stored bucket/index, echoed inputs, empty `vectors: []` on a populated index, or random IDs that are never stored are all failures. ARNs returned by create must be the same ARNs the get/list operations report, and must be accepted wherever the model allows an ARN in place of a name.

## State Model

- `S3VectorsState { buckets: BTreeMap<String /* vectorBucketName */, VectorBucket>, policies: BTreeMap<String /* bucketName */, String /* policy JSON */>, tags: BTreeMap<String /* resourceArn */, BTreeMap<String,String>> }`
- `VectorBucket { name: String, arn: String, creation_time: SystemTime, encryption_configuration: Option<EncryptionConfiguration>, default_index_mode: Option<IndexMode>, indexes: BTreeMap<String /* indexName */, VectorIndex> }`
- `VectorIndex { bucket_name: String, name: String, arn: String, creation_time: SystemTime, data_type: DataType /* float32 */, dimension: u32, distance_metric: DistanceMetric /* cosine|euclidean */, metadata_configuration: Option<MetadataConfiguration>, encryption_configuration: Option<EncryptionConfiguration>, index_mode: IndexMode, vectors: BTreeMap<String /* vector key */, StoredVector> }`
- `StoredVector { key: String, data: Vec<f32>, metadata: Option<serde_json::Value> }`
- Lookup helpers: resolve (vectorBucketName | vectorBucketArn) -> &VectorBucket and (vectorBucketName+indexName | indexArn) -> &VectorIndex, returning `NotFoundException` when absent; vector keys are stored in a `BTreeMap` so ListVectors ordering and `nextToken` paging are deterministic.

The remaining operations (PutVectorBucketPolicy / GetVectorBucketPolicy / DeleteVectorBucketPolicy, PutVectorBucketDefaultIndexMode, UpdateIndexMode, DeleteVectors, TagResource / UntagResource / ListTagsForResource, segmented ListVectors) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for service-com-amazonaws-s3vectors-s3vectors lists all 21 operations with routes, required members and errors (CreateVectorBucket -> required `vectorBucketArn` output; CreateIndex -> required `dimension`/`distanceMetric`/`dataType` and `indexArn` output; PutVectors -> required `vectors` list<PutInputVector> 1..500; GetVectors -> required `keys` 1..100 with `returnData`/`returnMetadata` defaulting to false; QueryVectors -> required `topK`, `queryVector`, and required output `distanceMetric`; DeleteIndex/DeleteVectorBucket -> NotFoundException/ConflictException). Smithy shapes VectorBucket, Index, PutInputVector, GetOutputVector, QueryOutputVector and the VectorData union (member `float32`) give the exact field sets quoted in the State Model, and PutInputVector.data documents that 'Vector dimensions must match the dimension count that's configured for the vector index'. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-s3vectors-s3vectors/src/ops/` matches all 21 handler files, so nothing is implemented and no test asserts any behaviour.

## Steps to Reproduce

1. aws s3vectors create-vector-bucket --vector-bucket-name demo-vectors  # expect 200 with a vectorBucketArn; today 501 NotImplemented
2. aws s3vectors create-index --vector-bucket-name demo-vectors --index-name demo-index --data-type float32 --dimension 3 --distance-metric cosine  # expect 200 with indexArn
3. aws s3vectors get-index --vector-bucket-name demo-vectors --index-name demo-index  # expect the index as created: dimension 3, distanceMetric cosine, creationTime
4. aws s3vectors put-vectors --vector-bucket-name demo-vectors --index-name demo-index --vectors '[{"key":"v1","data":{"float32":[1,0,0]},"metadata":{"color":"red"}},{"key":"v2","data":{"float32":[0,1,0]}}]'  # expect 200
5. aws s3vectors get-vectors --vector-bucket-name demo-vectors --index-name demo-index --keys v1 --return-data --return-metadata  # expect v1 with data [1,0,0] and metadata {color: red}
6. aws s3vectors query-vectors --vector-bucket-name demo-vectors --index-name demo-index --top-k 2 --query-vector '{"float32":[1,0,0]}' --return-distance --return-metadata  # expect v1 first with distance ~0, then v2, and distanceMetric cosine
7. aws s3vectors list-vectors --vector-bucket-name demo-vectors --index-name demo-index --max-results 1  # expect 1 vector plus a nextToken that returns the other on the next call
8. aws s3vectors put-vectors --vector-bucket-name demo-vectors --index-name demo-index --vectors '[{"key":"v1","data":{"float32":[0,0,1]}}]' then get-vectors --keys v1 --return-data  # expect the upserted data [0,0,1], still exactly 2 vectors in the index
9. aws s3vectors put-vectors --vector-bucket-name demo-vectors --index-name demo-index --vectors '[{"key":"bad","data":{"float32":[1,2]}}]'  # expect a 400 validation error (dimension mismatch), not 200
10. aws s3vectors delete-vector-bucket --vector-bucket-name demo-vectors  # expect ConflictException 409 while demo-index still exists
11. aws s3vectors delete-index --vector-bucket-name demo-vectors --index-name demo-index then delete-vector-bucket --vector-bucket-name demo-vectors  # expect 200s, and a following get-index / get-vector-bucket to return NotFoundException 404
