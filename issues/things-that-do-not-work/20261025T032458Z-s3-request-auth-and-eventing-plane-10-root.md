Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-put-object-with-a-deliberately-wrong-crc32-is-rejected-and-the-key-is-absent s3.s3-csv-flexible-checksums-round-trip-on-get-with-checksum-mode s3.deep-s3-immutability-composite-sha256-checksum-reports-parts s3.s3checksum-incompatible-algorithm-and-type-pairs-rejected s3.s3checksum-digest-is-a-property-of-the-bytes-in-a-derived-second-region s3.s3checksum-sha256-mismatch-refuses-the-overwrite-and-the-stored-bytes-survive s3.sigv4skew-cross-region-credential-scope-rejected s3.sigv4skew-future-dated-put-outside-window-rejected s3.sigv4skew-window-enforced-in-derived-second-region-rejected s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry s3.post-object-form-upload-enforces-its-policy-conditions s3.presigned-post-form-upload-substitutes-filename-and-matches-content-type s3.identity-policy-s3-prefix-condition-scopes-listbucket-and-getobject-to-a-home-prefix s3.s3-csv-explicit-deny-on-the-csv-key-denies-the-owner-but-not-other-keys s3.secure-transport-policy-plain-http-denied s3.s3-ssec-and-endpoint-enforcement-secure-transport-deny-policy-refuses-plaintext-http-and-allows-https s3.notification-invalid-destination-rejected s3.op-contract-putBucketNotificationConfiguration s3.notification-to-sqs s3.notification-filter-non-matching-key-not-delivered s3.s3crr-new-object-replicates-byte-identical-to-a-derived-peer-region s3.s3crr-delete-marker-replicates-only-where-marker-replication-is-enabled s3.s3crr-a-version-delete-is-never-replicated-even-with-marker-replication-enabled s3.s3crr-a-write-to-the-replica-never-propagates-back-to-the-source s3.s3crr-object-written-before-the-rule-is-never-backfilled s3.s3crr-replication-fails-visibly-when-the-role-cannot-write-the-destination s3.replication-covers-the-eligible-prefix-only-and-withholds-the-delete-marker s3.get-bucket-replication-not-configured-rejected s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body s3.write-get-object-response-roundtrip s3.write-get-object-response-lambda-callback s3.write-get-object-response-lambda-pattern s3.write-get-object-response-metadata-headers-passthrough s3.write-get-object-response-error-status-relay s3.write-get-object-response-invalid-request-token-rejected s3.e2e-object-lambda-error-passthrough-flow
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-put-object-with-a-deliberately-wrong-crc32-is-rejected-and-the-key-is-absent|run#9|AssertionError|assert exc is not None, (
Issue-Key: s3.s3-csv-flexible-checksums-round-trip-on-get-with-checksum-mode|run#4|AssertionError|assert (resp.get(field), resp.get("ChecksumType")) == (value, "FULL_OBJECT"), (
Issue-Key: s3.deep-s3-immutability-composite-sha256-checksum-reports-parts|run#8|AssertionError|assert up.get("ChecksumAlgorithm") == "SHA256" and up.get("ChecksumType") == "COMPOSITE", (
Issue-Key: s3.s3checksum-incompatible-algorithm-and-type-pairs-rejected|run#4|AssertionError|assert sha_full is not None, (
Issue-Key: s3.s3checksum-digest-is-a-property-of-the-bytes-in-a-derived-second-region|run#10|KeyError|assert home_put["ChecksumSHA256"] == want, (
Issue-Key: s3.s3checksum-sha256-mismatch-refuses-the-overwrite-and-the-stored-bytes-survive|run#10|KeyError|assert first["ChecksumSHA256"] == v1_digest, (
Issue-Key: s3.sigv4skew-cross-region-credential-scope-rejected|run#11|AssertionError|assert status == 400, (
Issue-Key: s3.sigv4skew-future-dated-put-outside-window-rejected|run#9|AssertionError|assert status == 403, (
Issue-Key: s3.sigv4skew-window-enforced-in-derived-second-region-rejected|run#8|AssertionError|assert status == 403, f"the home-region skewed PUT must be refused 403, got {status}"
Issue-Key: s3|PutObject|SignatureDoesNotMatch
Issue-Key: s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry|run#17|AssertionError|assert _xml_error(body) == ("SignatureDoesNotMatch", "The request signature we calculated does not match the signature you provided. Check your key and signing
Issue-Key: s3.post-object-form-upload-enforces-its-policy-conditions|run#8|AssertionError|assert status == 201, (
Issue-Key: s3.presigned-post-form-upload-substitutes-filename-and-matches-content-type|run#11|AssertionError|assert status == 201, (
Issue-Key: s3.identity-policy-s3-prefix-condition-scopes-listbucket-and-getobject-to-a-home-prefix|_client_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: s3.s3-csv-explicit-deny-on-the-csv-key-denies-the-owner-but-not-other-keys|_poll_refusal#10|Timeout|time.sleep(interval_s)
Issue-Key: s3.secure-transport-policy-plain-http-denied|run#10|ConnectionClosedError|base = http_s3.get_object(Bucket=bucket, Key="a.txt")["Body"].read()
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-secure-transport-deny-policy-refuses-plaintext-http-and-allows-https|_http#9|BadStatusLine|with _OPENER.open(req, timeout=60) as resp:
Issue-Key: s3.notification-invalid-destination-rejected|_err_code#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.op-contract-putBucketNotificationConfiguration|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.notification-to-sqs|run#15|AssertionError|assert received is not None, "No S3 event notification received within 30s"
Issue-Key: s3.notification-filter-non-matching-key-not-delivered|_drain_records#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: s3.s3crr-new-object-replicates-byte-identical-to-a-derived-peer-region|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3crr-delete-marker-replicates-only-where-marker-replication-is-enabled|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3crr-a-version-delete-is-never-replicated-even-with-marker-replication-enabled|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3crr-a-write-to-the-replica-never-propagates-back-to-the-source|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3crr-object-written-before-the-rule-is-never-backfilled|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3crr-replication-fails-visibly-when-the-role-cannot-write-the-destination|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: s3.replication-covers-the-eligible-prefix-only-and-withholds-the-delete-marker|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: s3.get-bucket-replication-not-configured-rejected|run#7|AssertionError|assert never_configured.get("Message") == "The replication configuration was not found", (
Issue-Key: s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes|_await_http#13|Timeout|time.sleep(3.0)
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys|_await#9|Timeout|time.sleep(3.0)
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body|_error#10|AssertionError|raise AssertionError("the call was expected to be refused and it succeeded")
Issue-Key: s3|WriteGetObjectResponse|404
Issue-Related: 20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# S3 request/auth and eventing plane: 10 root causes — checksums unverified, SigV4 scope and skew unchecked, POST Object unrouted, no policy evaluation, no HTTPS listener, notifications, replication, website endpoint, Object Lambda

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Ten distinct net-new root causes across the S3 request/auth path and its event/derived-data planes. Each is numbered with its own failing check ids; all ten must be closed before this issue is submitted.

Cause 1 of 10: flexible checksums are neither verified on write nor reported on read.
AWS: Smithy s3#PutObject carries @httpChecksum{requestAlgorithmMember: ChecksumAlgorithm} and declares ChecksumCRC32/CRC32C/CRC64NVME/SHA1/SHA256/... request and response headers; s3#GetObject carries @httpChecksum{requestValidationModeMember: ChecksumMode, responseAlgorithms: [CRC64NVME, CRC32, ...]} and declares ChecksumType. A supplied checksum that does not match the body is refused BadDigest/400; a GET with ChecksumMode=ENABLED returns the stored digest and ChecksumType; CreateMultipartUpload with SHA256 implies ChecksumType COMPOSITE and SHA256+FULL_OBJECT is refused InvalidRequest.
Emulator: the header is stored, not verified — `PutObject accepted ChecksumCRC32='OttwbA==' for a body whose actual CRC32 is 'YUvSPw=='`; GET with ChecksumMode returns nothing (`must return ChecksumCRC32=DwkUNQ== FULL_OBJECT; got None, None`), CreateMultipartUpload echoes `ChecksumType None` where COMPOSITE is required and accepts SHA256+FULL_OBJECT, and ChecksumSHA256 is absent from responses (`KeyError: 'ChecksumSHA256'`).
Why: integrity is the point of the feature — every corrupted upload is silently accepted and no reader can detect it.
Fix: the PutObject/UploadPart/CreateMultipartUpload/GetObject checksum computation, validation and serialization.
Traces: s3.s3-put-object-with-a-deliberately-wrong-crc32-is-rejected-and-the-key-is-absent, s3.s3-csv-flexible-checksums-round-trip-on-get-with-checksum-mode, s3.deep-s3-immutability-composite-sha256-checksum-reports-parts, s3.s3checksum-incompatible-algorithm-and-type-pairs-rejected, s3.s3checksum-digest-is-a-property-of-the-bytes-in-a-derived-second-region, s3.s3checksum-sha256-mismatch-refuses-the-overwrite-and-the-stored-bytes-survive

Cause 2 of 10: the SigV4 credential scope region and the request date window are not validated.
AWS: a request whose credential scope names a different region than the endpoint is refused 400 (AuthorizationHeaderMalformed / credential-scope error); a request signed more than 15 minutes from the service clock is refused 403 RequestTimeTooSkewed.
Emulator: both are accepted with 200 — `a PUT scoped to 'us-west-2' but sent to 'ap-northeast-1' must be refused 400, got 200`, `a PUT signed 1500 s ahead of S3's clock must be refused 403, got 200`.
Why: signing bugs and clock-skew bugs are invisible locally and only appear against AWS; the emulator also cannot be used to test a client's skew/retry handling.
Fix: the SigV4 verification layer in front of the S3 handlers.
Traces: s3.sigv4skew-cross-region-credential-scope-rejected, s3.sigv4skew-future-dated-put-outside-window-rejected, s3.sigv4skew-window-enforced-in-derived-second-region-rejected

Cause 3 of 10: virtual-hosted-style and presigned requests fail signature verification.
AWS: a request addressed as <bucket>.s3.<region>.amazonaws.com with the bucket hoisted into the Host header verifies and succeeds; a correctly presigned PUT succeeds, and only a tampered one is refused SignatureDoesNotMatch/403.
Emulator: a virtual-hosted-style PutObject is refused — `An error occurred (SignatureDoesNotMatch) when calling the PutObject operation: The request signature does not match` — and the presigned-PUT check cannot observe the refusal it expects because the signature path answers SignatureDoesNotMatch unconditionally.
Why: virtual-hosted addressing is the SDK default for S3, so a large class of clients cannot talk to the emulator at all.
Fix: canonical-request construction for host-style addressing and query-presigned requests in the S3 SigV4 verifier.
Traces: s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion, s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry

Cause 4 of 10: POST Object (HTML form upload) is not routed at all.
AWS: POST /<bucket>/ with a multipart/form-data body carrying a policy, signature and key is the browser-upload API; a form satisfying every policy condition and carrying success_action_status=201 is answered 201.
Emulator: the request never reaches S3 — the router answers 404 with `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`.
Why: POST Object has no Authorization header by design, so the router's service-resolution rule excludes it; every browser direct-to-S3 upload is unusable.
Fix: the front-door request router's service resolution (recognise bucket-addressed POST form uploads) plus a PostObject handler.
Traces: s3.post-object-form-upload-enforces-its-policy-conditions, s3.presigned-post-form-upload-substitutes-filename-and-matches-content-type

Cause 5 of 10: bucket policies and identity policies are not evaluated on the data plane.
AWS: an identity policy whose s3:prefix condition scopes ListBucket to one prefix refuses a listing of another prefix with AccessDenied/403; an explicit Deny on one key denies even the bucket owner for that key.
Emulator: the scoped call succeeds — `expected the call to be rejected, but it succeeded and returned {... 'Prefix': 'home/some-other-user/', 'KeyCount': 1}` — and the explicit-deny check never reaches a verdict, hanging to the 60 s cap.
Why: no authorization test is meaningful; a policy that would deny in AWS allows here, which is the worst possible direction for a security review.
Fix: policy evaluation (bucket policy + identity policy + condition keys) in the S3 request authorizer.
Traces: s3.identity-policy-s3-prefix-condition-scopes-listbucket-and-getobject-to-a-home-prefix, s3.s3-csv-explicit-deny-on-the-csv-key-denies-the-owner-but-not-other-keys

Cause 6 of 10: there is no TLS listener, so aws:SecureTransport policies cannot be exercised and HTTPS requests get a malformed response.
AWS: S3 serves HTTPS; a bucket policy denying requests where aws:SecureTransport is false refuses a plain-HTTP request with AccessDenied/403 and allows the same request over HTTPS.
Emulator: the plain-HTTP request is not answered at all — `botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "http://127.0.0.1:<port>/<bucket>/a.txt"` — and the HTTPS leg gets a malformed status line, `http.client.BadStatusLine:  2`. The emulator is announcing a scheme/endpoint it does not serve.
Why: the single most common S3 hardening statement cannot be tested, and the connection-level failure is indistinguishable from a network fault instead of a 403 AccessDenied.
Fix: the S3 front-door listener (serve TLS, or answer the policy decision on the plain-HTTP port) and aws:SecureTransport condition evaluation.
Traces: s3.secure-transport-policy-plain-http-denied, s3.s3-ssec-and-endpoint-enforcement-secure-transport-deny-policy-refuses-plaintext-http-and-allows-https

Cause 7 of 10: PutBucketNotificationConfiguration validates no destination and no event is ever delivered.
AWS: PutBucketNotificationConfiguration resolves each destination and refuses an unresolvable or unauthorized one with InvalidArgument/400; a valid put emits an s3:TestEvent to the destination immediately, and subsequent object writes emit s3:ObjectCreated:* events.
Emulator: an invalid destination is accepted — `expected the call to be rejected, but it SUCCEEDED and returned {... 200 ...}` — no s3:TestEvent is produced, and no object event is ever delivered (`No S3 event notification received within 30s`; the filter check hangs to the 60 s cap).
Why: an event-driven pipeline wired exactly as AWS documents produces nothing, and a misconfiguration that AWS catches at configuration time is only discovered as silence.
Fix: destination resolution/validation in the PutBucketNotificationConfiguration handler plus the event emitter on the object write paths.
Traces: s3.notification-invalid-destination-rejected, s3.op-contract-putBucketNotificationConfiguration, s3.notification-to-sqs, s3.notification-filter-non-matching-key-not-delivered

Cause 8 of 10: cross-region replication never replicates.
AWS: with a replication configuration and a role that can write the destination, a new object appears byte-identical in the destination bucket and x-amz-replication-status is reported; marker replication, prefix scoping and the 'no backfill of pre-existing objects' rule are all observable; GetBucketReplication on a bucket with no configuration is refused ReplicationConfigurationNotFoundError/404.
Emulator: every replication check hangs to the 60 s cap waiting for an object that never arrives; the not-configured read does not produce the expected refusal.
Why: a DR/compliance topology reports healthy while copying nothing, which is the one failure mode replication exists to rule out.
Fix: the replication engine driven off the bucket replication configuration, and the GetBucketReplication not-found path.
Traces: s3.s3crr-new-object-replicates-byte-identical-to-a-derived-peer-region, s3.s3crr-delete-marker-replicates-only-where-marker-replication-is-enabled, s3.s3crr-a-version-delete-is-never-replicated-even-with-marker-replication-enabled, s3.s3crr-a-write-to-the-replica-never-propagates-back-to-the-source, s3.s3crr-object-written-before-the-rule-is-never-backfilled, s3.s3crr-replication-fails-visibly-when-the-role-cannot-write-the-destination, s3.replication-covers-the-eligible-prefix-only-and-withholds-the-delete-marker, s3.get-bucket-replication-not-configured-rejected

Cause 9 of 10: the S3 website endpoint serves nothing.
AWS: a bucket with a website configuration serves the index document at the root and for directory-style keys, serves the error document with the configured status, honours routing rules on 404, answers 301 with Location for an object carrying x-amz-website-redirect-location, and refuses writes on the website endpoint.
Emulator: every website check hangs to the 60 s cap, and the redirect check records `the call was expected to be refused and it succeeded`.
Why: static-site hosting — including the CloudFront-origin pattern — cannot be exercised at all.
Fix: the website endpoint handler (index/error document resolution, routing rules, redirect handling, method restriction).
Traces: s3.website-endpoint-serves-index-and-error-documents-and-refuses-writes, s3.s3-ssec-and-endpoint-enforcement-website-routing-rule-on-404-redirects-missing-keys, s3.s3-ssec-and-endpoint-enforcement-website-redirect-location-answers-301-while-rest-serves-the-body

Cause 10 of 10: GetObject through an S3 Object Lambda access point fails with NotImplemented naming ListObjects, so WriteGetObjectResponse is unreachable.
AWS: a GET against an Object Lambda access point invokes the transform function, which replies with WriteGetObjectResponse; the relayed status, body, metadata headers and error status all reach the caller, and a bogus request route/token is refused InvalidRequest.
Emulator: `An error occurred (NotImplemented) when calling the GetObject operation: operation ListObjects is scaffolded but not implemented in this emulator` — the GetObject path for an Object Lambda access point calls into the unimplemented ListObjects stub — and a bogus route/token yields a bare '404' instead of an S3 error code.
Why: the entire Object Lambda surface is unusable, and the error names an operation the caller never invoked, which misdirects diagnosis.
Fix: the Object Lambda access-point GetObject path and the WriteGetObjectResponse route (and its dependency on ListObjects).
Traces: s3.write-get-object-response-roundtrip, s3.write-get-object-response-lambda-callback, s3.write-get-object-response-lambda-pattern, s3.write-get-object-response-metadata-headers-passthrough, s3.write-get-object-response-error-status-relay, s3.write-get-object-response-invalid-request-token-rejected, s3.e2e-object-lambda-error-passthrough-flow

## Evidence
Cause 1: `AssertionError: PutObject accepted ChecksumCRC32='OttwbA==' for a body whose actual CRC32 is 'YUvSPw==' — the header is being stored rather than verified`; `GET with ChecksumMode must return ChecksumCRC32=DwkUNQ== FULL_OBJECT; got None, None`; `CreateMultipartUpload echoed ChecksumAlgorithm 'SHA256', ChecksumType None; want SHA256 / COMPOSITE`; `CreateMultipartUpload accepted ChecksumAlgorithm=SHA256 with ChecksumType=FULL_OBJECT`; `KeyError: 'ChecksumSHA256'` (x2).
Cause 2: `a PUT scoped to 'us-west-2' but sent to 'ap-northeast-1' must be refused 400, got 200: b'<PutObjectOutput .../>'`; `a PUT signed 1500 s ahead of S3's clock must be refused 403, got 200`; `the home-region skewed PUT must be refused 403, got 200`.
Cause 3: `botocore.exceptions.ClientError: An error occurred (SignatureDoesNotMatch) when calling the PutObject operation: The request signature does not match` on a virtual-hosted-style round trip; `the refused presigned PUT returned no SignatureDoesNotMatch error: b'<?xml ...><Code>SignatureDoesNotMatch</Code>...'`.
Cause 4: `the conforming POST Object was answered 404 with b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'; a POST whose form satisfies every policy condition and carries success_action_status=201 must be accepted with 201` (and the same for the presigned POST).
Cause 5: `expected the call to be rejected, but it succeeded and returned {... 'Prefix': 'home/some-other-user/', 'KeyCount': 1}`; `Error: Check timed out after 60.0s` for the explicit-deny check.
Cause 6: `botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "http://127.0.0.1:36229/<bucket>/a.txt"`; `http.client.BadStatusLine:  2`. Expected AccessDenied/403 on HTTP and 200 on HTTPS.
Cause 7: `expected the call to be rejected, but it SUCCEEDED and returned {... 200 ...}` (invalid notification destination); `expected a ClientError, but the call SUCCEEDED and returned {... 200 ...}`; `No S3 event notification received within 30s`; `Error: Check timed out after 60.0s`.
Cause 8: `Error: Check timed out after 60.0s` for all six s3crr checks and the prefix-scoping check; `AssertionError: There is no replication configuration for this bucket`.
Cause 9: `Error: Check timed out after 60.0s` (website index/error documents, routing rule on 404); `AssertionError: the call was expected to be refused and it succeeded`.
Cause 10: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetObject operation: operation ListObjects is scaffolded but not implemented in this emulator` in six checks; `AssertionError: unexpected error code for a bogus route/token: '404'`.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, re-running the check ids listed under each cause's Traces line (all ids are in trace_ids). Manual repros: (1) PutObject with ChecksumCRC32 set to a wrong digest, then GetObject with ChecksumMode=ENABLED; (2) sign a PutObject with a credential scope naming a different region, and one dated 1500 s in the future; (3) issue a PutObject with the bucket in the Host header (virtual-hosted addressing); (4) POST a multipart/form-data upload to /<bucket>/ with policy+signature fields; (5) attach an identity policy whose s3:prefix condition scopes ListBucket, then list another prefix; (6) attach a bucket policy denying aws:SecureTransport=false and request over plain HTTP, then over HTTPS; (7) PutBucketNotificationConfiguration naming a nonexistent queue, then a valid one, then PutObject and wait for the event; (8) PutBucketReplication between two buckets, PutObject in the source, poll the destination; (9) PutBucketWebsite then GET the website endpoint root; (10) GET through an Object Lambda access point.
