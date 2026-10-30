Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.import-asset-signed-url-job-roundtrip dataexchange.revision-asset-finalize-state-transition dataexchange.update-asset-name-persists-and-full-lifecycle-e2e dataexchange.delete-asset-after-finalize-rejected dataexchange.finalize-revision-without-assets-rejected dataexchange.update-revoked-revision-rejected dataexchange.revoke-revision-not-in-grant-rejected dataexchange.accept-data-grant-wrong-caller-rejected dataexchange.create-data-grant-ends-at-past-rejected dataexchange.create-data-grant-invalid-receiver-principal-rejected dataexchange.data-grant-create-reflects-pending-state dataexchange.data-grant-delete-removes-grant
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange.import-asset-signed-url-job-roundtrip|run#13|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.revision-asset-finalize-state-transition|_import_asset_via_signed_url#6|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.update-asset-name-persists-and-full-lifecycle-e2e|_import_asset_via_signed_url#6|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.delete-asset-after-finalize-rejected|_import_asset_via_signed_url#4|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.finalize-revision-without-assets-rejected|_import_asset_via_signed_url#10|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.update-revoked-revision-rejected|_import_asset_via_signed_url#4|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.revoke-revision-not-in-grant-rejected|_import_asset_via_signed_url#4|HTTPError|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: dataexchange.accept-data-grant-wrong-caller-rejected|_create_finalized_dataset#4|HTTPError|with urllib.request.urlopen(req, timeout=30, context=_ssl_context(ctx.http_verify)) as resp:
Issue-Key: dataexchange.create-data-grant-ends-at-past-rejected|_create_finalized_dataset#2|HTTPError|with urllib.request.urlopen(req, timeout=30, context=_ssl_context(ctx.http_verify)) as resp:
Issue-Key: dataexchange.create-data-grant-invalid-receiver-principal-rejected|_create_finalized_dataset#2|HTTPError|with urllib.request.urlopen(req, timeout=30, context=_ssl_context(ctx.http_verify)) as resp:
Issue-Key: dataexchange.data-grant-create-reflects-pending-state|_create_finalized_dataset#4|HTTPError|with urllib.request.urlopen(req, timeout=30, context=_ssl_context(ctx.http_verify)) as resp:
Issue-Key: dataexchange.data-grant-delete-removes-grant|_create_finalized_dataset#2|HTTPError|with urllib.request.urlopen(req, timeout=30, context=_ssl_context(ctx.http_verify)) as resp:
Issue-Commit: b2028dd6393016a9896c0e35ff905bdd884f0327

# DataExchange CreateJob mints an IMPORT_ASSET_FROM_SIGNED_URL upload URL that nothing serves: the PUT returns HTTP 404, blocking every asset-bearing workflow

> _Note: this issue was identified against the code base at commit `b2028dd6393016a9896c0e35ff905bdd884f0327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Cause 1 of 1: the upload target CreateJob hands out for an IMPORT_ASSET_FROM_SIGNED_URL job is announced but not served, so the asset-upload leg of the import protocol cannot be completed at all.

AWS: for Type IMPORT_ASSET_FROM_SIGNED_URL, CreateJob (POST /v1/jobs, success 201) answers Details.ImportAssetFromSignedUrl with a usable presigned upload URL and its expiry (Smithy: ImportAssetFromSignedUrlResponseDetails { AssetName required, DataSetId required, Md5Hash, RevisionId required, SignedUrl: __string "The signed URL.", SignedUrlExpiresAt: Timestamp timestampFormat date-time; request detail ImportAssetFromSignedUrlRequestDetails.Md5Hash required, @length 24/24, base64 pattern). The documented client protocol is three legged: CreateJob, then an HTTP PUT of the asset bytes to that SignedUrl carrying Content-MD5 equal to the request's Md5Hash — which the upload endpoint answers 2xx (recorded behavior: the replayed clients accept 200 or 201) — and only then StartJob, which imports the uploaded bytes into the named revision as an asset. The URL is a real, immediately writable location for the lifetime SignedUrlExpiresAt advertises; the host, job id and expiry instant are per-environment values, the writability is the requirement.

Emulator: CreateJob returns 201 with a SignedUrl of the shape https://<the host the request arrived at>/v1/jobs/<job id>/asset and a SignedUrlExpiresAt one hour out, but no route serves that path. A PUT of the asset bytes to the emulator's own announced URL fails with HTTP 404 Not Found (replay detail: `urllib.error.HTTPError: HTTP Error 404: Not Found`, raised from the raw PUT, not from any SDK call). The handler states this is deliberate: services/service-com-amazonaws-dataexchange-dataexchange/src/ops/create_job.rs, fn upload_url — "no operation in this service serves it, and nothing downstream reads it back as content".

Why: a client that follows the documented import protocol cannot get past step two. Every workflow that needs a revision with an asset in it is unreachable, which is why this single cause accounts for 12 of this suite's failing checks: the asset lifecycle (import roundtrip, asset rename, delete-after-finalize), revision finalization and its refusals (finalize-without-assets, update-revoked-revision, revoke-revision-not-in-grant), and all data-grant checks, which need a finalized source data set before CreateDataGrant can be exercised at all. The failure mode is also the worst kind for a caller: the service itself handed out the address that then 404s, so the client cannot tell a broken emulator from a mis-signed URL, and it never learns the upload was never stored.

Fix: the dataexchange crate needs an upload data plane for the locator CreateJob mints — a route on the same endpoint the API is served on that accepts the PUT at the minted path, stores the body as that job's pending asset content, verifies Content-MD5 against the job's recorded Md5Hash, and refuses after SignedUrlExpiresAt — together with the job-execution path that moves those stored bytes into the revision as an asset when the job runs. The URL-minting contract lives in services/service-com-amazonaws-dataexchange-dataexchange/src/ops/create_job.rs (fn upload_url, const UPLOAD_URL_VALIDITY_SECONDS) and must agree with whatever serves it.

Note for the fixer: StartJob is separately an unimplemented scaffold stub (an issue naming it and services/service-com-amazonaws-dataexchange-dataexchange/src/ops/start_job.rs is already filed). These 12 checks never reach StartJob — they die on the upload PUT before it — so this 404 is a distinct cause and implementing StartJob alone will not clear any of them. Neither does the open issue "DataExchange jobs never leave WAITING: cancel and import never reach a terminal state" (20261029T172356Z-dataexchange-jobs-never-leave-waiting-cancel-and-import.md), which is about job state advance after a start and explicitly sets the signed URL aside.

Traces: dataexchange.import-asset-signed-url-job-roundtrip is the minimal example (CreateJob then PUT); the other eleven listed checks fail at the same PUT inside their finalized-data-set setup.

## Evidence
Expected: PUT <Details.ImportAssetFromSignedUrl.SignedUrl> with header Content-MD5: <the base64 MD5 the CreateJob request carried> and the asset bytes as the body -> HTTP 200 (or 201), the bytes retained as the job's asset content.
Actual: HTTP 404 Not Found. Replay detail, identical across all 12 checks:
  File "request.py", line 639, in http_error_default
    raise HTTPError(req.full_url, code, msg, hdrs, fp)
  urllib.error.HTTPError: HTTP Error 404: Not Found
The failing request is the raw PUT to the URL the emulator itself returned, so no SDK error code or message is involved: the status 404 with no modeled body is the whole of the response.
Source confirmation (services/service-com-amazonaws-dataexchange-dataexchange/src/ops/create_job.rs): `fn upload_url(endpoint, job_id, leaf) -> Option<String> { endpoint.map(|host| format!("https://{host}/v1/jobs/{job_id}/{leaf}")) }`, documented as "no operation in this service serves it, and nothing downstream reads it back as content"; the ImportAssetFromSignedUrl arm sets `signed_url = upload_url(endpoint, job_id, "asset")` and SignedUrlExpiresAt = CreatedAt + 3600s.
The host, the 30-40 character job id and the expiry instant are per-environment/per-run values and are not part of the requirement; that a PUT to the announced URL succeeds is.
Smithy: ImportAssetFromSignedUrlResponseDetails.SignedUrl (__string), .SignedUrlExpiresAt (Timestamp, date-time); CreateJob POST /v1/jobs code 201; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1), check dataexchange.import-asset-signed-url-job-roundtrip.
2. CreateDataSet (AssetType S3_SNAPSHOT), then CreateRevision on it.
3. CreateJob with Type=IMPORT_ASSET_FROM_SIGNED_URL and Details.ImportAssetFromSignedUrl = { AssetName, DataSetId, RevisionId, Md5Hash = base64(md5(bytes)) }. Note the returned Details.ImportAssetFromSignedUrl.SignedUrl.
4. PUT the same bytes to that SignedUrl with header Content-MD5 set to the same Md5Hash. The emulator answers HTTP 404 Not Found; AWS's presigned target answers 2xx and retains the bytes.
5. Because step 4 fails, the following StartJob/GetJob import never happens and no revision can be finalized with an asset — repeat with dataexchange.revision-asset-finalize-state-transition or any dataexchange.data-grant-* check to see the same 404 raised from their finalized-data-set setup helper.
