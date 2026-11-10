Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied cloudfront__s3.response-headers-policy-cors-get-granted-disallowed-method-refused cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published cloudfront__s3.unpublished-or-origin-event-function-association-rejected
Issue-Key: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected|run#13|AssertionError|assert body == _ROOT_PAGE, f"/ served {body[:200]!r}, expected the bucket-root index.html"
Issue-Key: cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object|_get_until#14|AssertionError|raise AssertionError(f"{description} not observed within {_EDGE_DEADLINE_S}s; last {last!r}")
Issue-Key: cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden|_edge_status#19|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc}") from exc
Issue-Key: cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden|_edge_status#17|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc}") from exc
Issue-Key: cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied|_get_until#10|AssertionError|raise AssertionError(f"{description} not observed within {_EDGE_DEADLINE_S}s; last {last!r}")
Issue-Key: cloudfront__s3.response-headers-policy-cors-get-granted-disallowed-method-refused|_poll#11|AssertionError|raise AssertionError(f"{description} not answered {want} within {timeout_s}s; last {detail!r}")
Issue-Key: cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key|run#13|AssertionError|assert st == 403, f"/onyx-test/ through the bare distribution returned HTTP {st!r}: {body[:300]!r}"
Issue-Key: cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published|_get_until#12|AssertionError|raise AssertionError(f"{description} not observed within {_EDGE_DEADLINE_S}s; last {last!r}")
Issue-Key: cloudfront__s3.unpublished-or-origin-event-function-association-rejected|run#19|AssertionError|assert message == want, f"the origin-request refusal said {message!r}, expected {want!r}"
Issue-Commit: 69cb8c4786d799a8a66c2117bf8eebb4fb69db3f

# cloudfront__s3: 6 net-new causes in the S3-origin edge — DefaultRootObject ignored (bucket listing exposed at /), OAC/OAI-granted origin fetches refused 403, CORS GET refused, missing-key 404 instead of 403, CloudFront Functions never run, and an enum-validation message that is not AWS's

> _Note: this issue was identified against the code base at commit `69cb8c4786d799a8a66c2117bf8eebb4fb69db3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront viewer GET at a distribution edge (S3 origin) / UpdateDistribution

## Description
Six distinct root causes, all in the cloudfront__s3 suite and all reached now that the edge is answering. None of them is covered by an open issue: the open cloudfront__s3 issues 20261109T181319Z-cloudfront-s3-the-s3-origin-edge-is-unreachable.md and 20261109T220008Z-cloudfront-s3-a-distribution-s-domainname-is-published.md describe an unserved DomainName, which is a different (earlier) failure mode — here the edge does answer, and answers wrongly.

Cause 1 of 6: a distribution's DefaultRootObject is not applied, and the edge exposes the bucket listing at /.
AWS: Smithy: DistributionConfig.DefaultRootObject — "When a viewer requests the root URL for your distribution, the default root object is the object that you want CloudFront to request from your origin... A default root object avoids exposing the contents of your distribution." A viewer GET of / on a distribution whose DefaultRootObject names an object must be answered 200 with that object's bytes, byte for byte, and must never relay a bucket listing to the viewer.
Emulator: GET / answers 200 whose body is S3's ListObjects document, '<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">...<Contents><Key>&lt;tenant&gt;/app.js</Key>...' — the viewer path "/" is forwarded to the REST origin as an empty key, so the origin answers with a listing instead of the configured root object.
Why: a client that relies on the default root object gets a directory listing of the private bucket instead of its index page; every single-page app served from a CloudFront+S3 origin is broken, and the bucket's whole key space leaks to any viewer.
Fix: the edge request handler that maps a viewer path onto an origin path — substitute DefaultRootObject when the viewer path is "/" before the origin request is built and signed.
Traces: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected.

Cause 2 of 6: an origin fetch the bucket policy/ACL grants to the distribution is still refused 403 AccessDenied at the edge.
AWS: recorded behavior only (the S3 and CloudFront models carry no authorization rule). With an origin access control attached and a bucket policy allowing s3:GetObject to Principal {"Service": "cloudfront.amazonaws.com"} under Condition StringEquals AWS:SourceArn = this distribution's ARN — or, for the legacy path, with the origin access identity's canonical user granted READ — the signed origin fetch succeeds and the edge answers 200 with the object's bytes and an x-cache of "Miss from cloudfront" then "Hit from cloudfront".
Emulator: every such fetch answers 403 with header x-cache: "Error from cloudfront" and body '<Error><Code>AccessDenied</Code><Message>Access Denied</Message><RequestId>...</RequestId><HostId>...</HostId></Error>' (RequestId/HostId are per-run values, not part of the requirement). The grant the test installed is never honoured, so both the positive controls and the negative controls that depend on them collapse.
Why: a client cannot serve any private object through CloudFront. Worse, because the refusal is identical for a granted and an ungranted bucket policy, every OAC/OAI authorization negative passes vacuously — a caller cannot distinguish a correctly locked origin from a broken one.
Fix: the edge's origin-request signing plus the S3 bucket-policy/ACL evaluation for the CloudFront service principal and the OAI canonical user.
Traces: cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object, cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden, cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden, cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied.

Cause 3 of 6: a simple GET carrying an Origin header is refused 403 at the edge instead of being served with the response-headers policy's CORS headers.
AWS: recorded behavior only — CloudFront never refuses a simple (non-preflight) GET on the basis of its Origin header. With a response-headers policy whose CorsConfig lists the request's origin in AccessControlAllowOrigins, the GET is answered 200 with the object body and Access-Control-Allow-Origin set to that origin; only the methods the policy does not allow are refused, and a preflight is answered on OPTIONS.
Emulator: the GET is answered 403 with the reason 'cross-site origin', so the granted-origin positive never reaches the assertion and the disallowed-method negative is never exercised.
Why: a browser client fetching a config/asset cross-origin through CloudFront is denied outright, and the emulator reports a refusal AWS does not model for any CORS outcome.
Fix: the edge's response-headers-policy/CORS handling — the Origin header must select the response headers, never gate the request.
Traces: cloudfront__s3.response-headers-policy-cors-get-granted-disallowed-method-refused.

Cause 4 of 6: S3 GetObject for a key that does not exist answers 404 NoSuchKey to a principal that was never granted s3:ListBucket.
AWS: the GetObject model carries only NoSuchKey (httpError 404) and InvalidObjectState, so the 403 path is documented rather than modelled: S3 returns 404 NoSuchKey for a missing key only when the requester holds s3:ListBucket on the bucket; without it, S3 answers 403 with code AccessDenied and the fixed message "Access Denied", so object existence is not revealed. The status and code flip to 404/NoSuchKey — with <Key> echoing the verbatim key from the request — once s3:ListBucket is granted.
Emulator: the refusal is 404 '<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key>onyx-test/</Key>...' while the policy grants s3:GetObject only, i.e. the ListBucket grant is not consulted at all.
Why: a client probing a private prefix learns which keys exist, which is exactly what the 403 contract prevents; and a caller diagnosing a CloudFront 403 against an OAC origin is handed a 404 instead.
Fix: the GetObject/HeadObject not-found path must consult the caller's s3:ListBucket authorization before choosing between 403 AccessDenied and 404 NoSuchKey.
Traces: cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key. (The open issue 20261108T031654Z-cloudfront-dynamodb-lambda-s3-stepfuncti-cloudfront-dynamodb-lambda-s3-stepfunctions-getobject-for-a.md names the same behaviour under a different suite heading and does not carry this check's replay key.)

Cause 5 of 6: a LIVE CloudFront Function associated at viewer-request is never executed at the edge.
AWS: Smithy: FunctionAssociation.EventType — "The event type of the function, either viewer-request or viewer-response." A published (LIVE) function associated at viewer-request runs on every viewer request before the cache lookup, so a function that rewrites event.request.uri from a directory-style path onto that path plus the index object makes the origin request carry the rewritten key and the edge answer 200 with that object; re-publishing an edited function changes what the edge serves.
Emulator: the association is accepted but the function body never runs — the origin is asked for the verbatim viewer path and the edge answers 404 '<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key>onyx-test/</Key>...' with x-cache "Hit from cloudfront".
Why: any routing, rewrite, redirect or header-injection logic a client puts in a CloudFront Function silently does nothing, and the "live code until an edit is published" contract (DEVELOPMENT vs LIVE) cannot be observed at all.
Fix: the edge request pipeline must invoke the associated function's LIVE code for viewer-request/viewer-response and apply the returned request/response mutation.
Traces: cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published.

Cause 6 of 6: the enum-constraint validation message reads "Member must satisfy enum value set:" where AWS writes "Member must satisfy value set:".
AWS: Smithy: FunctionAssociation.EventType is required and documented "You cannot use origin-facing event types (origin-request and origin-response) with a CloudFront function." Associating origin-request is refused with a validation message whose fixed text is "<N> validation errors detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy value set: [viewer-request, viewer-response]". Only the error count, the rejected value and the member path are variable, all taken from the request; the phrase after "constraint: " is fixed text.
Emulator: the identical message but with the extra word "enum": "...failed to satisfy constraint: Member must satisfy enum value set: [viewer-request, viewer-response]".
Why: clients and test suites that match on AWS's constraint sentence do not recognise the refusal, and a message that differs from AWS's wording is itself a defect.
Fix: the shared enum-constraint validation message formatter used by the distribution-config validator.
Traces: cloudfront__s3.unpublished-or-origin-event-function-association-rejected.

## Evidence
Cause 1 — expected the bucket-root index.html; got 200 with '<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><IsTruncated>false</IsTruncated><Marker></Marker><Contents><Key>basalt-tenant/app.js</Key><LastModified>...</Last' (AssertionError: / served ..., expected the bucket-root index.html).

Cause 2 — expected 200 + object bytes; got, repeatedly to the 300 s deadline, (403, {'x-cache': 'Error from cloudfront', ...}, '<Error><Code>AccessDenied</Code><Message>Access Denied</Message>...') and 'DATAPLANE_UNIMPLEMENTED: https://<edge>/onyx-test/index.html answering 200 not met within 300s (last: <Response [403]>)'.

Cause 3 — expected 200 for 'GET /app/config.json from https://app.example.com'; got, to the 300 s deadline, (403, 'cross-site origin').

Cause 4 — expected 403 with '<Code>AccessDenied</Code>' for /onyx-test/ while only s3:GetObject is granted; got HTTP 404 '<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message><Key>onyx-test/</Key>...'.

Cause 5 — expected 200 for the published viewer-request rewrite of /onyx-test/; got, to the 300 s deadline, (404, {'x-cache': 'Hit from cloudfront', ...}, '<Error><Code>NoSuchKey</Code>...<Key>onyx-test/</Key>...').

Cause 6 — expected "1 validation errors detected: Value 'origin-request' at 'distributionConfig.defaultCacheBehavior.functionAssociations.items.1.member.eventType' failed to satisfy constraint: Member must satisfy value set: [viewer-request, viewer-response]"; got the same text with "Member must satisfy enum value set: [viewer-request, viewer-response]".

## Steps to Reproduce
Replay the behavioral suite and re-run the cloudfront__s3 checks listed in trace_ids. By hand: (1) create a bucket, seed index.html at the root and <tenant>/index.html, create an origin access control, create a distribution over the bucket's REST endpoint with DefaultRootObject=index.html and a bucket policy granting s3:GetObject to cloudfront.amazonaws.com with AWS:SourceArn = the distribution ARN; wait for Status=Deployed; GET / and GET /<tenant>/index.html at the distribution domain. (2) GET /<tenant>/ with only s3:GetObject granted, then grant s3:ListBucket and GET again. (3) attach a response-headers policy with a CorsConfig allowing https://app.example.com and GET an object with that Origin header. (4) publish a CloudFront Function that rewrites event.request.uri and associate it at viewer-request, then GET the directory-style path. (5) call UpdateDistribution/CreateDistribution with a FunctionAssociation whose EventType is origin-request and compare the validation message.
