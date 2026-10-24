Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Bucket not-found answers HTTP 404 with a bare NotFoundException instead of Lightsail's 400 / code=DoesNotExist envelope

_Filed: 2026-10-24T09:56:37Z_
_Operation: GetBuckets | Class: error-response_

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A request naming a bucket this account does not hold is answered by the bucket handlers with the MODELLED `NotFoundException` variant, which the generated error enum renders at the `@httpError(404)` the model declares, with a body carrying only `message`.

Every other not-found path in this crate answers the envelope the crate itself documents as Lightsail's recorded wire behaviour: HTTP **400**, wire error `NotFoundException`, and the body member `code` set to `DoesNotExist` (plus the message). See `GetInstance`, `GetDisk`, `GetStaticIp`, `GetContainerServices`, `GetDistributions`, `GetLoadBalancer`, `GetRelationalDatabase`, which all go through `crate::scaffolded::Scaffolded::resource_does_not_exist` / `does_not_exist`.

Expected: `GetBuckets` with an unknown `bucketName` answers the same envelope as the rest of the service — status 400 and `code: "DoesNotExist"` in the body — so a Lightsail client that branches on the body's `code` (which `src/scaffolded.rs` states is what it branches on) can tell "already gone" from a transport failure the same way for a bucket as for an instance.

Actual: status 404 and no `code` member at all, so the one field a Lightsail SDK client is documented (in this crate) to key off is absent, and the status differs from every sibling read in the same service.

The same deviation is in the sibling bucket handlers and should be fixed with it: `create_bucket_access_key.rs` (`not_found`, lines ~315-324), `get_bucket_access_keys.rs` (lines ~189-198), `delete_bucket_access_key.rs` (lines ~241-250) and `delete_bucket.rs` (lines ~361-372) — all four build the modelled `NotFoundException` directly rather than going through the crate's not-found helpers.

## Evidence

Spec/crate rule: `services/service-com-amazonaws-lightsail-lightsail-20161128/src/scaffolded.rs` lines 44-54 — "Lightsail answers a request naming a resource it cannot find with HTTP 400 and the body's `code` set to `DoesNotExist`, even though the model declares `@httpError(404)` on `NotFoundException`; a Lightsail SDK client maps the failure from the body, not the status. [...] EVERY not-found path in this crate goes through one of those two" — and lines 199-205, `NOT_FOUND_WIRE_STATUS: u16 = 400` ("The recorded wire status governs").

Handler: `src/ops/get_buckets.rs` lines 203-211 (`store.bucket(&region, name).ok_or_else(|| not_found(...))`) and lines 267-276, where `not_found` builds `GetBucketsErrorModeled::NotFoundException`, i.e. the modelled variant whose status comes straight from the model's trait (404) and whose `code` member is left `Default::default()` (absent).

Contrast in the same crate: `src/ops/get_disk.rs` lines 176-181 and `src/ops/get_instance.rs` lines 179-187 use `Scaffolded::resource_does_not_exist`, which yields 400 + `code = "DoesNotExist"`.

Test coverage: the bucket tests assert the current 404/bare-body shape; no test asserts the crate-wide `DoesNotExist` envelope for a bucket, so the inconsistency is unguarded.

## Steps to Reproduce

1. Boot the emulator and point a Lightsail client at it (region us-east-1).
2. Call GetBuckets with bucketName set to a name no CreateBucket has created.
3. Observe HTTP 404 and a body carrying only `message` (no `code`).
4. Call GetInstance with an instanceName nothing created and observe HTTP 400 with body `code: "DoesNotExist"` — the two not-found answers of the same service disagree on both status and body shape.
5. Repeat with CreateBucketAccessKey / GetBucketAccessKeys / DeleteBucketAccessKey / DeleteBucket on an unknown bucket name for the same 404 envelope.
