Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.presigned-get s3.presigned-post-expired-policy-rejected-and-writes-nothing s3.sigv4skew-presigned-get-refusal-names-its-expiry-window s3.sigv4skew-presigned-put-after-expiry-writes-no-object-rejected s3.put-object-encryption-type-mismatch s3.sse-c-wrong-customer-key-rejected s3.put-object-invalid-request-rejected s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.presigned-get|run#7|AssertionError|raise AssertionError(
Issue-Key: s3.presigned-post-expired-policy-rejected-and-writes-nothing|run#8|AssertionError|assert status == 201, (
Issue-Key: s3.sigv4skew-presigned-get-refusal-names-its-expiry-window|run#25|KeyError|assert err["X-Amz-Expires"] == str(expires_in), (
Issue-Key: s3.sigv4skew-presigned-put-after-expiry-writes-no-object-rejected|run#26|KeyError|assert err["Expires"] == expires_at.strftime("%Y-%m-%dT%H:%M:%SZ"), (
Issue-Key: s3.put-object-encryption-type-mismatch|run#11|AssertionError|assert (code, message) == ("AccessDenied", (
Issue-Key: s3.sse-c-wrong-customer-key-rejected|run#15|AssertionError|assert code == "AccessDenied", f"expected AccessDenied for a blocked SSE-C PutObject, got {code!r}"
Issue-Key: s3.put-object-invalid-request-rejected|run#8|AssertionError|assert code == ("InvalidRequest", "Bucket is missing ObjectLockConfiguration"), f"expected InvalidRequest, got {code!r}"
Issue-Key: s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected|run#9|AssertionError|assert encoded == [f"{prefix}a%0Ab-%F0%9F%98%80.txt"], (
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# S3 browser/edge surface: presigned GET refused 403, POST Object answers 501, expiry refusal omits its window, SSE-C and object-lock headers NotImplemented, ListObjects URL encoding wrong

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Six net-new root causes on the presigned/anonymous edge and the header-variant paths; a fixer must close all six.

Cause 1 of 6: a valid presigned GET is refused 403.
AWS: a presigned GET URL inside its expiry window serves the object with HTTP 200 and the exact bytes.
Emulator: the URL answers HTTP 403 Forbidden and serves nothing.
Why: every browser-facing download link -- the single commonest S3 integration -- fails, so no client can hand out a time-limited read URL. (Related to the open 20261027T190000Z-s3-legacy-query-credential-presigned-get.md, which does not carry this replay key.)
Fix: the query-string SigV4 verification on the object GET path, which must honour a correctly signed presigned URL.
Traces: s3.presigned-get

Cause 2 of 6: POST Object (browser form upload) is not routed at all.
AWS: a POST to /<bucket>/ (or /<bucket>) whose multipart form satisfies every condition of its signed policy is accepted; with success_action_status=201 the answer is HTTP 201 with a PostResponse body, and a form whose policy has expired is refused with the policy error, having written no object.
Emulator: answers HTTP 501 with <Error><Code>NotImplemented</Code><Message>operation POST /<bucket>/ is not implemented in this emulator</Message>.
Why: browser direct-to-S3 uploads cannot work at all, and the expired-policy refusal -- the security-relevant half -- can never be observed.
Fix: the POST Object route and its policy evaluation in the S3 bucket-level request dispatcher.
Traces: s3.presigned-post-expired-policy-rejected-and-writes-nothing

Cause 3 of 6: the 'Request has expired' refusal omits the window it is refusing on.
AWS: an expired presigned request is refused 403 with an AccessDenied error document whose body carries Code 'AccessDenied', Message 'Request has expired', and the elements X-Amz-Expires (the signed window in seconds, taken from the request), Expires (X-Amz-Date + X-Amz-Expires, formatted %Y-%m-%dT%H:%M:%SZ) and ServerTime (at or after Expires).
Emulator: the error XML has no X-Amz-Expires and no Expires element (the check raises KeyError on each).
Why: the caller cannot see which window expired or when, so it cannot distinguish an expired URL from any other AccessDenied and cannot correct its clock or its ExpiresIn.
Fix: the expired-presigned-request error serializer, which must emit X-Amz-Expires, Expires and ServerTime alongside the AccessDenied code and message.
Traces: s3.sigv4skew-presigned-get-refusal-names-its-expiry-window, s3.sigv4skew-presigned-put-after-expiry-writes-no-object-rejected

Cause 4 of 6: SSE-C header variants answer NotImplemented, so SSE-C authorization can never be refused.
AWS: PutObject and CreateMultipartOpload carrying x-amz-server-side-encryption-customer-algorithm/-key/-key-MD5 are served; a request blocked by policy is refused AccessDenied naming the SSE-C condition, and a GET with the wrong customer key is refused AccessDenied. (Smithy: SSECustomerAlgorithm / SSECustomerKey / SSECustomerKeyMD5 are modeled httpHeader members on these operations.)
Emulator: answers NotImplemented -- 'operation CreateMultipartUpload with SSECustomerAlgorithm is scaffolded but not implemented in this emulator' and 'operation PutObject with x-amz-server-side-encryption-customer-algorithm is scaffolded but not implemented in this emulator'.
Why: a client cannot use customer-provided keys at all, and the two refusals that matter (policy-blocked SSE-C, wrong customer key) are indistinguishable from an unimplemented service. (The open 20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md covers other SSE-C checks but not these replay keys.)
Fix: SSE-C support in the PutObject and CreateMultipartUpload handlers and their authorization path.
Traces: s3.put-object-encryption-type-mismatch, s3.sse-c-wrong-customer-key-rejected

Cause 5 of 6: PutObject with x-amz-object-lock-mode answers NotImplemented instead of validating it.
AWS: a PutObject carrying an object-lock mode without the retain-until date (or on a bucket without object lock enabled) is refused InvalidRequest.
Emulator: 'operation PutObject with x-amz-object-lock-mode is scaffolded but not implemented in this emulator'.
Why: the caller is told the service lacks the feature when the request itself is invalid, so it cannot correct the request.
Fix: object-lock header handling and validation in the PutObject handler.
Traces: s3.put-object-invalid-request-rejected

Cause 6 of 6: ListObjects EncodingType handling is wrong, and an unsupported EncodingType is not refused.
AWS: with EncodingType=url, keys are form-encoded in the listing (a space becomes '+', control characters become %XX such as CR -> %0D), and an EncodingType value other than 'url' is refused.
Emulator: encodes differently -- a key with a newline and an emoji comes back as 'encneg%2Fa%0Ab-%F0%9F%98%80.txt' rather than AWS's encoding -- and the unsupported EncodingType is not refused.
Why: a client that decodes the listing with AWS's rules recovers the wrong key bytes, so listings and subsequent per-key operations disagree. (Companion to the open 20261025T032458Z-s3-remaining-divergences-8-root-causes-selectobjectcontent-routing.md, which does not carry this replay key.)
Fix: the ListObjects/ListObjectsV2 key encoder and its EncodingType validation.
Traces: s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected

## Evidence
s3.presigned-get -> AssertionError: DATAPLANE_UNIMPLEMENTED: presigned GET URL did not serve the object: HTTPError: HTTP Error 403: Forbidden
s3.presigned-post-expired-policy-rejected-and-writes-nothing -> AssertionError: the conforming POST was answered 501 with b'<?xml version="1.0"?><Error><Code>NotImplemented</Code><Message>operation POST /sim2real-behav-bkt-8f42b82b4144 is not implemented in this emulator</Message>...'; expected 201
s3.sigv4skew-presigned-get-refusal-names-its-expiry-window -> KeyError: 'X-Amz-Expires' at `assert err["X-Amz-Expires"] == str(expires_in)`, where err is the parsed error XML of the 403 body
s3.sigv4skew-presigned-put-after-expiry-writes-no-object-rejected -> KeyError: 'Expires' at `assert err["Expires"] == expires_at.strftime("%Y-%m-%dT%H:%M:%SZ")`
s3.put-object-encryption-type-mismatch -> AssertionError: AccessDenied on CreateMultipartUpload must name the SSE-C block specifically ...; got 'NotImplemented' ('operation CreateMultipartUpload with SSECustomerAlgorithm is scaffolded but not implemented in this emulator')
s3.sse-c-wrong-customer-key-rejected -> AssertionError: expected AccessDenied for a blocked SSE-C PutObject, got 'NotImplemented'
s3.put-object-invalid-request-rejected -> AssertionError: expected InvalidRequest, got ('NotImplemented', 'operation PutObject with x-amz-object-lock-mode is scaffolded but not implemented in this emulator')
s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected -> AssertionError: EncodingType=url did not encode the key as expected: got ['encneg%2Fa%0Ab-%F0%9F%98%80.txt']

## Steps to Reproduce
Replay the behavioral suite and run the s3 checks in trace_ids. Generate a presigned GET with ExpiresIn and fetch it inside its window (expect 200 and the exact bytes), then after expiry (expect 403 AccessDenied 'Request has expired' whose XML carries X-Amz-Expires, Expires and ServerTime). POST a multipart form satisfying a signed policy with success_action_status=201 to /<bucket>/ (expect 201), and one with an expired policy (expect the policy refusal and no object). Call PutObject and CreateMultipartUpload with x-amz-server-side-encryption-customer-algorithm/-key/-key-MD5, PutObject with x-amz-object-lock-mode, and ListObjectsV2 with EncodingType=url over keys containing a space, a CR and non-ASCII characters, plus an unsupported EncodingType value.
