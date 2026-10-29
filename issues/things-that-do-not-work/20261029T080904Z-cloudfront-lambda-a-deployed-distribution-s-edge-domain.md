Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|_poll_until#34|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 8acf21a252327a123d13b159a352e2f9f2a954c2

# cloudfront__lambda: a deployed distribution's edge domain answers nothing, so the signed-URL source-IP / key-revocation enforcement is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `8acf21a252327a123d13b159a352e2f9f2a954c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue carries ONE root cause. Cause 1 of 1 must be closed before submitting.

Cause 1 of 1: a CloudFront distribution over a Lambda function-URL origin never serves a response at its edge domain — not even on the unsigned behavior the check uses as its positive control — so no signed-URL policy evaluation and no trusted-key-group revocation can be observed. Failing check id: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied.

AWS: recorded behavior only (the viewer-facing edge is outside the Smithy control-plane model; control-plane side is CreateDistribution -> POST /2020-05-31/distribution, 201, which echoes TrustedKeyGroups and the behaviors back verbatim whether or not anything enforces them). What the recording requires: once GetDistribution reports Status `Deployed`, a GET on an unsigned behavior of the distribution's DomainName returns HTTP 200 with the origin's body and the edge adds a CloudFront-Viewer-Address header to the origin request; a URL signed with a key in the behavior's TrustedKeyGroups over a custom policy whose Condition.IpAddress.AWS:SourceIp covers that observed viewer address is served 200; the same resource signed with a policy allowing only a range that excludes it is refused HTTP 403 with error Code `AccessDenied` and no origin bytes; after UpdateKeyGroup removes a public key, URLs signed with it settle at HTTP 403 with error Code `InvalidKey` while the remaining key still returns 200. Those statuses and the Code values AccessDenied / InvalidKey are the requirement; the generated domain name, key ids and the viewer address are per-run values.

Emulator: the distribution's edge domain produces no response at all. The check's first edge request — the unsigned `GET {edge}/whoami` positive control — never reaches 200 and the check's own poller gives up: `TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded` (raised from `_poll_until` at check line 75, called from run line 359). Because the viewer address is discovered from that response, every signed leg (admission, AccessDenied, InvalidKey) is unreachable, and no status or error Code exists to compare.

Why: this is the authorization boundary for private content. An edge that answers nothing is indistinguishable from one that admits every signature: a client cannot confirm that an IP condition is honoured, nor — the operationally critical part — that removing a public key from a key group actually REVOKES the URLs signed with it. The control plane happily echoes TrustedKeyGroups back, so a client can 'verify' a protection that does not exist.

Fix: the cloudfront crate's edge/data-plane request path — the distribution's DomainName must answer HTTPS requests, select the matching cache behavior by path pattern, forward to the origin (adding CloudFront-Viewer-Address to the origin request), and on behaviors with TrustedKeyGroups verify Policy/Signature/Key-Pair-Id against the keys currently in the group (re-read per request so a removed key stops admitting), evaluate the custom policy's Resource/DateLessThan/IpAddress conditions, and emit 403 with Code AccessDenied for a failed condition and Code InvalidKey for an unknown/revoked key.

Note: the open issue 20261027T001538Z-cloudfront-lambda-the-edge-enforces-nothing-because-it.md bundles this cause as its Cause 2 of 2 and lists this check id, but under a stale replay key (`_poll_until#34|Timeout|time.sleep(interval_s)`); in this replay the check fails earlier and with a different frame (`_poll_until#75`, TimeoutError on the unsigned /whoami 200), so the suite did not mark it owned. 20261025T220525Z-cloudfront-lambda-the-oac-signed-lambda-function-url.md records the same silent edge for the OAC check only.

Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied.

## Evidence
Expected (AWS): after GetDistribution reports Status 'Deployed', GET https://<DomainName>/whoami -> HTTP 200 with the origin marker and a cloudfront-viewer-address header value; a URL signed with key A over a /32 policy for that address -> HTTP 200; the same resource under a policy for an excluded range -> HTTP 403 with error Code AccessDenied and no origin bytes; after UpdateKeyGroup removes key B, B's signed URL -> HTTP 403 Code InvalidKey while A's still returns 200.

Actual (emulator): cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied [error]
  File "check_cloudfront__lambda_deep_cloudfront_2_cf_edge_enforce_signed_url_source_ip_denied.py", line 359, in run
    who = _poll_until(lambda: _get(session, f"{edge}/whoami"), ...)
  File "...", line 75, in _poll_until
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded

So not even the unsigned positive control is served: the edge domain returns no 200 within the run budget, and the CloudFront-Viewer-Address header the negative legs are built from is never produced. Distribution domain names, key ids and viewer addresses are per-run values and not requirements; the 200 on the unsigned behavior and the 403 + Code AccessDenied / Code InvalidKey refusals are.

## Steps to Reproduce
1. Boot the emulator and replay cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied from simparity-behavioral-ga-prod-v1.
2. Create a Lambda with a function URL that echoes its marker, rawPath, the cloudfront-viewer-address header and the header names; front it with a CloudFront OAC.
3. CreatePublicKey (A) and CreateKeyGroup([A]); CreateDistribution over that origin with TrustedKeyGroups=[the group], IsIPV6Enabled=false and an unsigned /whoami* behavior; poll GetDistribution until Status == Deployed.
4. GET https://<DomainName>/whoami and expect HTTP 200 plus the echoed viewer address.
5. Observe: the request never returns 200 within the run budget; nothing is served through <DomainName>, so the signed-URL admission, the AccessDenied refusal for an excluded source-IP policy and the InvalidKey refusal after UpdateKeyGroup are all unreachable.
