Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.sigv4skew-cross-region-credential-scope-rejected s3.sigv4skew-future-dated-put-outside-window-rejected s3.sigv4skew-window-enforced-in-derived-second-region-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.sigv4skew-cross-region-credential-scope-rejected|_http#10|URLError|with urllib.request.urlopen(req, context=_ssl_context(verify)) as resp:
Issue-Key: s3.sigv4skew-future-dated-put-outside-window-rejected|_http#8|URLError|with urllib.request.urlopen(req) as resp:
Issue-Key: s3.sigv4skew-window-enforced-in-derived-second-region-rejected|_http#7|URLError|with urllib.request.urlopen(req) as resp:
Issue-Commit: d24b6997be17cc686e45f4a7148f10449cf3bf59

# s3: the HTTPS front door aborts the handshake for a virtual-hosted-style bucket authority (TLS illegal_parameter), so the three SigV4 credential-scope / clock-skew refusals are never served

> _Note: this issue was identified against the code base at commit `d24b6997be17cc686e45f4a7148f10449cf3bf59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
One net-new root cause in the S3 request-admission surface (transport reachability of virtual-hosted-style addressing). Three failing checks, one cause.

Cause 1 of 1: a raw HTTPS request whose authority is the virtual-hosted form `<bucket>.<announced-endpoint-authority>` never completes a TLS handshake with the emulator, so the SigV4 refusals these checks exist to verify are never produced.

AWS: S3 serves the same operations under virtual-hosted-style addressing, where the bucket is the leading DNS label of the authority and the request path carries no bucket segment. Smithy: AmazonS3, version 2006-03-01, protocol restXml (noErrorWrapping), sigv4Name `s3`, awsService.endpointPrefix `s3`, arnNamespace `s3`, authSchemes [sigv4], endpointResolution.endpointRuleSet = true with authSchemes [sigv4, sigv4-s3express, sigv4a] — the modeled endpoint rule set is exactly what rewrites the `Bucket` input member into the endpoint authority, i.e. the virtual-hosted form is a modeled, first-class request shape, not an SDK nicety. Over that transport AWS then applies SigV4 admission: a correctly-timed PUT whose credential scope names a region other than the bucket's region is refused with HTTP 400 and error code `AuthorizationHeaderMalformed`, whose body carries a `<Region>` element naming the region S3 expected and the message `The authorization header is malformed; the region '<signed-region>' is wrong; expecting '<bucket-region>'` (the quoted sentence is the fixed text; both region names are values taken from the request's credential scope and from the addressed bucket's location, never literals from one run); a PUT signed outside S3's signing window is refused with the clock-skew refusal, and in both cases nothing is written, so a following GetObject answers `NoSuchKey` / `The specified key does not exist.`. `AuthorizationHeaderMalformed` is not a modeled error shape in the Smithy model (S3 carries it as a wire-level admission error), so for the code/message/status themselves this is recorded behavior only, as asserted by the three checks.

Emulator: none of the three checks gets as far as an HTTP response. Each dies while opening the connection, with the identical failure: `urllib.error.URLError: <urlopen error [SSL: SSLV3_ALERT_ILLEGAL_PARAMETER] ssl/tls alert illegal parameter (_ssl.c:1010)>`, raised out of `urllib/request.py https_open -> do_open`. The target is not an AWS address and is not hardcoded: the checks build it from the emulator's own announced endpoint — `endpoint = urlsplit(client.meta.endpoint_url); host = f"{bucket}.{endpoint.netloc}"; url = f"{endpoint.scheme}://{host}{path}"` — i.e. the emulator's announced scheme and authority with the bucket prepended as a DNS label, verified with the harness' own `ctx.http_verify`. So the emulator's HTTPS front door terminates the handshake with an `illegal_parameter` alert as soon as the authority is the bucket-prefixed (virtual-hosted) form, and no status, error code or error message can be compared at all. Hypothesis: the front door's TLS termination presents/accepts only the single authority it announces and rejects the SNI value `<bucket>.<authority>`; the announced authority being an IP literal (recorded in the related open issue below) would make every `<bucket>.`-prefixed SNI unknown to it. The requirement here is not a particular host, port or certificate — those are per-environment values — but that the announced endpoint be reachable in the `<bucket>.<authority>` form over the same scheme it announces, and that the request then be admitted and SigV4-verified with the bucket resolved from the leading Host label.

Why: a client relying on AWS's behavior gets no S3 admission behavior at all on this transport. Virtual-hosted style is the default form an AWS SDK uses against real S3, and every tool that signs its own requests (presigners, CDN/OAC origins, proxies, SDKs in other languages, curl-based scripts) sends exactly the request these checks send. Against the emulator such a client sees a TLS handshake failure instead of a service answer, which it cannot distinguish from a network outage or a certificate problem on its own side; it will retry rather than correct its request. Worse, the entire SigV4 admission surface behind that transport is unverifiable: cross-region credential scope (`AuthorizationHeaderMalformed` plus the expected-region hint a client uses to re-sign) and the signing-window refusal are never exercised, so a request real AWS rejects as mis-signed may be silently accepted by the emulator on the path-style transport and nobody finds out.

Fix: the s3 crate's listener / front door — its TLS termination and SNI handling, and the host-based request routing that must resolve a bucket from the leading label of the `Host` header when the path carries no bucket segment, canonicalizing SigV4 over the Host header exactly as signed. This is the same component as the related open issue, one layer earlier (that issue stops at the SDK silently degrading to path style; these three checks hand-build the virtual-hosted request and are refused by TLS before any HTTP is spoken).

Related open issue, named rather than assumed to cover this: 20261029T083133Z-s3-virtual-hosted-style-bucket-addressing-is-unreachable.md. It carries only `s3.sdk-virtual-hosted-requests-round-trip-an-object-and-its-deletion` in its Issue-Trace-Ids and describes a SILENT degradation (botocore keeps path style because the announced authority is an IP literal, every call succeeding). It does not carry these three replay keys and does not describe a TLS handshake abort, so a fixer who makes the SDK form virtual-hosted requests without making the `<bucket>.<authority>` form terminable over TLS will leave all three of these checks failing.

Traces: s3.sigv4skew-cross-region-credential-scope-rejected, s3.sigv4skew-future-dated-put-outside-window-rejected, s3.sigv4skew-window-enforced-in-derived-second-region-rejected.

## Evidence
Cause 1 — expected (real AWS):
- An HTTPS PUT to `https://<bucket>.<endpoint-authority>/<key>`, SigV4-signed with a credential scope naming a region other than the bucket's region, returns HTTP 400 with an XML error whose `Code` is `AuthorizationHeaderMalformed`, whose `Region` element names the bucket's region, and whose `Message` is `The authorization header is malformed; the region '<signed-region>' is wrong; expecting '<bucket-region>'` (fixed sentence; both region names come from the request's credential scope and the addressed bucket's location). A following GetObject of that key answers `NoSuchKey` / `The specified key does not exist.`, proving nothing was written. The same request re-signed for the correct region returns HTTP 200 and the signed GET returns the exact bytes.
- A PUT signed outside the signing window is likewise refused and writes nothing; a derived second region enforces the same window.

Actual (emulator), identical for all three checks and raised before any request is delivered:
  File "request.py", line 1405, in https_open
    return self.do_open(http.client.HTTPSConnection, req,
  File "request.py", line 1360, in do_open
    raise URLError(err)
  urllib.error.URLError: <urlopen error [SSL: SSLV3_ALERT_ILLEGAL_PARAMETER] ssl/tls alert illegal parameter (_ssl.c:1010)>

Status / error code / error message divergence: there is no response to compare. AWS answers 400 `AuthorizationHeaderMalformed` with the message above (and the skew refusal for the window cases); the emulator answers nothing — the TLS handshake for the `<bucket>.<announced-authority>` form is aborted with an `illegal_parameter` alert, so the request is never admitted.

The authority is derived from the emulator itself, not from AWS: `_target()` in each check does `endpoint = urlsplit(client.meta.endpoint_url); host = f"{bucket}.{endpoint.netloc}"` and reuses `endpoint.scheme` and `ctx.http_verify`. The specific host, port and bucket name are per-run values and are not part of the requirement; the requirement is that the bucket-prefixed form of the announced authority be reachable over the announced scheme.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check ids in trace_ids: s3.sigv4skew-cross-region-credential-scope-rejected, s3.sigv4skew-future-dated-put-outside-window-rejected, s3.sigv4skew-window-enforced-in-derived-second-region-rejected.

Manually:
1. Create a bucket through the SDK (adding CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
2. Read the endpoint the emulator announces: `endpoint = urlsplit(s3.meta.endpoint_url)`.
3. Build the virtual-hosted target by hand: `host = f"{bucket}.{endpoint.netloc}"`, `url = f"{endpoint.scheme}://{host}/sigv4skew/scoped.txt"`.
4. Hand-sign a PUT for that `host` with SigV4, using a credential scope `<datestamp>/<other-region>/s3/aws4_request` (a region other than the bucket's), and send it with urllib/requests using the harness' TLS verification setting.
5. Observe: the call never returns an HTTP status — the TLS handshake aborts with `[SSL: SSLV3_ALERT_ILLEGAL_PARAMETER] ssl/tls alert illegal parameter`. Expected instead: HTTP 400, `Code=AuthorizationHeaderMalformed`, `Region=<bucket region>`, `Message=The authorization header is malformed; the region '<signed region>' is wrong; expecting '<bucket region>'`.
6. Control: send the identical request to the path-style authority (`{endpoint.scheme}://{endpoint.netloc}/{bucket}/sigv4skew/scoped.txt`, signed over that Host). The handshake completes, confirming the abort is specific to the bucket-prefixed authority.
7. Repeat steps 4-5 with a correctly-scoped signature timestamped outside the signing window to see the clock-skew refusal is equally unreachable.
