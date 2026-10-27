Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.multipart-upload-and-presigned-get-round-trip
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.multipart-upload-and-presigned-get-round-trip|_fetch#25|AssertionError|raise AssertionError(
Issue-Commit: e023b33658189531eb8614d1ec951568d660b5c0

# s3: a presigned GET whose credential travels in the LEGACY query form is refused before any handler runs

> _Note: this is the one cause of
> `20261027T052624Z-s3-8-net-new-root-causes-partnumber-reads.md` that could not be closed
> with it. The other seven are closed; this one is blocked outside `services/`, in the
> agent-immutable `vendor/` tree, so it is recorded here rather than left inside a report
> whose other causes now work._

## Operation
GetObject

## Description
AWS serves a presigned GET of an existing key over the data-plane endpoint with HTTP 200,
exactly as a signed `GetObject` would; the signature is the only difference. Two credential
forms can carry that signature in a URL's query string:

* the SigV4 form — `X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`,
  `X-Amz-SignedHeaders`, `X-Amz-Signature`; and
* the LEGACY form — `AWSAccessKeyId`, `Signature`, `Expires` (AWS General Reference,
  *Authenticating requests: using query parameters*), whose signature is an HMAC-SHA1 over a
  string-to-sign ending in the `Expires` instant. `us-east-1` still accepts it, which is why
  the recorded run was served.

The emulator serves the FIRST form (the object data plane resolves the same object a signed
read resolves, in both the path-style and the virtual-hosted-style address — pinned by
`services/service-com-amazonaws-s3-amazons3/tests/all/presigned_object_reads.rs`). It cannot
serve the SECOND: the legacy credential is refused by the framework's authentication layer
before any handler of this service runs.

* `vendor/server/src/request_authentication.rs` — `SigV4Authenticator::recognizes` claims a
  request only when it carries an `Authorization` header beginning `AWS4-HMAC-SHA256` or a
  query `X-Amz-Algorithm` equal to it, and `Signature::of` reads only the `X-Amz-*` fields.
  A request carrying the legacy three therefore matches NO registered authenticator, and
  `smithy_server_runtime::MultiServiceRouter::authenticate` (via `authenticator_for`) refuses
  it with the framework's `ACCESS_DENIED` — HTTP 403 — before dispatch reaches the operation.
* `vendor/server/src/lib.rs` — `resolve_route_key` / `service_from_sigv4_query` read only the
  SigV4 scope, so such a request also resolves no routing key; before the s3 service began
  claiming it (`services/service-com-amazonaws-s3-amazons3/src/service.rs`,
  `carries_legacy_presigned_credential`) the router answered `404 UnknownService` for it
  instead, which is the 404 the recorded check reported.

Neither file is editable from inside this emulator, and a service cannot register an
authenticator or declare an operation unsigned (the model carries no `@optionalAuth`, and
`AGENTS.md` forbids overriding `auth_schemes`). So the emulator's honest answer today is the
authentication refusal, and the recorded 200 is unreachable from `services/`.

## Required core change
An authenticator that RECOGNIZES and VERIFIES the legacy query-credential form —
`AWSAccessKeyId` + `Signature` + `Expires`, HMAC-SHA1 over the canonical string-to-sign,
refusing an expired `Expires` — registered beside `SigV4Authenticator` in
`vendor/server/src/request_authentication.rs`, plus `resolve_route_key` in
`vendor/server/src/lib.rs` reading that form (its scope names no service, so the host or an
explicit S3 claim has to supply the routing key, which
`service-com-amazonaws-s3-amazons3`'s `can_handle` already does).

## Evidence
`AssertionError: DATAPLANE_UNIMPLEMENTED: the presigned GET URL did not serve the object:
HTTPError: HTTP Error 403: Forbidden` — the status the framework's authentication layer
answers once the request routes (it was `HTTP Error 404: Not Found` while nothing claimed
it). AWS answers 200 with the object's bytes.

## Steps to Reproduce
1. Complete a two-part multipart upload at a key.
2. Generate a presigned GET for that key with a client whose S3 `signature_version` is the
   legacy `s3` rather than `s3v4`, so the URL carries `AWSAccessKeyId`, `Signature` and
   `Expires`.
3. Fetch the URL over HTTP. AWS serves the object; this emulator answers 403.
