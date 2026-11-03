Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.cli-s3-cp-large-file-multipart-checksum-defaults s3.s3-csvcols-cli-s3-cp-to-stdout-matches-sdk-bytes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.cli-s3-cp-large-file-multipart-checksum-defaults|_run#22|AssertionError|assert r.returncode == 0, f"{' '.join(argv)} failed (rc={r.returncode}): {r.stderr[-600:]!r}"
Issue-Key: s3.s3-csvcols-cli-s3-cp-to-stdout-matches-sdk-bytes|run#7|AssertionError|assert done.returncode == 0, f"aws s3 cp exited {done.returncode}: {done.stderr[-800:]!r}"
Issue-Commit: 47485e50fea67a007c4a8093a42341fcc32a5be5

# s3: the HeadObject the AWS CLI's `s3 cp` download path issues is refused HTTP 400 Bad Request, so no CLI download of an existing object can succeed (1 cause)

> _Note: this issue was identified against the code base at commit `47485e50fea67a007c4a8093a42341fcc32a5be5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
HeadObject

## Description
This issue carries 1 net-new root cause in S3's HeadObject request-admission path, grounded in the two failing checks listed in trace_ids. Both fail at the same point and with the same response, so they are one cause; it must be closed before submitting.

===== Cause 1 of 1: a HeadObject issued by the AWS CLI's high-level `s3 cp` transfer path is answered HTTP 400 ("Bad Request") for an object that exists =====

AWS: HeadObject is modeled `HEAD /{Bucket}/{Key+}` with success status 200 (Smithy: HeadObject http {method: HEAD, uri: "/{Bucket}/{Key+}", code: 200}; input members are all @httpLabel/@httpHeader/@httpQuery bindings — Bucket, Key, IfMatch/IfNoneMatch/IfModifiedSince/IfUnmodifiedSince, Range, VersionId, PartNumber, RequestPayer, ChecksumMode, ExpectedBucketOwner, SSECustomerAlgorithm/Key/KeyMD5). For a key the bucket holds, AWS answers 200 with the metadata headers (ETag, Content-Length, Last-Modified, x-amz-meta-*) and NO body. AWS's own documentation for this operation states the only statuses a HEAD can carry are the generic ones it lists — 400 Bad Request, 403 Forbidden, 404 Not Found, 405, 412, 304 — and a 400 there means the REQUEST was malformed, never "the object is there": a present object is 200, an absent one 404 (403 without s3:ListBucket). The operation's only modeled error, `NotFound`, carries no @httpError (httpErrorDefaulted: true -> 400), which is exactly why a 400 on a HEAD is indistinguishable from a malformed request to a client: a HEAD response has no error document, so the status is all the client gets. Recorded behavior for the CLI leg: `aws s3 cp` first HEADs the key to size the download and then GETs it; against real AWS both legs succeed and the bytes round-trip.

Emulator: the HEAD that the AWS CLI's transfer manager issues on a download is answered HTTP 400 with no S3 error code, so the CLI reports `An error occurred (400) when calling the HeadObject operation: Bad Request` and exits 1 with `download failed: s3://<bucket>/<key> to <dest>`. The same key HEADs fine through the SDK in the same check (`head_object(Bucket=..., Key=..., ChecksumMode="ENABLED")` returns 200 with ChecksumType/ChecksumSHA256, and GetObject returns the exact bytes), and the CLI's UPLOAD legs in the same check succeed (including a two-part multipart `cp` with --checksum-algorithm SHA256) — so credentials, addressing and signing are all working for the CLI; only the HEAD it issues is refused. The refusal is also not produced inside HeadObject's handler for an existing object (that handler answers 200, or NoSuchBucket/NoSuchKey/NoSuchVersion as 404, or InvalidArgument for a malformed versionId), which places it in the request walk ahead of the handler. Hypothesis: a header or query parameter the CLI's transfer manager puts on its HEAD (it is a different client code path from the SDK call the check makes) fails header deserialization or input-constraint validation in the framework's deserialize/validate step, which renders the client-error default 400; the emulator's response carries no code or message that would name which member was rejected.

Why: a client gets a bare `400 Bad Request` where AWS answers 200, so the single most common S3 client operation — `aws s3 cp s3://bucket/key <dest>` and every CLI/`s3transfer`-based download, sync and streaming read (`cp ... -`) — fails outright against the emulator, while the same object is perfectly readable through an SDK call. Because a HEAD carries no error document, the caller cannot tell a rejected request from a missing object, a denied object or a broken endpoint, and the natural reading ("400 = my request is malformed") sends users to debug a request that is in fact valid. Any test suite or tool that shells out to the CLI to fetch objects is unusable, and a CLI download regression in a product under test would be masked by this.

Fix: s3's HeadObject request-admission path — the framework request walk (header/query deserialization and input-constraint validation for HeadObjectRequest) ahead of `HeadObject::handle` in the s3 crate — must accept the HEAD form the CLI's transfer manager sends and let it reach the handler, so an existing object answers 200 with its metadata headers; and no admission refusal on this route may surface as an unlabeled 400 for a request AWS accepts. Whatever member is being rejected must be deserialized/validated as the model binds it (the bindings listed above), and a genuinely absent object must keep answering 404 rather than the modeled `NotFound` default of 400.

Related open issue, which does NOT cover this: 20261028T105230Z-s3-8-net-new-root-causes-in-the.md (Cause 2) records virtual-hosted-style addressing failing SigV4 with 403 `SignatureDoesNotMatch`; that is a different status, a different error and a different request path (the CLI here is path-style and its writes succeed), and neither of the two checks below is carried by that issue or by any other open issue.

Traces: s3.cli-s3-cp-large-file-multipart-checksum-defaults (CLI download of a 12 MiB object the CLI itself uploaded multipart); s3.s3-csvcols-cli-s3-cp-to-stdout-matches-sdk-bytes (CLI download of a small SDK-written object to stdout).

## Evidence
Cause 1 — expected (AWS): `aws s3 cp s3://<bucket>/<key> <dest>` exits 0; its HeadObject leg answers HTTP 200 with the object's metadata headers (ETag, Content-Length, Last-Modified) and no body, and the downloaded file equals the stored bytes.

Actual (emulator), check s3.cli-s3-cp-large-file-multipart-checksum-defaults: the CLI upload legs succeed and the SDK HeadObject(ChecksumMode=ENABLED) + GetObjectAttributes assertions on the same key pass, then
  AssertionError: aws s3 cp s3://sim2real-behav-clichk-<id>/large-sha256.bin <dest> failed (rc=1): 'download failed: s3://sim2real-behav-clichk-<id>/large-sha256.bin to <dest> An error occurred (400) when calling the HeadObject operation: Bad Request\n'

Actual (emulator), check s3.s3-csvcols-cli-s3-cp-to-stdout-matches-sdk-bytes: the SDK GetObject positive control returns the exact seeded bytes, then
  AssertionError: aws s3 cp exited 1: b'download failed: s3://sim2real-scn-csvcols-<id>/products.csv to - An error occurred (400) when calling the HeadObject operation: Bad Request\n'

Message anatomy: `An error occurred (400) ... Bad Request` is botocore's rendering of a response that carried NO S3 error code at all (a HEAD has no body, so the status is the whole answer) — the divergence is the status itself: 400 where AWS sends 200 for a present key (and 404/403 for an absent/denied one). The bucket names, object keys, local destination paths and the generated id suffixes above are per-run values from the request and the harness, not part of the requirement; the requirement is the status/headers AWS returns for a HEAD of an existing object.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check ids in trace_ids.

Manually, against the emulator endpoint with the AWS CLI installed:
1. Create a bucket (any region; add CreateBucketConfiguration.LocationConstraint outside us-east-1) and PUT an object into it with the SDK, e.g. key `products.csv`.
2. Confirm the object is readable through the SDK: `head_object(Bucket=..., Key="products.csv")` -> 200, `get_object(...)` -> the exact bytes.
3. With the same credentials/endpoint in the environment, run `aws s3 cp s3://<bucket>/products.csv -` (or to a local file). Observe exit code 1 and `download failed: ... An error occurred (400) when calling the HeadObject operation: Bad Request` instead of the object's bytes.
4. Variant that also shows the write path is healthy: `aws s3 cp <12MiB local file> s3://<bucket>/large-sha256.bin --checksum-algorithm SHA256` succeeds (multipart, two parts), then `aws s3 cp s3://<bucket>/large-sha256.bin <dest>` fails with the same HeadObject 400.
5. Capture the HEAD request the CLI sends (`aws --debug s3 cp ...`) and compare its headers/query with the SDK's HeadObject to see which member the request walk is rejecting.
