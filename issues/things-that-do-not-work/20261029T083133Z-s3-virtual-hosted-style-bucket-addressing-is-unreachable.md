Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion|run#33|AssertionError|assert not path_style, f"requests did not name the bucket in the Host: {path_style!r}"
Issue-Commit: de2ed5591f516f72432ff713b85ef00b6924721d

# s3: virtual-hosted-style bucket addressing is unreachable — the announced endpoint is an IP literal, so an SDK configured addressing_style=virtual silently degrades to path style and the Host-label bucket resolution path is never served

> _Note: this issue was identified against the code base at commit `de2ed5591f516f72432ff713b85ef00b6924721d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
One net-new root cause in the S3 request-admission surface (addressing).

Related open issue: 20261028T105230Z-s3-8-net-new-root-causes-in-the.md carries this same check id in its Issue-Trace-Ids under its "Cause 2 of 8: virtual-hosted-style addressing fails SigV4 verification". That cause does NOT cover what the check now observes and the suite no longer binds the check to it: its replay key was `s3|PutObject|SignatureDoesNotMatch`, and the emulator no longer refuses the request at all. The divergence has MOVED one layer out — from "the signature over a virtual-hosted request is miscomputed" to "a virtual-hosted request can never be formed against the endpoint the emulator announces". A fixer who closes that issue's Cause 2 by fixing SigV4 canonicalization alone will still leave this check failing, so it is filed as its own cause.

Cause 1 of 1: the endpoint the emulator announces is an IP literal, so virtual-hosted (DNS-style) bucket addressing cannot be exercised and the bucket is never carried in the Host header.

AWS: S3 supports two addressing forms for the same operations. In virtual-hosted style the bucket name is the leading DNS label of the Host header and the URI path omits the bucket entirely (`Host: <bucket>.s3.<region>.amazonaws.com`, request line `PUT /vhost/object.bin`); in path style the bucket is the first path segment. Smithy: AmazonS3 (version 2006-03-01, protocol restXml, sigv4Name `s3`, endpointPrefix `s3`) declares `endpointResolution.endpointRuleSet: true` — the modeled endpoint rule set is precisely what rewrites the `Bucket` input member into the endpoint authority, and GetObject/PutObject model `Bucket` as `@httpLabel @required` only for the path-style form (`http: {method: GET, uri: "/{Bucket}/{Key+}?x-id=GetObject"}`). Virtual-hosted style is the DEFAULT an AWS SDK uses against real S3; path style is the fallback. Because it is a DNS form, it requires the endpoint authority to be a resolvable host name: botocore refuses to rewrite to virtual-hosted style when the configured endpoint authority is an IP literal and keeps path style instead. Recorded behavior only for the degradation rule itself (it is an SDK/endpoint-rules property, not a wire shape in the model); the wire requirement from the model is that S3 must serve a request whose bucket arrives as the leading Host label and whose path omits the bucket.

Emulator: the check builds a second client with `Config(s3={"addressing_style": "virtual"})` and records the Host of every request it sends. All eight requests of the round trip (PutObject, HeadObject, GetObject, ListObjectsV2, DeleteObject, the post-delete GetObject, the post-delete ListObjectsV2 and PutObjectTagging) are sent with the bare IP-literal authority of the emulator's announced endpoint as the Host — none of them names the bucket as a Host label, and the assertion `assert not path_style` fails with the recorded host list. Every one of the eight calls itself SUCCEEDS on the wire (the put ETag matches the body MD5, the read is byte-exact, the deleted key reads NoSuchKey / "The specified key does not exist."), so the data plane is not the defect: the defect is that the endpoint the emulator announces admits only path style, so the virtual-hosted code path is never reached and the emulator's own Host-label bucket resolution is never exercised. Hypothesis: the listener/front door advertises its bind address (an IP) as the endpoint authority rather than a DNS name for which `<bucket>.<authority>` resolves to the same listener; that would also explain why the previously recorded `SignatureDoesNotMatch` for this check no longer occurs — no virtual-hosted request is produced for the verifier to reject.

The requirement here is NOT a particular host name or port (those differ per environment). It is that (a) the announced endpoint authority be a DNS name under which `<bucket>.<authority>` reaches the same listener, and (b) the S3 front door resolve the bucket from the leading Host label when the path carries no bucket segment, canonicalizing SigV4 over the `Host` header exactly as signed.

Why: a client relying on AWS's behavior gets two wrong answers. First, virtual-hosted addressing is the AWS default for real S3, so an application, a tool or a test suite that pins `addressing_style=virtual` (or simply does not override it against a real endpoint) exercises a transport shape against the emulator that it will never use in production, and the emulator reports a clean pass; a bucket-from-Host regression in the emulator — or in the application's own Host handling, SigV4 signing, bucket-name DNS-compatibility validation, or proxy/CDN origin configuration — cannot be caught here. Second, the degradation is SILENT: no error, no warning, a fully green round trip. A user debugging why their virtual-hosted deployment fails in AWS but passes against the emulator has no signal at all, and anything that depends on the bucket being in the authority (OAC/OAI origins, SigV4 host canonicalization, DNS-compatible bucket naming rules) is untestable.

Fix: the s3 crate's listener/front door and endpoint announcement. The announced endpoint must use a DNS-resolvable authority for which the `<bucket>.` prefixed form resolves to the same listener, and the request router must strip a leading bucket label from the `Host` header (rather than requiring a bucket path segment) when resolving the bucket, with SigV4 canonicalization taken from the signed `Host` header and the received request-target. This is the same component named by Cause 2 of 20261028T105230Z-s3-8-net-new-root-causes-in-the.md, one layer earlier.

Traces: s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion.

## Evidence
Cause 1 — expected: with `boto3.client('s3', config=Config(s3={'addressing_style':'virtual'}))`, every request of the round trip names the bucket as the leading label of the Host header (`Host: <bucket>.<endpoint-authority>`) with a bucket-less request path, and the write/head/read/list/delete round trip succeeds; the check's final assertion `path_style = [h for h in hosts if not h.startswith(bucket + '.')]; assert not path_style` holds.

Actual: AssertionError: requests did not name the bucket in the Host: ['127.0.0.1:42123', '127.0.0.1:42123', '127.0.0.1:42123', '127.0.0.1:42123', '127.0.0.1:42123', '127.0.0.1:42123', '127.0.0.1:42123', '127.0.0.1:42123'] — all eight recorded Hosts are the bare endpoint authority (an IP literal; the specific address and port are per-run values, not part of the requirement), so not one request used virtual-hosted addressing. Raised at check_s3_sdk_virtual_hosted_requests_round_trip_an_object_and_its_deletion.py line 109 (`assert not path_style`).

All eight underlying calls returned 2xx and were individually correct: PutObject ETag equalled the MD5 of the 4096-byte body, HeadObject reported the written ContentLength and the same ETag, GetObject returned the exact bytes, ListObjectsV2(Prefix='vhost/') listed exactly 'vhost/object.bin', and after DeleteObject the GetObject and PutObjectTagging on the dead key were both refused 4xx with Code=NoSuchKey and Message="The specified key does not exist." — i.e. the object plane is fine and the divergence is confined to addressing.

Status/code/message divergence: none on any individual call. The divergence is the transport form: AWS serves `Host: <bucket>.<endpoint>` + bucket-less path; the emulator's announced endpoint admits only `Host: <endpoint>` + `/<bucket>/<key>`.

Context: the open issue 20261028T105230Z-s3-8-net-new-root-causes-in-the.md recorded this same check failing earlier with `(SignatureDoesNotMatch) ... The request signature we calculated does not match the signature you provided. Check your key and signing method.` on the first virtual-hosted PutObject. That refusal no longer occurs because no virtual-hosted request is ever emitted.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check id in trace_ids: s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion.

Manually:
1. Create a bucket (adding CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
2. Build a second S3 client pinned to virtual-hosted addressing: `boto3.client('s3', endpoint_url=<the endpoint the emulator announces>, config=Config(s3={'addressing_style': 'virtual'}, retries={'max_attempts': 1}))`.
3. Register a `before-send.s3` handler that records `request.headers['Host']` (falling back to the authority of `request.url`).
4. Through that client: put_object(Key='vhost/object.bin', Body=os.urandom(4096)), head_object, get_object, list_objects_v2(Prefix='vhost/'), delete_object, then get_object and put_object_tagging on the deleted key.
5. Inspect the recorded Hosts. Every entry is the bare endpoint authority (an IP literal) instead of '<bucket>.' + that authority, i.e. botocore kept path style because the endpoint authority is not a DNS name; no request ever carried the bucket in the Host.
6. Confirm the server side independently: send a hand-built request with `Host: <bucket>.<authority>` and a bucket-less path (e.g. `GET /vhost/object.bin`), SigV4-signed over that Host, and observe whether the bucket is resolved from the Host label.
