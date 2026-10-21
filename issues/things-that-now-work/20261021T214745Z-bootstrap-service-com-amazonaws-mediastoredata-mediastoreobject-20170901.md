Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the MediaStore Data emulator

_Filed: 2026-10-21T21:47:45Z_
_Operation: service-com-amazonaws-mediastoredata-mediastoreobject-20170901 | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no MediaStore Data client workflow works at all. Implement this minimal happy-path slice — the object lifecycle of the service's single primary resource (the MediaStore object) — statefully, in this order:

1. **PutObject** — `PUT /{Path+}`. Input: `Body` (blob, @httpPayload, required), `Path` (@httpLabel, required, len 1..900, pattern `^(?:[A-Za-z0-9_\.\-\~]+/){0,10}[A-Za-z0-9_\.\-\~]+$`), `ContentType` (Content-Type header), `CacheControl` (Cache-Control header), `StorageClass` (x-amz-storage-class, only `TEMPORAL`), `UploadAvailability` (x-amz-upload-availability, `STANDARD`|`STREAMING`). Must store the body bytes plus all metadata under the normalized path, and return `ContentSHA256` (the real lowercase/uppercase hex SHA-256 of the stored body, 64 hex chars), `ETag` (hex, derived from the stored content, stable for identical content) and the effective `StorageClass`. Re-PUT of the same path overwrites the object and yields a new ETag/ContentSHA256/LastModified.
2. **DescribeObject** — `HEAD /{Path+}`. Returns headers computed from the stored object: `ETag`, `Content-Type`, `Content-Length` (actual stored byte length), `Cache-Control`, `Last-Modified`. Unknown path → `ObjectNotFoundException` (HTTP 404), not 501 and not a hollow 200.
3. **GetObject** — `GET /{Path+}`. Returns `Body` = the exact bytes stored by PutObject, plus `ETag`, `Content-Type`, `Content-Length`, `Cache-Control`, `Last-Modified`, and `StatusCode` (@httpResponseCode) = 200. When the `Range` header is present (pattern `^bytes=(?:\d+\-\d*|\d*\-\d+)$`), return only that slice with `StatusCode` 206, `Content-Range: bytes <first>-<last>/<total>` and `Content-Length` of the slice; a range whose start is past the end of the object → `RequestedRangeNotSatisfiableException` (HTTP 416). Unknown path → `ObjectNotFoundException` (404).
4. **ListItems** — `GET /` with `Path`, `MaxResults` (1..1000) and `NextToken` query params. Must enumerate the stored objects under the requested folder prefix from state: one `Item` per object (name, Type=OBJECT, ETag, ContentType, ContentLength, LastModified) and one `Item` per immediate sub-folder (Type=FOLDER). Must honour `MaxResults` as a hard page bound and return an opaque `NextToken` when more items remain; passing that token back must continue the listing without repeats or omissions, and must return no `NextToken` on the final page.
5. **DeleteObject** — `DELETE /{Path+}`. Removes the object from state; a subsequent DescribeObject/GetObject on that path must return `ObjectNotFoundException` (404), and ListItems must no longer include it. Deleting an absent path → `ObjectNotFoundException` (404).

All five must compute their responses from persisted shared state reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoed inputs, no random ETag that is not stored, no empty `Items` list. A PutObject followed by GetObject must return byte-identical content, and the ETag/ContentSHA256 reported by PutObject must match what DescribeObject/GetObject/ListItems later report. Request-shape rules in the model must be enforced rather than ignored: the `Path` length/pattern constraints and the `StorageClass`/`UploadAvailability` enum values are validation failures (HTTP 400), and a request naming a container that does not exist in state surfaces `ContainerNotFoundException` (404).

## State Model

In `src/state.rs`, on the service state struct:

- `objects: BTreeMap<String, StoredObject>` — key is the normalized object path (leading `/` stripped, e.g. `folder/file.jpg`); `BTreeMap` so ListItems has a deterministic lexicographic order for stable pagination.
- `StoredObject { body: Vec<u8>, content_type: Option<String>, cache_control: Option<String>, storage_class: String /* "TEMPORAL" */, upload_availability: Option<String>, etag: String /* hex */, content_sha256: String /* 64 hex chars */, content_length: i64, last_modified: SystemTime }`.
- `list_tokens: HashMap<String, String>` — opaque NextToken -> the path key to resume after (or encode the resume key directly in the token), so ListItems pagination is continuable across calls.

Operations outside this slice: none — these 5 are the whole service, so this bootstrap slice is the complete first functional version. Anything left unenforced after it (full header-echo fidelity, STREAMING upload availability semantics, container-level behaviours) is filed later as individual gap issues.

## Evidence

OPERATIONS.md for this crate (services/service-com-amazonaws-mediastoredata-mediastoreobject-20170901/OPERATIONS.md) lists exactly 5 operations with restJson1 wire protocol, sdkId `MediaStore Data`: DeleteObject (DELETE /{Path+}), DescribeObject (HEAD /{Path+}), GetObject (GET /{Path+} with Range/ContentRange/StatusCode and RequestedRangeNotSatisfiableException 416), ListItems (GET / with @paginated inputToken=NextToken/outputToken=NextToken, MaxResults range 1..1000), PutObject (PUT /{Path+} with Body @httpPayload required, returning ContentSHA256/ETag/StorageClass). All 5 handler files under src/ops/ still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (grep confirms describe_object.rs, delete_object.rs, put_object.rs, list_items.rs, get_object.rs line 4) and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. No test asserts any stateful behavior; only the scaffold's tests/state_roundtrip.rs exists. ContentSHA256/ETag/Content-Length/Range semantics are grounded in the model's output members and in well-established MediaStore Data object-store behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `mediastore-data` client at it.
2. PutObject(Path="folder/file.txt", Body=b"hello mediastore", ContentType="text/plain", CacheControl="max-age=300", StorageClass="TEMPORAL") — expect 200 with ETag, StorageClass=TEMPORAL and ContentSHA256 = the real SHA-256 hex of the body; today it returns 501.
3. DescribeObject(Path="folder/file.txt") — expect 200 with Content-Length=16, Content-Type=text/plain, Cache-Control=max-age=300, Last-Modified set, and the SAME ETag returned by PutObject.
4. GetObject(Path="folder/file.txt") — expect StatusCode 200 and Body byte-identical to what was put.
5. GetObject(Path="folder/file.txt", Range="bytes=0-4") — expect StatusCode 206, Body=b"hello", Content-Length=5, Content-Range="bytes 0-4/16"; GetObject with Range="bytes=100-" — expect RequestedRangeNotSatisfiableException (HTTP 416).
6. PutObject two more objects ("folder/b.txt", "folder/c.txt"), then ListItems(Path="folder", MaxResults=2) — expect exactly 2 Items with real ETag/ContentLength/LastModified plus a NextToken; call ListItems again with that NextToken — expect the remaining item(s) and no NextToken.
7. DeleteObject(Path="folder/file.txt") — expect 200; then DescribeObject and GetObject on that path must return ObjectNotFoundException (HTTP 404), and ListItems(Path="folder") must no longer list it.
8. DeleteObject(Path="folder/file.txt") again — expect ObjectNotFoundException (HTTP 404).
9. GetObject(Path="never/written.txt") — expect ObjectNotFoundException (HTTP 404), not 501 and not an empty 200.
