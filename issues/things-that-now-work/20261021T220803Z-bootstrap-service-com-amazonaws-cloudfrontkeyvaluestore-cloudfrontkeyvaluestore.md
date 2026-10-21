Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the CloudFront KeyValueStore emulator

_Filed: 2026-10-21T22:08:03Z_
_Operation: service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client can do anything with the service. Implement a first, small, stateful slice covering the key-value lifecycle of a key value store, in this order:

1. **DescribeKeyValueStore** — `GET /key-value-stores/{KvsARN}`. Input: `KvsARN` (@httpLabel). Output must be computed from the stored store: `KvsARN`, `Created` (timestamp set when the store was first materialised), `LastModified` (timestamp of the last mutating call), `ItemCount` (number of keys actually held), `TotalSizeInBytes` (sum of key+value byte lengths), `Status`, and the `ETag` response header carrying the store's CURRENT etag. Not a canned struct: after two PutKeys, `ItemCount` must be 2.
2. **PutKey** — `PUT /key-value-stores/{KvsARN}/keys/{Key}`. Input: `KvsARN`, `Key` (@httpLabel), `Value` (body, required), `IfMatch` (@httpHeader If-Match, required). Must insert/overwrite the key in the store's map, bump the store etag to a NEW value, update `LastModified`, and return the recomputed `ItemCount`/`TotalSizeInBytes` plus the new `ETag` header. An `IfMatch` that does not equal the store's current etag must be rejected with the modeled conflict error (`ConflictException`, HTTP 409) and must NOT mutate state — optimistic concurrency via ETag is the central contract of this API.
3. **GetKey** — `GET /key-value-stores/{KvsARN}/keys/{Key}`. Must return the exact `Key`/`Value` previously written (byte-for-byte, no echo of the request path only) plus the store's current `ItemCount`/`TotalSizeInBytes`. Unknown key → `ResourceNotFoundException` (HTTP 404).
4. **ListKeys** — `GET /key-value-stores/{KvsARN}/keys`. Must enumerate the stored pairs as `Items: list<ListKeysResponseListItem{Key, Value}>`, honour `MaxResults` (@httpQuery, default 10) as a page bound, and return an opaque `NextToken` when more keys remain that, when passed back as `NextToken`, resumes after the last returned key. A single page that ignores MaxResults/NextToken is not acceptable.
5. **DeleteKey** — `DELETE /key-value-stores/{KvsARN}/keys/{Key}` with required `IfMatch`. Must remove the key, bump the etag, update `LastModified`, and return the recomputed `ItemCount`/`TotalSizeInBytes` + new `ETag` header. A following GetKey for that key must then return `ResourceNotFoundException` (404), and DescribeKeyValueStore must show the decremented `ItemCount`.
6. **UpdateKeys** — `POST /key-value-stores/{KvsARN}/keys` with required `IfMatch`, `Puts: list<PutKeyRequestListItem{Key,Value}>`, `Deletes: list<DeleteKeyRequestListItem{Key}>`. Must apply the whole batch atomically under one etag check (all-or-nothing), then return recomputed counts and a single new `ETag`.

Because the store itself is created by the CloudFront service (`CreateKeyValueStore`), this crate should lazily materialise a store record on first reference to a well-formed key-value-store ARN (`arn:aws:cloudfront::<account>:key-value-store/<id>`), and return `ResourceNotFoundException` (404) for an ARN that is not of that form. Validation that must be enforced rather than silently accepted: `Key` length 1..1024, `KvsARN` length 1..2048, `Value` required, and a missing `If-Match` header on PutKey/DeleteKey/UpdateKeys → `ValidationException` (HTTP 400).

No operation may return a hardcoded, zeroed, or echoed response: every field above (ItemCount, TotalSizeInBytes, ETag, Items, Value, Created/LastModified) must be derived from the persisted state in the shared engine, so a create → read → list → delete → verify-gone sequence observes consistent values across calls.

## State Model

In `src/state.rs`, on the service state struct:

- `stores: HashMap<String /* KvsARN */, KeyValueStore>`
- `struct KeyValueStore {`
  - `kvs_arn: String,`
  - `keys: BTreeMap<String /* Key */, String /* Value */>`  — BTreeMap so ListKeys has a stable, resumable ordering for NextToken,
  - `etag: String,`  — opaque, regenerated on every successful mutation (PutKey / DeleteKey / UpdateKeys),
  - `created: SystemTime,`
  - `last_modified: SystemTime,`
  - `status: String,`  — e.g. "READY",
  - `failure_reason: Option<String>,`
- `}`

Derived on read, never stored stale: `item_count = keys.len()`, `total_size_in_bytes = keys.iter().map(|(k,v)| k.len() + v.len()).sum()`.
ListKeys pagination token: an opaque encoding of the last returned key, resumed with `keys.range(last_key.., ..).skip(1)`.

Operations outside this slice stay 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-cloudfrontkeyvaluestore-cloudfrontkeyvaluestore/OPERATIONS.md` lists all 6 operations with the routes, members and errors cited above (DeleteKey, DescribeKeyValueStore, GetKey, ListKeys, PutKey, UpdateKeys; protocol restJson1, signing name `cloudfront-keyvaluestore`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all six handler files (delete_key.rs, describe_key_value_store.rs, get_key.rs, list_keys.rs, put_key.rs, update_keys.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. Shapes `ListKeysResponseListItem{Key,Value}`, `PutKeyRequestListItem{Key,Value}` and `DeleteKeyRequestListItem{Key}` come from the Smithy model (smithy_query). ListKeys is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Items)` with `MaxResults` default 10; PutKey/DeleteKey/UpdateKeys all declare a required `If-Match` header and a `ConflictException` (409), which is the modeled basis for ETag-based optimistic concurrency. No tests assert any of this behaviour today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `cloudfront-keyvaluestore` client at it with KvsARN = arn:aws:cloudfront::123456789012:key-value-store/11111111-2222-3333-4444-555555555555.
2. Call DescribeKeyValueStore → expect 200 with ItemCount=0, TotalSizeInBytes=0, a Created timestamp and an ETag header (today: HTTP 501).
3. Call PutKey(Key="color", Value="blue", IfMatch=<etag from step 2>) → expect 200 with ItemCount=1, TotalSizeInBytes>0 and a NEW ETag header.
4. Call PutKey(Key="size", Value="large", IfMatch=<stale etag from step 2>) → expect ConflictException (HTTP 409) and the store unchanged.
5. Call PutKey(Key="size", Value="large", IfMatch=<current etag>) → expect 200, ItemCount=2.
6. Call GetKey(Key="color") → expect 200 with Value="blue" and ItemCount=2.
7. Call ListKeys(MaxResults=1) → expect 200 with exactly 1 item and a NextToken; call again with that NextToken → expect the remaining item and no NextToken.
8. Call DeleteKey(Key="color", IfMatch=<current etag>) → expect 200 with ItemCount=1 and a new ETag; then GetKey(Key="color") → expect ResourceNotFoundException (HTTP 404).
9. Call UpdateKeys(IfMatch=<current etag>, Puts=[{a,1},{b,2}], Deletes=[{size}]) → expect 200 with ItemCount=2; DescribeKeyValueStore must report ItemCount=2 and the same ETag returned by UpdateKeys.
