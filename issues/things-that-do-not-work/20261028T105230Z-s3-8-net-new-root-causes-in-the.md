Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.post-object-form-upload-enforces-its-policy-conditions s3.presigned-post-form-upload-substitutes-filename-and-matches-content-type s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion s3.sigv4skew-future-dated-put-outside-window-rejected s3.sigv4skew-window-enforced-in-derived-second-region-rejected s3.sigv4skew-cross-region-credential-scope-rejected s3.s3-ssec-and-endpoint-enforcement-secure-transport-deny-policy-refuses-plaintext-http-and-allows-https s3.secure-transport-policy-plain-http-denied s3.s3-csv-explicit-deny-on-the-csv-key-denies-the-owner-but-not-other-keys s3.identity-policy-s3-prefix-condition-scopes-listbucket-and-getobject-to-a-home-prefix s3.write-get-object-response-invalid-request-token-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.post-object-form-upload-enforces-its-policy-conditions|run#8|AssertionError|assert status == 201, (
Issue-Key: s3.presigned-post-form-upload-substitutes-filename-and-matches-content-type|run#11|AssertionError|assert status == 201, (
Issue-Key: s3|PutObject|SignatureDoesNotMatch
Issue-Key: s3.sigv4skew-future-dated-put-outside-window-rejected|run#9|AssertionError|assert status == 403, (
Issue-Key: s3.sigv4skew-window-enforced-in-derived-second-region-rejected|run#8|AssertionError|assert status == 403, f"the home-region skewed PUT must be refused 403, got {status}"
Issue-Key: s3.sigv4skew-cross-region-credential-scope-rejected|run#11|AssertionError|assert status == 400, (
Issue-Key: s3.s3-ssec-and-endpoint-enforcement-secure-transport-deny-policy-refuses-plaintext-http-and-allows-https|_http#9|BadStatusLine|with _OPENER.open(req, timeout=60) as resp:
Issue-Key: s3.secure-transport-policy-plain-http-denied|run#10|ConnectionClosedError|base = http_s3.get_object(Bucket=bucket, Key="a.txt")["Body"].read()
Issue-Key: s3.s3-csv-explicit-deny-on-the-csv-key-denies-the-owner-but-not-other-keys|_poll_refusal#10|Timeout|time.sleep(interval_s)
Issue-Key: s3.identity-policy-s3-prefix-condition-scopes-listbucket-and-getobject-to-a-home-prefix|_client_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: s3|WriteGetObjectResponse|404
Issue-Commit: a991e58b63dd0e61a72e74cc1f818b9fb1c8de8d

# s3: 8 net-new root causes in the request/auth surface — POST Object unrouted, virtual-hosted SigV4 rejected, no clock-skew or credential-scope checks, plain HTTP unserved, bucket/identity policy never evaluated, WriteGetObjectResponse host validation

> _Note: this issue was identified against the code base at commit `a991e58b63dd0e61a72e74cc1f818b9fb1c8de8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
This issue bundles 8 distinct net-new root causes in the S3 request-admission surface (addressing, SigV4 verification, transport, policy authorization). All 8 must be closed before submitting; each cause below lists its own failing check ids.

===== Cause 1 of 8: POST Object (browser-based HTML form upload) is not routed at all =====
AWS: S3 exposes POST Object as a browser form upload: an HTTP POST to the bucket root (`POST /` on the bucket, or `POST /<bucket>` path-style) with a `multipart/form-data` body carrying `key`, `policy`, `x-amz-credential`, `x-amz-algorithm`, `x-amz-date`, `x-amz-signature` and `success_action_status`. The request carries NO `Authorization` header and no `X-Amz-Target`; the credential scope travels in the form field `x-amz-credential`. When every policy condition is satisfied and `success_action_status=201` is present, S3 answers HTTP 201 with a `<PostResponse>` document. POST Object is a REST-only operation and is not in the Smithy model (recorded behavior only; the checks' own reference is the S3 POST Object API).
Emulator: both conforming POSTs are answered HTTP 404 with a JSON body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`. The generic multi-service router cannot attribute the request to S3 because it only looks at an `Authorization` SigV4 credential scope, `X-Amz-Target` or the Host header, so the request never reaches an S3 handler.
Why: every browser/mobile direct-upload integration (presigned POST) is impossible against the emulator, and the refusal is not even an S3-shaped error — a client gets a non-AWS JSON `UnknownService` body with a 404 where AWS returns 201 or an S3 XML error.
Fix: the request router's service attribution must recognise an S3 POST form upload (a `multipart/form-data` POST whose path is a bucket root) and dispatch it to a POST Object handler in the s3 crate that verifies the form policy and signature and honours `success_action_status`/`success_action_redirect`.
Traces: s3.post-object-form-upload-enforces-its-policy-conditions (conforming POST answered 404, expected 201); s3.presigned-post-form-upload-substitutes-filename-and-matches-content-type (same 404 body).

===== Cause 2 of 8: virtual-hosted-style addressing fails SigV4 verification =====
AWS: a bucket may be addressed virtual-hosted style — the bucket name is the leading label of the Host header and the URI path omits the bucket (`Host: <bucket>.<endpoint>`, `PUT /vhost/object.bin`). SigV4's canonical request is built from the request as signed, i.e. the signed `host` header and the bucket-less path, so an SDK using `addressing_style=virtual` round-trips PutObject/HeadObject/GetObject/ListObjectsV2/DeleteObject normally (Smithy: s3 is `aws.protocols#restXml` with `aws.auth#sigv4`; the bucket is an `httpLabel` only in the path-style form).
Emulator: the very first virtual-hosted PutObject is refused `SignatureDoesNotMatch` — "The request signature we calculated does not match the signature you provided. Check your key and signing method." The verifier evidently recomputes the canonical request from the path-style URI it was reached on (bucket in the path) and/or the socket Host rather than from the signed `Host` header and the request line as received, so the computed signature can never match.
Why: any SDK or tool configured for virtual-hosted addressing (the AWS default for real S3) is completely unusable; the failure is reported as a credentials problem, sending users to debug keys that are correct.
Fix: the s3 crate's SigV4 request-authentication layer must canonicalize using the `Host` header exactly as signed and the received request-target, and must strip a leading bucket label from Host (rather than from the path) when resolving the bucket.
Traces: s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion.

===== Cause 3 of 8: the SigV4 clock-skew window is not enforced (RequestTimeTooSkewed is never returned) =====
AWS: a SigV4-signed request whose `X-Amz-Date` is outside S3's acceptance window (±15 minutes) is refused with HTTP 403 and error code `RequestTimeTooSkewed`, message `The difference between the request time and the current time is too large.` (that message is entirely fixed text, no substitutions). The error XML additionally carries `<RequestTime>` echoing the signed `X-Amz-Date` in `YYYYMMDDTHHMMSSZ` form and `<ServerTime>` holding the server's own clock in ISO-8601 — so for a future-dated request ServerTime is earlier than RequestTime. A request inside the window (e.g. 240 s ahead) is accepted normally. Smithy: `RequestTimeTooSkewed` is not a modeled member of PutObject's `errors`, so this is recorded behavior per the S3 sig-v4 authentication reference.
Emulator: a PUT signed 1500 s ahead of the server's own clock is accepted with HTTP 200 and an empty `<PutObjectOutput>` document, and the object is written. The same happens in the derived-second-region variant: the home-region skewed PUT answers 200 instead of 403.
Why: skew is a real production failure mode (drifting hosts, replayed/stale presigned requests). A client that tests its skew handling against the emulator sees stale signatures silently accepted, and an attacker-replayed signature never expires.
Fix: the s3 crate's SigV4 verification layer must compare the signed `X-Amz-Date` against the server clock, refuse outside the window with 403/`RequestTimeTooSkewed` and serialize `RequestTime`/`ServerTime` into the error XML.
Traces: s3.sigv4skew-future-dated-put-outside-window-rejected; s3.sigv4skew-window-enforced-in-derived-second-region-rejected.

===== Cause 4 of 8: the SigV4 credential-scope region is not validated against the bucket's region =====
AWS: a request signed with a credential scope naming region A but addressed to a bucket that lives in region B is refused with HTTP 400 and error code `AuthorizationHeaderMalformed`. Its message is `The authorization header is malformed; the region '<signed-region>' is wrong; expecting '<bucket-region>'` — the fixed text is "The authorization header is malformed; the region '", "' is wrong; expecting '", "'"; the two variable parts are the region taken from the request's credential scope and the bucket's actual region taken from emulator state. The error XML also carries a `<Region>` element holding that expected region. Nothing is written.
Emulator: the mis-scoped PUT is accepted with HTTP 200 and an empty `<PutObjectOutput>`, and the object is created in the addressed bucket.
Why: region-scoping bugs in client configuration are invisible, and the emulator diverges from AWS both in status (200 vs 400) and in effect (an object exists that AWS would never have created).
Fix: the s3 crate's SigV4 verification layer must compare the credential-scope region with the resolved bucket's region and emit the AuthorizationHeaderMalformed refusal (with its `Region` element) before the operation handler runs.
Traces: s3.sigv4skew-cross-region-credential-scope-rejected.

===== Cause 5 of 8: the endpoint answers nothing valid over plain HTTP, so aws:SecureTransport can never be exercised =====
AWS: S3 serves the same REST API over both HTTP and HTTPS. A plain-HTTP signed request succeeds when no policy forbids it; once a bucket policy adds `Effect: Deny` with `Condition: {"Bool": {"aws:SecureTransport": "false"}}`, the same plain-HTTP GetObject/PutObject/ListObjectsV2 are refused HTTP 403 `AccessDenied` with the explicit-deny message `User: <caller ARN> is not authorized to perform: s3:PutObject on resource: "<resource ARN>" with an explicit deny in a resource-based policy`, while HTTPS keeps working. Recorded behavior only (the condition key is an IAM/S3 policy feature, not modeled in Smithy); reference: the S3 example-bucket-policies guide the check cites.
Emulator: a plain-HTTP request to the emulator's own announced endpoint host:port never gets a valid HTTP response at all — the baseline plain-HTTP GetObject (made BEFORE any policy exists) fails with `botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "http://<host>:<port>/<bucket>/a.txt"`, and the second check's raw plain-HTTP request dies in the client with `http.client.BadStatusLine: 2` — i.e. the listener answered TLS bytes to an HTTP request. The endpoint it announces is reachable only over TLS, so the entire aws:SecureTransport behaviour (both the permissive baseline and the Deny) is unreachable.
Why: a client cannot exercise or regression-test any `aws:SecureTransport` policy, and a plain-HTTP caller gets a transport-level abort instead of either a 200 or an S3 `AccessDenied` — it cannot tell a policy refusal from a broken endpoint.
Fix: the s3 crate's listener/front door must serve the plain-HTTP scheme on the endpoint it announces (or answer it with a well-formed HTTP response), and the bucket-policy evaluator must populate `aws:SecureTransport` from the request scheme so the Deny condition is evaluated.
Traces: s3.secure-transport-policy-plain-http-denied (ConnectionClosedError on the pre-policy baseline); s3.s3-ssec-and-endpoint-enforcement-secure-transport-deny-policy-refuses-plaintext-http-and-allows-https (BadStatusLine).

===== Cause 6 of 8: a bucket policy's explicit Deny is not evaluated on GetObject =====
AWS: an explicit `Deny` in a resource-based (bucket) policy outranks any identity Allow, including the bucket owner's. With a bucket policy denying `s3:GetObject` on `arn:aws:s3:::<bucket>/<key>`, GetObject of that key is refused HTTP 403 `AccessDenied` with the message `User: <caller ARN> is not authorized to perform: s3:GetObject on resource: "<key ARN>" with an explicit deny in a resource-based policy`. The fixed text is "User: ", " is not authorized to perform: ", " on resource: ", " with an explicit deny in a resource-based policy"; the variable parts are the caller's ARN (from the signing identity, read via sts:GetCallerIdentity), the action name, and the quoted resource ARN built from the request's bucket and key. Any other key in the same bucket still reads 200, and a key that does not exist still answers `NoSuchKey` / 404 / "The specified key does not exist." Recorded behavior only (policy evaluation is not expressed in the Smithy model).
Emulator: GetObject of the denied key keeps returning 200 with the object bytes. The check polls for the refusal and runs out the 60 s harness cap ("Check timed out after 60.0s"), so the Deny is never applied at any point.
Why: resource-based Deny is the primary S3 guardrail (data-perimeter and confused-deputy controls). An emulator that serves denied objects gives a false pass to exactly the policies a team most needs to verify. A related open issue, 20261027T000203Z-iam-s3-s3-getobject-performs-no-iam-authorization-a-principal.md, records the no-Allow case under the `iam__s3` service; it does not carry this check and does not cover the explicit-Deny path or its message.
Fix: the s3 crate's GetObject handler must run bucket-policy evaluation (explicit Deny first) before serving bytes, and the authorization-refusal serializer must produce AWS's explicit-deny message template.
Traces: s3.s3-csv-explicit-deny-on-the-csv-key-denies-the-owner-but-not-other-keys.

===== Cause 7 of 8: identity-policy condition keys (s3:prefix) are not evaluated on ListBucket/GetObject =====
AWS: an identity policy that allows `s3:ListBucket` only under `Condition: {"StringLike": {"s3:prefix": ["home/${aws:username}/*", ...]}}` refuses a ListObjectsV2 whose `Prefix` falls outside the permitted set with HTTP 403 `AccessDenied` — the condition key `s3:prefix` is taken from the request's `prefix` parameter, so listing another user's home prefix is denied while the caller's own prefix lists normally. Recorded behavior only (condition-key evaluation is not in the Smithy model).
Emulator: `ListObjectsV2(Prefix="home/some-other-user/")` under that policy succeeds with HTTP 200 and returns the other user's key (`Contents: [{Key: 'home/<other-user>/doc.txt', ...}], KeyCount: 1`) instead of being refused.
Why: prefix-scoped "home directory" policies are the canonical S3 multi-tenant isolation pattern. The emulator reports them as working while leaking every tenant's listing, so an isolation regression ships unnoticed.
Fix: the s3 crate's authorization layer must populate the request condition context (at minimum `s3:prefix`, `s3:delimiter`, `s3:max-keys`) from the ListBucket/ListObjectsV2 request and evaluate identity-policy conditions in the ListBucket and GetObject handlers.
Traces: s3.identity-policy-s3-prefix-condition-scopes-listbucket-and-getobject-to-a-home-prefix.

===== Cause 8 of 8: WriteGetObjectResponse accepts/mis-refuses a RequestRoute that was never issued =====
AWS: `WriteGetObjectResponse` is addressed through a RequestRoute-derived host — Smithy models `RequestRoute` as `httpHeader: x-amz-request-route` with `hostLabel: true` and `endpoint.hostPrefix: "{RequestRoute}."`. A RequestRoute that corresponds to no outstanding Object Lambda invocation therefore fails host validation before any token comparison, and S3 refuses it with HTTP 400, error code `InvalidHostHeader` and the message `Invalid Host header.` (entirely fixed text). A second, independently bogus route/token pair is refused identically.
Emulator: the call is refused with error code `'404'` — i.e. a bare HTTP status surfaced as the error code, with no `InvalidHostHeader` code and no `Invalid Host header.` message. The host-prefixed endpoint is simply not routed.
Why: an Object Lambda function's error handling keys off `InvalidHostHeader` to detect a stale/expired route-token; against the emulator it instead sees an un-modeled `404` pseudo-code it cannot classify or retry on.
Fix: the s3 crate's WriteGetObjectResponse handler (and the router's hostPrefix endpoint resolution) must accept the `{RequestRoute}.` host form, validate the route against outstanding Object Lambda invocations, and refuse an unknown route with 400 / `InvalidHostHeader` / "Invalid Host header.".
Traces: s3.write-get-object-response-invalid-request-token-rejected.

## Evidence
Cause 1 — expected: HTTP 201 for a conforming POST Object form carrying success_action_status=201. Actual: HTTP 404 with body b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'.

Cause 2 — expected: PutObject 200 with ETag = MD5 of the body when the SDK uses addressing_style=virtual (Host: <bucket>.<endpoint>, path /vhost/object.bin). Actual: ClientError (SignatureDoesNotMatch) when calling the PutObject operation: "The request signature we calculated does not match the signature you provided. Check your key and signing method."

Cause 3 — expected: HTTP 403, Code=RequestTimeTooSkewed, Message="The difference between the request time and the current time is too large.", RequestTime echoing the signed X-Amz-Date (YYYYMMDDTHHMMSSZ), ServerTime earlier than it. Actual: HTTP 200, body b'<PutObjectOutput xmlns="http://s3.amazonaws.com/doc/2006-03-01/"></PutObjectOutput>' — "a PUT signed 1500 s ahead of S3's clock must be refused 403, got 200"; and "the home-region skewed PUT must be refused 403, got 200".

Cause 4 — expected: HTTP 400, Code=AuthorizationHeaderMalformed, Region=<bucket region>, Message="The authorization header is malformed; the region '<signed region>' is wrong; expecting '<bucket region>'". Actual: HTTP 200 b'<PutObjectOutput .../>' — "a PUT scoped to 'us-west-2' but sent to 'ap-northeast-1' must be refused 400, got 200".

Cause 5 — expected: plain-HTTP GetObject returns the object bytes before any policy; after the aws:SecureTransport=false Deny, plain-HTTP Get/Put/List are 403 AccessDenied with the explicit-deny message while HTTPS still works. Actual: the pre-policy plain-HTTP GetObject raises botocore.exceptions.ConnectionClosedError: "Connection was closed before we received a valid response from endpoint URL: \"http://127.0.0.1:<port>/<bucket>/a.txt\""; the raw-HTTP variant raises http.client.BadStatusLine:  2.

Cause 6 — expected: 403 AccessDenied, Message='User: <caller ARN> is not authorized to perform: s3:GetObject on resource: "arn:aws:s3:::<bucket>/sample.csv" with an explicit deny in a resource-based policy'; other keys still 200; a missing key still NoSuchKey/404/"The specified key does not exist.". Actual: the denied key keeps returning 200 with its bytes; the refusal poll never observes AccessDenied and the check reports "Check timed out after 60.0s".

Cause 7 — expected: ListObjectsV2 with a Prefix outside the s3:prefix condition is refused AccessDenied. Actual: HTTPStatusCode 200 with Contents=[{'Key': 'home/some-other-user/doc.txt', ...}], KeyCount 1, Prefix 'home/some-other-user/' — "expected the call to be rejected, but it succeeded".

Cause 8 — expected: HTTP 400, Code=InvalidHostHeader, Message="Invalid Host header." for a RequestRoute/RequestToken pair matching no outstanding invocation. Actual: Error.Code = '404' — "unexpected error code for a bogus route/token: '404'".

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check ids in trace_ids.

Cause 1: build a presigned POST form for a bucket (fields key/policy/x-amz-credential/x-amz-algorithm/x-amz-date/x-amz-signature/success_action_status=201) and POST multipart/form-data to the bucket root over plain HTTP(S); observe 404 UnknownService instead of 201.
Cause 2: `boto3.client('s3', config=Config(s3={'addressing_style':'virtual'}))` against the emulator with the bucket as the leading Host label and a bucket-less path; call put_object — observe SignatureDoesNotMatch.
Cause 3: hand-sign a PutObject with X-Amz-Date set 1500 s after the server's clock and send it; observe 200 instead of 403 RequestTimeTooSkewed.
Cause 4: create a bucket in region B, then hand-sign a PutObject for it with a credential scope naming region A; observe 200 instead of 400 AuthorizationHeaderMalformed.
Cause 5: derive an `http://` endpoint from the emulator's announced endpoint and call get_object; observe ConnectionClosedError/BadStatusLine rather than a 200 or an S3 error. Then add the aws:SecureTransport=false Deny bucket policy and retry.
Cause 6: put an object, then put a bucket policy with Effect=Deny, Principal="*", Action=s3:GetObject, Resource=arn:aws:s3:::<bucket>/<key>; call get_object on that key repeatedly — it keeps returning 200.
Cause 7: attach an identity policy allowing s3:ListBucket only under StringLike s3:prefix for the caller's own home prefix; call list_objects_v2 with Prefix='home/<another-user>/' — it returns that user's keys.
Cause 8: call write_get_object_response with a RequestRoute/RequestToken pair that was never issued; inspect Error.Code — it is '404' instead of InvalidHostHeader.
