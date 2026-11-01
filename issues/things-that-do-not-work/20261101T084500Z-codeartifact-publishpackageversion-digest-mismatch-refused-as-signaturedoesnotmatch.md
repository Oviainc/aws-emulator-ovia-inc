Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent|run#15|AssertionError|assert code == "ValidationException", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeArtifact: a PublishPackageVersion whose assetSHA256 disagrees with the body is refused 403 SignatureDoesNotMatch by SigV4 payload verification instead of 400 ValidationException

> _Note: this cause was first filed as cause 2 of
> `20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest`,
> whose other two causes are now implemented. It is carried here on its own so it stays
> open work, with the replay key it owns. The code base can have drifted from the commit
> above, so trace it back to the current tree._

## Operation
PublishPackageVersion

## Description
One cause: the digest-integrity contract of `PublishPackageVersion` is unreachable, because
the request is refused as an AUTHENTICATION failure before the operation is dispatched.

AWS: `assetSHA256` is bound to the HTTP header `x-amz-content-sha256`
(`smithy_query(query="operation", operation="PublishPackageVersion")`: `assetSHA256` ->
`SHA256`, `httpHeader x-amz-content-sha256`, required, `@length 64..64`, `@pattern
^[0-9a-f]+$`) and `assetContent` is the streaming `@httpPayload`. A declared digest that
does not match the uploaded bytes is a MODELED APPLICATION error, not an auth failure: the
request authenticates, the service hashes the body, compares, and refuses with
`ValidationException` (HTTP 400, which that operation models) whose message names both
digests — "The provided content hashes do not match the calculated content hashes. Hash
algorithm 'SHA-256', Expected hash '<the assetSHA256 the request declared>', Actual hash
'<the SHA-256 the service computed over the uploaded bytes>'": the two hex digests are
values, one from the request header and one from the uploaded bytes, the rest is fixed text,
and the rejected publish stores nothing.

Emulator: HTTP 403 with `x-amzn-errortype` `SignatureDoesNotMatch` and the message 'The
request signature does not match' — an error this operation does not model at all.

Why: SDKs and tools treat `SignatureDoesNotMatch` as a credential or clock problem and
re-sign, refresh credentials or alarm on an authorization failure, so a corrupted upload is
diagnosed as a signing failure and the 400 branch a client needs in order to recompute its
digest and retry the UPLOAD is never taken.

Where it is refused, and why implementing the handler is not enough: the handler half is
already written. `src/ops/publish_package_version.rs` hashes the received bytes, compares
them with the declared digest and raises the modelled `ValidationException` with the sentence
above, storing nothing — it is simply never reached. The request is refused by the SigV4
payload-hash check in the request pipeline, which lives in the read-only core:

* `vendor/server/src/request_authentication.rs`, `fn payload` — "a declared marker
  (`UNSIGNED-PAYLOAD`, `STREAMING-...`) is signed as written; a declared hash is not trusted,
  the body is hashed instead", so a declared `x-amz-content-sha256` is honoured only when it
  is a marker containing `-`.
* `vendor/smithy-server-runtime/src/multi_service_router.rs` — `dispatch` runs that
  signature check for the MODELLED operation before the operation's `run`, so no service seam
  reaches the decision: `can_handle` sees only the request parts (never the body), and
  `override_refusal` can only reword a refusal already decided.

A client signs the canonical request with the `x-amz-content-sha256` value it SENT (botocore's
`SigV4Auth._canonical_request` uses that header when present), so whenever the header is a
member-bound digest that disagrees with the body, the emulator's recomputed canonical request
differs from the client's and authentication fails.

Fix (the core change this needs): `fn payload` in
`vendor/server/src/request_authentication.rs` must take a declared `x-amz-content-sha256`
that is a 64-character lowercase hex digest as the SIGNED payload hash
(`SignableBody::Precomputed`) rather than re-deriving it from the body — it is a value the
client signed with, which is why AWS can authenticate the request and still refuse the
digest at the application layer. With that one change the request authenticates and the
comparison already written in the handler answers the modelled 400. The change is outside
`services/`, so it cannot be made from inside the emulator tree.

Traces: codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent.

## Evidence
codeartifact.codeartifact-publish-with-mismatched-asset-sha256-rejected-and-version-absent:
  expected ValidationException (HTTP 400) for a digest mismatch
  got      'SignatureDoesNotMatch': 'The request signature does not match'
  Request: publish_package_version(format='generic', assetContent=<8192 bytes>,
  assetName='payload.bin', assetSHA256=<the true sha256 with one hex character changed>).
  The same call carrying the TRUE digest is served by the handler, so this is not an
  unimplemented-operation cause: only the mismatched-digest request is refused, and it is
  refused in the auth layer.

## Steps to Reproduce
1. create_domain(domain=D); create_repository(domain=D, repository=R).
2. blob = 8192 arbitrary bytes; wrong = sha256(blob).hexdigest() with one hex character
   changed.
3. publish_package_version(domain=D, repository=R, format='generic', namespace='sim2real',
   package=P, packageVersion='1.0.0', assetContent=blob, assetName='payload.bin',
   assetSHA256=wrong) -> observe 403 SignatureDoesNotMatch instead of a 400
   ValidationException naming the declared and the calculated hash, and confirm with
   describe_package_version that the version is absent either way.
4. Repeat step 3 with the TRUE digest -> HTTP 200, which shows the refusal is the digest
   check in the auth layer and not the handler.
