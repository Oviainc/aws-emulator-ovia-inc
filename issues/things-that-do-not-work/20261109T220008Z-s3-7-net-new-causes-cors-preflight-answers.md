Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.cors-preflight-allowed-origin-returns-rule-headers s3.cors-preflight-disallowed-method-rejected-allowed-method-passes s3.cors-preflight-disallowed-origin-rejected-allowed-origin-passes s3.cors-preflight-on-bucket-without-cors-configuration-rejected s3.cors-preflight-refused-after-delete-bucket-cors s3.cors-presigned-put-and-get-with-origin-carry-allow-origin s3.cors-presigned-with-origin-without-cors-config-no-cors-headers s3.e2e-website-deployment s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes s3.s3-website-hosting-get-rejected-for-a-bucket-never-configured s3.s3-website-hosting-bare-folder-path-redirects-to-trailing-slash s3.s3-website-hosting-missing-key-with-no-error-document-is-rejected-with-the-default-error-page s3.s3-website-hosting-redirect-all-requests-to-answers-301-to-the-target-host s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys s3.put-object-policy-requires-if-none-match-header s3.delete-object-policy-requires-if-match-header s3.copy-object-under-if-none-match-policy s3.s3-chunked-upload-missing-decoded-content-length-rejected-object-absent s3.s3-chunked-upload-trailer-checksum-mismatch-rejected-object-absent s3.s3-chunked-upload-trailer-name-mismatch-rejected-object-absent s3.s3-chunked-upload-trailer-checksum-stores-object-and-checksum s3.copy-object-if-match-on-the-destination-refuses-a-stale-etag s3.multipart-upload-with-sparse-part-numbers-uploaded-in-reverse-round-trips s3.put-object-sse-kms-encryption-context-round-trips-then-malformed-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.cors-preflight-allowed-origin-returns-rule-headers|_until_status#6|AssertionError|raise AssertionError(
Issue-Key: s3.cors-preflight-disallowed-method-rejected-allowed-method-passes|_until_status#6|AssertionError|raise AssertionError(
Issue-Key: s3.cors-preflight-disallowed-origin-rejected-allowed-origin-passes|_until_status#6|AssertionError|raise AssertionError(
Issue-Key: s3.cors-preflight-on-bucket-without-cors-configuration-rejected|_until_status#6|AssertionError|raise AssertionError(
Issue-Key: s3.cors-preflight-refused-after-delete-bucket-cors|_until_status#6|AssertionError|raise AssertionError(
Issue-Key: s3.cors-presigned-put-and-get-with-origin-carry-allow-origin|_until_status#8|AssertionError|raise AssertionError(
Issue-Key: s3.cors-presigned-with-origin-without-cors-config-no-cors-headers|_until_status#6|AssertionError|raise AssertionError(
Issue-Key: s3.e2e-website-deployment|run#11|CapabilityNotOffered|endpoint = ctx.s3_website_urls([aws], bucket)[0]
Issue-Key: s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes|_website_base_url#14|CapabilityNotOffered|candidates = ctx.s3_website_urls(
Issue-Key: s3.s3-website-hosting-get-rejected-for-a-bucket-never-configured|_website_base_url#10|CapabilityNotOffered|candidates = ctx.s3_website_urls(
Issue-Key: s3.s3-website-hosting-bare-folder-path-redirects-to-trailing-slash|_http#7|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: s3.s3-website-hosting-missing-key-with-no-error-document-is-rejected-with-the-default-error-page|_http#9|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: s3.s3-website-hosting-redirect-all-requests-to-answers-301-to-the-target-host|_http#9|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys|_website_base_url#9|CapabilityNotOffered|candidates = ctx.s3_website_urls(
Issue-Key: s3.put-object-policy-requires-if-none-match-header|_await_policy#8|TimeoutError|raise TimeoutError(
Issue-Key: s3.delete-object-policy-requires-if-match-header|_await_policy#9|TimeoutError|raise TimeoutError(
Issue-Key: s3.copy-object-under-if-none-match-policy|_await_policy#11|TimeoutError|raise TimeoutError(
Issue-Key: s3.s3-chunked-upload-missing-decoded-content-length-rejected-object-absent|run#6|AssertionError|assert status == 411, (
Issue-Key: s3.s3-chunked-upload-trailer-checksum-mismatch-rejected-object-absent|run#9|AssertionError|assert status == 400, (
Issue-Key: s3.s3-chunked-upload-trailer-name-mismatch-rejected-object-absent|run#7|AssertionError|assert status == 400, (
Issue-Key: s3.s3-chunked-upload-trailer-checksum-stores-object-and-checksum|run#9|AssertionError|assert head.get("ChecksumCRC32") == crc32, (
Issue-Key: s3.copy-object-if-match-on-the-destination-refuses-a-stale-etag|run#12|AssertionError|assert late == PRECONDITION, f"CopyObject If-Match on a stale destination ETag: expected {PRECONDITION!r}, got {late!r}"
Issue-Key: s3.multipart-upload-with-sparse-part-numbers-uploaded-in-reverse-round-trips|_fetch#30|AssertionError|raise AssertionError(
Issue-Key: s3.put-object-sse-kms-encryption-context-round-trips-then-malformed-rejected|run#15|AssertionError|assert decoded == expected, f"PutObject echoed context {decoded!r}, expected {expected!r}"
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# s3: 7 net-new causes — CORS preflight answers 501, the website endpoint is unserved/announced on an AWS host, s3:if-match/if-none-match policy conditions are unenforced, aws-chunked trailers are ignored, CopyObject If-Match is a 501 stub, presigned GET of a multipart object is 403, and the SSE-KMS context echo drops aws:s3:arn

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Seven causes.

Cause 1 of 7: the CORS preflight (OPTIONS on a bucket/object path) is not implemented.
AWS: S3 answers a CORS preflight OPTIONS for a bucket with a CORS configuration with HTTP 200 and the rule's Access-Control-Allow-Origin/-Methods/-Headers/-Max-Age headers; a disallowed origin or method is refused 403 with 'CORSResponse: This CORS request is not allowed.'; a bucket with no CORS configuration (or after DeleteBucketCors) is refused 403 'CORSResponse: CORS is not enabled for this bucket.' (recorded + documented REST behavior — the preflight is not a modeled Smithy operation, the CORS rule set is s3 PutBucketCors/GetBucketCors CORSRules).
Emulator: every preflight answers HTTP 501 <Error><Code>NotImplemented</Code><Message>operation OPTIONS /<bucket>/<key> is not implemented in this emulator</Message>. Fixed text 'operation OPTIONS ... is not implemented in this emulator'; the path is taken from the request.
Why: no browser can upload to or read from a bucket through the emulator — the preflight is the gate for every cross-origin PUT/GET — and both the allow and the deny verdicts of a CORS configuration are unobservable.
Fix: the S3 request router needs an OPTIONS handler that evaluates the bucket's CORS rules and emits the Access-Control-* response headers (and the 403 CORSResponse refusals).
Traces: s3.cors-preflight-allowed-origin-returns-rule-headers, s3.cors-preflight-disallowed-method-rejected-allowed-method-passes, s3.cors-preflight-disallowed-origin-rejected-allowed-origin-passes, s3.cors-preflight-on-bucket-without-cors-configuration-rejected, s3.cors-preflight-refused-after-delete-bucket-cors, s3.cors-presigned-put-and-get-with-origin-carry-allow-origin, s3.cors-presigned-with-origin-without-cors-config-no-cors-headers.

Cause 2 of 7: there is no reachable S3 website endpoint — the emulator either offers none or announces one on an AWS-owned host.
AWS: a bucket with a website configuration is served on a website endpoint (documented form <bucket>.s3-website-<region>.amazonaws.com) that serves IndexDocument for '/' and for a bare folder path (301 to the trailing slash), serves ErrorDocument or the default error page with 404 for a missing key, honours RoutingRules and RedirectAllRequestsTo with 301 to the target host, and refuses writes. Smithy: s3 PutBucketWebsite/GetBucketWebsite carry IndexDocument, ErrorDocument, RoutingRules and RedirectAllRequestsTo.
Emulator: half the checks get 'CapabilityNotOffered: the emulator offers no S3 website endpoint' and half get 'PublishedUrlRejected: endpoint_url was given http://<bucket>.s3-website-us-west-2.amazonaws.com/ ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path>' — i.e. where a website address is produced at all it is AWS's public hostname, which the emulator does not answer on. The AWS-shaped host is the fixed part; bucket name, region and port vary per run.
Why: static website hosting is entirely unusable, and every website-specific rule (index/error documents, folder redirects, routing rules, write refusal) is unverifiable.
Fix: serve the S3 website endpoint on the emulator's own host (own host/port or a per-bucket listener), publish that address, and implement the website request semantics in that handler.
Traces: s3.e2e-website-deployment, s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes, s3.s3-website-hosting-get-rejected-for-a-bucket-never-configured, s3.s3-website-hosting-bare-folder-path-redirects-to-trailing-slash, s3.s3-website-hosting-missing-key-with-no-error-document-is-rejected-with-the-default-error-page, s3.s3-website-hosting-redirect-all-requests-to-answers-301-to-the-target-host, s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys.

Cause 3 of 7: bucket-policy conditions on s3:if-match / s3:if-none-match are not enforced.
AWS: a bucket policy that denies an action unless the request carries If-Match (or If-None-Match) is evaluated against the request's conditional headers; a PutObject/DeleteObject/CopyObject without the required header is refused 403 AccessDenied 'Access Denied', and with it is allowed. Recorded behavior: each check polls for the AccessDenied verdict on the header-less call.
Emulator: the header-less calls all SUCCEED — PutObject 200, DeleteObject 204, CopyObject 200 — so the policy condition key is ignored entirely.
Why: conditional-write enforcement is how callers make overwrite-protection mandatory; ignoring the condition lets exactly the unguarded writes the policy forbids through, and the policy reads as effective while it is not.
Fix: the S3 authorizer's policy-condition evaluation must populate and match the s3:if-match / s3:if-none-match request condition keys from the request's If-Match/If-None-Match headers.
Traces: s3.put-object-policy-requires-if-none-match-header, s3.delete-object-policy-requires-if-match-header, s3.copy-object-under-if-none-match-policy.

Cause 4 of 7: aws-chunked (STREAMING ... TRAILER) uploads are accepted without validating or storing the trailer.
AWS: for a chunked PUT, a missing x-amz-decoded-content-length is refused 411 (MissingContentLength); a trailing-chunk checksum that does not match the payload is refused 400 (BadDigest/InvalidRequest) and no object is created; a trailer whose chunk header name differs from the x-amz-trailer value is refused 400; and a valid trailer checksum is stored so HeadObject reports it. Smithy: the checksum members (x-amz-checksum-crc32 etc., httpChecksum trailer support) are modeled on PutObject/CompleteMultipartUpload, so the stored ChecksumCRC32 is part of the object's metadata.
Emulator: all three malformed uploads answer HTTP 200 with an empty body and create the object, and after a valid trailer upload HeadObject reports ChecksumCRC32=None.
Why: corruption detection on the primary SDK upload path is gone — a truncated or corrupted stream is stored as if intact, and callers that read back the checksum to verify integrity get nothing.
Fix: the S3 PutObject body reader must parse aws-chunked framing: require x-amz-decoded-content-length (411), verify the declared trailer name and its checksum against the decoded payload (400, object not created), and persist the trailer checksum into object metadata.
Traces: s3.s3-chunked-upload-missing-decoded-content-length-rejected-object-absent, s3.s3-chunked-upload-trailer-checksum-mismatch-rejected-object-absent, s3.s3-chunked-upload-trailer-name-mismatch-rejected-object-absent, s3.s3-chunked-upload-trailer-checksum-stores-object-and-checksum.

Cause 5 of 7: CopyObject with If-Match is an unimplemented scaffold stub.
AWS: Smithy s3 CopyObject models IfMatch (header 'If-Match') and IfNoneMatch (header 'If-None-Match') on CopyObjectRequest; a copy whose If-Match does not match the destination's current ETag is refused HTTP 412 PreconditionFailed with message 'At least one of the pre-conditions you specified did not hold'.
Emulator: the call answers HTTP 501 NotImplemented 'operation CopyObject with If-Match is scaffolded but not implemented in this emulator' (fixed text, with the member name taken from the request).
Why: conditional copies cannot protect a destination from a concurrent overwrite, and the caller gets a 501 — a retriable-looking server error — instead of the 412 that tells it to re-read and retry.
Fix: implement the If-Match/If-None-Match precondition evaluation against the destination object in the CopyObject handler.
Traces: s3.copy-object-if-match-on-the-destination-refuses-a-stale-etag.

Cause 6 of 7: a presigned GET of a multipart-assembled object is refused 403 instead of serving the bytes.
AWS: a valid, unexpired presigned GET URL serves the object — including one assembled by CompleteMultipartUpload from sparse, reverse-order part numbers — with HTTP 200 and the concatenated part bytes.
Emulator: the presigned GET answers HTTP 403 (reported as 'DATAPLANE_UNIMPLEMENTED: the presigned GET URL did not serve the object: HTTPError: HTTP Error 403: Forbidden'), so the multipart round-trip cannot be verified.
Why: the multipart upload path cannot be validated through the presigned URL any browser-based download uses; the object may be assembled correctly but is unreadable by the documented means.
Fix: the S3 request authenticator must verify query-string (presigned) SigV4 for GetObject so the object plane serves these requests.
Traces: s3.multipart-upload-with-sparse-part-numbers-uploaded-in-reverse-round-trips.

Cause 7 of 7: the SSE-KMS encryption context echoed by PutObject drops the implicit aws:s3:arn entry.
AWS: for SSE-KMS, S3 adds the object's ARN to the encryption context under the key 'aws:s3:arn' and the response's x-amz-server-side-encryption-context (Smithy: s3 PutObject SSEKMSEncryptionContext, header 'x-amz-server-side-encryption-context', base64 JSON) is the full context including that entry alongside the caller's pairs.
Emulator: the echoed context contains only the caller's pairs — recorded as {'team': ...} where AWS returns {'team': ..., 'aws:s3:arn': 'arn:aws:s3:::<bucket>/<key>'}; the fixed part is the aws:s3:arn key and the object-ARN form, the bucket/key values come from the request.
Why: a caller that reproduces the context to decrypt out-of-band, or whose KMS key policy constrains kms:EncryptionContext:aws:s3:arn, computes a different context than S3 used and the grant/decrypt fails.
Fix: the PutObject SSE-KMS path must inject aws:s3:arn into the encryption context it uses and serialize that full context in x-amz-server-side-encryption-context.
Traces: s3.put-object-sse-kms-encryption-context-round-trips-then-malformed-rejected.

## Evidence
Cause 1: AssertionError: the preflight with {'Origin': 'https://app.example.com', 'Access-Control-Request-Method': 'PUT'} answered 501, not 200: b'<?xml version="1.0"?><Error><Code>NotImplemented</Code><Message>operation OPTIONS /<bucket>/<key> is not implemented in this emulator</Message>...' (same for GET and for the no-CORS-config/deleted-CORS cases).
Cause 2: simparity.behavioral.context.CapabilityNotOffered: the emulator offers no S3 website endpoint; and simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-s3site-<id>.s3-website-us-west-2.amazonaws.com/ ... its host is not one of the emulator's own hosts.
Cause 3: TimeoutError: expected AccessDenied within 20s, last outcome: ('SUCCESS', {... 'HTTPStatusCode': 200 ...}) for PutObject without If-None-Match; ('SUCCESS', {... 204 ...}) for DeleteObject without If-Match; ('SUCCESS', {... 200, CopyObjectResult ...}) for CopyObject.
Cause 4: AssertionError: a chunked PUT with no x-amz-decoded-content-length must be refused 411, got 200: b''; a trailer CRC32 that does not match the payload must be refused 400, got 200: b''; a trailer chunk whose header name does not match x-amz-trailer must be refused 400, got 200: b''; HeadObject reports ChecksumCRC32=None, expected 'AXX3/Q=='.
Cause 5: AssertionError: CopyObject If-Match on a stale destination ETag: expected ('PreconditionFailed', 'At least one of the pre-conditions you specified did not hold', 412), got ('NotImplemented', 'operation CopyObject with If-Match is scaffolded but not implemented in this emulator', 501).
Cause 6: AssertionError: DATAPLANE_UNIMPLEMENTED: the presigned GET URL did not serve the object: HTTPError: HTTP Error 403: Forbidden.
Cause 7: AssertionError: PutObject echoed context {'team': 'sim2real'}, expected {'team': 'sim2real', 'aws:s3:arn': 'arn:aws:s3:::<bucket>/context-echo.txt'}.

## Steps to Reproduce
Cause 1: put_bucket_cors with a rule allowing an origin and PUT/GET, then send OPTIONS /<bucket>/<key> with Origin + Access-Control-Request-Method -> 501 NotImplemented.
Cause 2: put_bucket_website(IndexDocument/ErrorDocument/RoutingRules) and fetch the bucket's website endpoint -> no endpoint offered, or an announced <bucket>.s3-website-<region>.amazonaws.com host the emulator does not serve.
Cause 3: put_bucket_policy denying s3:PutObject/DeleteObject/CopyObject unless the request carries If-None-Match/If-Match; make the call without the header -> it succeeds.
Cause 4: PUT with Content-Encoding aws-chunked and x-amz-trailer=x-amz-checksum-crc32, (a) omitting x-amz-decoded-content-length, (b) with a wrong trailer CRC32, (c) with a mismatched trailer name, (d) correct -> (a)-(c) answer 200 and create the object, (d) stores no ChecksumCRC32.
Cause 5: put two objects, then copy_object(..., IfMatch=<stale destination ETag>) -> 501 NotImplemented.
Cause 6: create_multipart_upload, upload sparse part numbers in reverse, complete, then fetch a generate_presigned_url('get_object') URL -> 403.
Cause 7: put_object(..., ServerSideEncryption='aws:kms', SSEKMSKeyId=<key>, SSEKMSEncryptionContext=base64({'team':'sim2real'})) and decode the response's x-amz-server-side-encryption-context -> no aws:s3:arn entry.
