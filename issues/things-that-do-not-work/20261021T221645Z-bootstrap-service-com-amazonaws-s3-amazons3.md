Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d7bdea90bc70459c4b589287302dc4eaf39117d

# Bootstrap the Amazon S3 emulator

_Filed: 2026-10-21T22:16:45Z_
_Operation: service-com-amazonaws-s3-amazons3 | Class: missing_

> _Note: this issue was identified against the code base at commit `7d7bdea90bc70459c4b589287302dc4eaf39117d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 112 operations in `services/service-com-amazonaws-s3-amazons3/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. No client can do anything with S3 today.

This issue scopes the FIRST, SMALL slice: the core object-storage loop, implemented statefully over the restXml protocol (per OPERATIONS.md: protocol `restXml`, sigv4 signing name `s3`, endpoint prefix `s3`). All six operations below must compute their responses from persisted state in the shared engine store reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back, no generating an ETag/VersionId without storing it.

### Happy path to implement (in order)

1. **CreateBucket** — `PUT /{Bucket}` -> 200. Input: `Bucket` (@httpLabel, required), `CreateBucketConfiguration` (@httpPayload, holds `LocationConstraint`), `ACL`, `ObjectLockEnabledForBucket`. Must insert a new bucket record keyed by name and return the `Location` header (`/{Bucket}`). A second CreateBucket for a name that already exists must fail with the modeled `BucketAlreadyOwnedByYou` (HTTP 409) rather than silently succeeding; `BucketAlreadyExists` (409) is also modeled.
2. **PutObject** — `PUT /{Bucket}/{Key+}?x-id=PutObject` -> 200. Input: `Body` (blob @httpPayload, required), `Bucket`, `Key` (len 1.., required), `ContentType`, `ContentLength`, `Metadata` (@httpPrefixHeaders `x-amz-meta-`), `StorageClass`, `CacheControl`, `ContentEncoding`. Must store the exact body bytes plus the user metadata under (bucket, key), overwriting any previous object at that key, compute the **ETag as the hex MD5 of the stored body wrapped in double quotes**, and return it in the `ETag` response header. Writing to a bucket that was never created must not create it implicitly — return `NoSuchBucket` (404, the common S3 error shape).
3. **GetObject** — `GET /{Bucket}/{Key+}?x-id=GetObject` -> 200. Must return the stored bytes in `Body`, with `ContentLength` equal to the stored byte length, the same `ETag` PutObject returned, the `LastModified` timestamp recorded at write time, the stored `ContentType`, and the stored `Metadata` echoed back as `x-amz-meta-*` headers. A key that was never written (or was deleted) must produce the modeled `NoSuchKey` (HTTP 404); a missing bucket must produce `NoSuchBucket` (404).
4. **ListObjectsV2** — `GET /{Bucket}?list-type=2` -> 200. Must enumerate the keys actually stored in that bucket, **sorted ascending by key (UTF-8 byte order)**, filling each `Contents` entry's `Key`, `Size`, `ETag`, `LastModified` and `StorageClass` from the stored object. Must honour `Prefix` (filter), `MaxKeys` (truncate; set `IsTruncated=true` and `NextContinuationToken`), `ContinuationToken` (resume), `StartAfter`, and `Delimiter` (roll keys up into `CommonPrefixes`). Must set `Name` to the bucket and `KeyCount` to the number of entries returned (`Contents` + `CommonPrefixes`). Unknown bucket -> `NoSuchBucket` (404).
5. **DeleteObject** — `DELETE /{Bucket}/{Key+}?x-id=DeleteObject` -> 204. Must remove the (bucket, key) entry so a following GetObject answers `NoSuchKey` and the key disappears from ListObjectsV2. Deleting a key that does not exist is a 204 success (S3 delete is idempotent).
6. **DeleteBucket** — `DELETE /{Bucket}` -> 204. Must remove the bucket record; a bucket that still has objects must be refused with `BucketNotEmpty` (409, common error shape), and a nonexistent bucket with `NoSuchBucket` (404). After a successful DeleteBucket, HeadBucket/GetObject on it must 404.

## State Model

All of this lives in `ComAmazonawsS3Amazons3State` (`src/state.rs`), reached through the ONE shared engine (never `InMemoryEngine::new()` inside a handler):

```
struct S3State {
    buckets: BTreeMap<String /*bucket name*/, Bucket>,
}

struct Bucket {
    name: String,
    creation_date: DateTime,          // for ListBuckets later
    location_constraint: Option<String>,  // from CreateBucketConfiguration
    objects: BTreeMap<String /*key*/, S3Object>,  // BTreeMap => lexicographic list order for free
}

struct S3Object {
    key: String,
    body: Vec<u8>,                    // exact bytes from PutObject.Body
    etag: String,                     // "\"<hex md5 of body>\""
    size: u64,                        // body.len()
    last_modified: DateTime,          // set on each write
    content_type: Option<String>,
    cache_control: Option<String>,
    content_encoding: Option<String>,
    metadata: BTreeMap<String, String>,  // x-amz-meta-* user metadata
    storage_class: Option<StorageClass>, // default STANDARD
}
```

Nothing else is in scope. Versioning, multipart upload, tagging, ACLs, CORS, lifecycle, replication, notifications, Object Lock, directory buckets and the ~100 remaining operations stay at 501 and will be filed as individual gap issues later — do not grow this first slice.

Done = a realistic client script (aws CLI / boto3 / Rust SDK against the emulator endpoint) runs the six calls above end-to-end, and the bytes, ETag, size, metadata and key listing it reads back are the ones it wrote — not just 2xx status codes.

## Evidence

1. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-s3-amazons3/src/ops/` matches every handler file, including `create_bucket.rs`, `put_object.rs`, `get_object.rs`, `list_objects_v2.rs`, `delete_object.rs` and `delete_bucket.rs`; each returns `Scaffolded::not_implemented(..)` => HTTP 501, so the service holds zero state.
2. `services/service-com-amazonaws-s3-amazons3/OPERATIONS.md` grounds every field named above: CreateBucket (line 47-52: route `PUT /{Bucket}`, `CreateBucketConfiguration` payload, `Location`/`BucketArn` output, errors `BucketAlreadyExists`/`BucketAlreadyOwnedByYou` 409); PutObject (line 689-694: `Body` blob @httpPayload required, `Metadata` @httpPrefixHeaders(x-amz-meta-), `ETag` output header); GetObject (line 383-388: `Body` required output, `ContentLength`, `ETag`, `LastModified`, `Metadata`, errors `NoSuchKey` 404 / `InvalidObjectState` 403); ListObjectsV2 (line 533-539: `Prefix`, `MaxKeys`, `ContinuationToken`, `StartAfter`, `Delimiter` inputs; `Contents`, `Name`, `KeyCount`, `IsTruncated`, `NextContinuationToken`, `CommonPrefixes` outputs; error `NoSuchBucket` 404); DeleteObject (line 187-192: `DELETE ... -> 204`); DeleteBucket (line 82-87: `DELETE /{Bucket} -> 204`).
3. `README.md` documents the wire protocol (`restXml`) and that the shared store must be reached via `self.engine` / `crate::state::service_state`.
4. Well-established S3 semantics grounding the behavioural requirements: ETag of a single-part PUT is the quoted hex MD5 of the object bytes; ListObjectsV2 returns keys in lexicographic UTF-8 order; DeleteObject is idempotent (204 even for a missing key); DeleteBucket on a non-empty bucket fails with BucketNotEmpty.
5. No tests exist for these operations beyond `tests/state_roundtrip.rs`, which only exercises the state API.

## Steps to Reproduce

1. Start the emulator and point an S3 client at it with path-style addressing (e.g. `aws --endpoint-url http://localhost:<port> s3api ...`).
2. `CreateBucket` with Bucket=`demo-bucket` -> expect 200 with a `Location: /demo-bucket` header. Today: HTTP 501 NotImplemented.
3. `CreateBucket` again with the same name -> expect 409 `BucketAlreadyOwnedByYou`.
4. `PutObject` Bucket=`demo-bucket`, Key=`a/hello.txt`, Body=`hello world`, ContentType=`text/plain`, metadata `x-amz-meta-owner: alice` -> expect 200 with `ETag: "5eb63bbbe01eeed093cb22bb8f5acdc3"` (quoted hex MD5 of the body).
5. `GetObject` Bucket=`demo-bucket`, Key=`a/hello.txt` -> expect 200, body exactly `hello world`, `Content-Length: 11`, the same ETag, a `Last-Modified` timestamp, `Content-Type: text/plain`, and `x-amz-meta-owner: alice`.
6. `PutObject` a second key `b/other.txt`, then `ListObjectsV2` Bucket=`demo-bucket` -> expect `Name=demo-bucket`, `KeyCount=2`, Contents listing `a/hello.txt` then `b/other.txt` with correct Size/ETag/LastModified, `IsTruncated=false`.
7. `ListObjectsV2` with `prefix=a/` -> expect only `a/hello.txt`; with `max-keys=1` -> expect one entry, `IsTruncated=true` and a `NextContinuationToken` that returns the second key when passed back as `continuation-token`.
8. `GetObject` Bucket=`demo-bucket`, Key=`missing.txt` -> expect 404 `NoSuchKey`.
9. `DeleteObject` Bucket=`demo-bucket`, Key=`a/hello.txt` -> expect 204; a following `GetObject` for that key -> 404 `NoSuchKey`, and `ListObjectsV2` -> `KeyCount=1`.
10. `DeleteBucket` Bucket=`demo-bucket` while `b/other.txt` still exists -> expect 409 `BucketNotEmpty`; delete `b/other.txt`, then `DeleteBucket` -> 204, and a following `ListObjectsV2`/`HeadBucket` on `demo-bucket` -> 404 `NoSuchBucket`.
