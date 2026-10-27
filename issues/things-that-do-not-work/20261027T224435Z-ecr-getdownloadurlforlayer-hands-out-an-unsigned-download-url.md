Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.get-download-url-for-layer-url-expiry
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.get-download-url-for-layer-url-expiry|run#25|AssertionError|assert status2 == 403, f"expected 403 for a tampered signed URL, got {status2}"
Issue-Commit: d439c22423981a98136da88f5cba5181ca1e215e

# ECR GetDownloadUrlForLayer hands out an UNSIGNED download URL: the X-Amz-* query parameters are decorative, so a tampered 'presigned' URL is served 200 instead of refused 403 SignatureDoesNotMatch (1 cause)

> _Note: this issue was identified against the code base at commit `d439c22423981a98136da88f5cba5181ca1e215e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDownloadUrlForLayer

## Description
One net-new root cause in ECR's layer-download capability. It must be closed before this issue is submitted.

Scope vs. already-open ECR work: this is NOT covered by any open ECR issue. 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md concerns CompleteLayerUpload/CFN repositories/signing profiles/pull-through-cache validate+update, and 20261024T014332Z-ecr-asynchronous-effects-never-occur-and-several-enforcement.md concerns mirroring, replication, signing status and PTC enforcement — neither describes the download URL's signature. The S3-side presigned issues 20261027T093123Z-s3-2-net-new-root-causes-in-sigv4.md and 20261027T190000Z-s3-legacy-query-credential-presigned-get.md are a different service and a different front door (this URL is served by the ECR crate itself, not by the S3 front door). The failing check is marked neither OWNED nor ALREADY FILED in the replay report.

Cause 1 of 1: the URL GetDownloadUrlForLayer answers with is not signed at all — it carries SigV4-shaped query parameters that no fetch is judged against, so altering them changes nothing and the layer bytes are still served.

AWS: GetDownloadUrlForLayer's `downloadUrl` is documented as "The pre-signed Amazon S3 download URL for the requested layer" (Smithy: ecr#GetDownloadUrlForLayer output member `downloadUrl` -> `Url`; the operation doc states "Retrieves the pre-signed Amazon S3 download URL corresponding to an image layer"). A real S3 SigV4 presigned URL carries X-Amz-Algorithm, X-Amz-Credential, X-Amz-Date, X-Amz-Expires, X-Amz-SignedHeaders and X-Amz-Signature, and X-Amz-Expires is one of the SIGNED query parameters: changing it to any other in-range value (1..604800) while leaving X-Amz-Signature untouched makes the signature S3 recomputes differ from the one presented, and S3 refuses the fetch with HTTP 403 and the XML error document `<Code>SignatureDoesNotMatch</Code>` whose message is the fixed text "The request signature we calculated does not match the signature you provided. Check your key and signing method." (fixed text in full; the diagnostic elements AWSAccessKeyId / StringToSign / CanonicalRequest / SignatureProvided that accompany it are derived from the request as received). The requirement is therefore: the validity window of the handed-out URL is enforced CRYPTOGRAPHICALLY through parameters covered by a signature, not advisory. Smithy models no wire shape for the fetch itself (it is an object-store GET, not a modelled ECR operation), so for the fetch leg this is AWS's documented SigV4 query-string-authentication behavior plus recorded behavior.

Emulator: GetDownloadUrlForLayer mints a grant keyed by an opaque token and announces `https://<caller-endpoint>/ecr-layer-download/<token>/<layerDigest>?X-Amz-Date=<basic-timestamp>&X-Amz-Expires=<validity-seconds>` (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs, `layer_download_url`). There is no X-Amz-Algorithm, no X-Amz-Credential, no X-Amz-SignedHeaders and, crucially, no X-Amz-Signature; the serving path resolves only the stored grant addressed by the path token (`parse_layer_download_path` -> `layer_download_grants`) and ignores the query string entirely — the state.rs doc comment says so outright ("The query states the window the grant was minted with ... it is NOT what a fetch is judged against, which is the stored grant, so a rewritten query extends nothing. No signature is attached: this emulator signs nothing"). Consequently a fetch of the SAME URL with only X-Amz-Expires rewritten (1 instead of the originally announced value, every other pair byte-identical) is answered HTTP 200 with the full layer body, where AWS answers HTTP 403 SignatureDoesNotMatch. Observed: `AssertionError: expected 403 for a tampered signed URL, got 200`. Everything before that point in the same check PASSED, which localises the defect precisely: the URL carries an X-Amz-Expires in (0, 3600], and the untampered fetch returns exactly the bytes the layer was uploaded with.

Why: the emulator advertises a presigned URL and then honours it as a bearer token whose advertised bounds are unenforceable fiction. (a) Any client, proxy or test that validates or re-derives the signature — or that simply expects a tampered/forged query to be refused — gets a success where AWS refuses, so a negative test for URL tampering can never fail and a signing bug in caller code is masked. (b) The announced X-Amz-Expires is not a binding window on anything the fetch checks, so code that trusts the URL's stated validity (or deliberately widens it to prove it cannot) observes the opposite of AWS's contract. (c) Tooling that parses the presigned query for X-Amz-Algorithm / X-Amz-Credential / X-Amz-SignedHeaders / X-Amz-Signature finds them absent, so a URL that is shaped like AWS's at a glance is not consumable by a SigV4-aware consumer.

Fix: the ECR layer-download endpoint — mint the URL with the full SigV4 query parameter set (including X-Amz-Signature over the canonical request, with X-Amz-Expires among the signed parameters) in `state::layer_download_url`, and verify that signature in the layer-download request handler served under `state::LAYER_DOWNLOAD_PATH_PREFIX` (crate::service) BEFORE the grant is resolved, refusing a mismatch with HTTP 403 and an `<Error>` document whose `<Code>` is SignatureDoesNotMatch and whose `<Message>` is AWS's fixed text above. The grant lookup may remain the backing store for what the URL resolves to; what must change is that the query string becomes signed input that a fetch is judged against rather than decoration.

Traces: ecr.get-download-url-for-layer-url-expiry (fails at `assert status2 == 403, f"expected 403 for a tampered signed URL, got {status2}"`).

## Evidence
Cause 1 — ecr.get-download-url-for-layer-url-expiry, [mismatch]:
`File "check_ecr_get_download_url_for_layer_url_expiry.py", line 104, in run / assert status2 == 403, f"expected 403 for a tampered signed URL, got {status2}" / AssertionError: expected 403 for a tampered signed URL, got 200`

Expected (AWS): HTTP 403 on the tampered URL, with body `<Error><Code>SignatureDoesNotMatch</Code><Message>The request signature we calculated does not match the signature you provided. Check your key and signing method.</Message>...</Error>` — the Code and that Message are fixed text; only the accompanying diagnostic elements and request/host ids vary per request. Actual (emulator): HTTP 200 and the complete layer body.

Legs of the same check that PASSED, localising the defect to the fetch-side signature check rather than to upload, push or URL minting:
- InitiateLayerUpload / UploadLayerPart / CompleteLayerUpload for a config blob and a layer blob succeeded; PutImage stored an OCI manifest referencing both.
- GetDownloadUrlForLayer echoed `layerDigest` equal to the requested digest.
- The announced URL carried an `X-Amz-Expires` query parameter whose value satisfied `0 < ttl <= 3600`.
- Fetching the untampered URL returned HTTP 200 and a body byte-identical to the uploaded layer bytes.

Mechanism, from the checkout (file citations, not a patch): services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs `layer_download_url` formats the URL as `https://{host}{LAYER_DOWNLOAD_PATH_PREFIX}/{token}/{digest}?X-Amz-Date={..}&X-Amz-Expires={..}` with no X-Amz-Signature member, and its own doc comment states the query "is NOT what a fetch is judged against, which is the stored grant, so a rewritten query extends nothing. No signature is attached". `parse_layer_download_path` extracts only the token and digest from the path, so the query is never read on the serving side. The generated-host, token, timestamp and TTL values in any one run are per-run values and are not part of the requirement.

## Steps to Reproduce
Boot the emulator and replay the check id in trace_ids (ecr.get-download-url-for-layer-url-expiry).

Manual repro:
1. `CreateRepository` with a fresh name.
2. Upload a small blob: `InitiateLayerUpload`, `UploadLayerPart(partFirstByte=0, partLastByte=len-1, layerPartBlob=<bytes>)`, `CompleteLayerUpload(layerDigests=["sha256:<hex>"])`. Do the same for a tiny config blob (`b"{}"`).
3. `PutImage` with an OCI image manifest (schemaVersion 2, mediaType application/vnd.oci.image.manifest.v1+json) whose `config` names the config digest and whose single `layers` entry names the layer digest.
4. `GetDownloadUrlForLayer(repositoryName=<repo>, layerDigest=<layer digest>)` and take `downloadUrl`.
5. GET the URL unchanged with a raw HTTP client — it returns 200 and the uploaded bytes (correct).
6. Re-encode the SAME query with only the expiry pair changed to another in-range value (X-Amz-Expires=1, or for the `Expires` form a far-future epoch), leaving every other query pair byte-identical, and GET it.
   Expected: HTTP 403 with `<Code>SignatureDoesNotMatch</Code>` and the message "The request signature we calculated does not match the signature you provided. Check your key and signing method.".
   Observed: HTTP 200 with the full layer body.
7. Also inspect the query string from step 4: it contains only X-Amz-Date and X-Amz-Expires — no X-Amz-Algorithm, X-Amz-Credential, X-Amz-SignedHeaders or X-Amz-Signature — confirming nothing signed the URL.
