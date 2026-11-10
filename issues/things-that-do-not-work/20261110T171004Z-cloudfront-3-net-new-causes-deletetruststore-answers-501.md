Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-deleteTrustStore cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected cloudfront.e2e-distribution-webacl-association-blocks-request
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-deleteTrustStore|run#16|AssertionError|assert code == "InvalidIfMatchVersion", f"expected InvalidIfMatchVersion, got {code}"
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.e2e-distribution-webacl-association-blocks-request|_wait_distribution_deployed#28|Timeout|time.sleep(interval_s)
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# cloudfront: 3 net-new causes — DeleteTrustStore answers 501 NotImplemented, a throwing viewer-request function never yields 503 at the edge, and an associated Web ACL never blocks a request

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTrustStore

## Description
Three net-new root causes in the CloudFront emulator, all grounded in failing behavioural checks that no open issue carries the replay key for. Related open issues describe neighbouring symptoms but do not cover these checks: 20261109T181319Z-cloudfront-3-causes-a-distribution-s-domainname-is.md (distribution DomainName / edge serving), 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md (edge data plane), 20261109T181319Z-cloudformation-cloudfront-s3-cloudformation-refuses-aws-cloudfront-truststore.md (the CFN resource type, not the API operation). All three causes must be closed before submitting.

Cause 1 of 3: DeleteTrustStore is an unimplemented scaffold stub.
AWS: Smithy (com.amazonaws.cloudfront#DeleteTrustStore): DELETE /2020-05-31/trust-store/{Id}, success status 204, required members Id (ResourceId, @length 1..64, @httpLabel) and IfMatch (@httpHeader If-Match); modeled errors AccessDenied 403, CannotDeleteEntityWhileInUse 409, EntityNotFound 404, InvalidArgument 400, InvalidIfMatchVersion 400, PreconditionFailed 412. A delete carrying an If-Match that is not the resource's current ETag is refused with HTTP 400, error code InvalidIfMatchVersion and the message 'The If-Match version is missing or not valid for the resource.' — that whole sentence is fixed text for this error class, with no variable part. A refused delete is a no-op: the trust store still resolves through GetTrustStore and its ETag is unchanged (a refusal never advances the ETag). With the current ETag the delete succeeds (204, empty body), after which GetTrustStore on the same identifier is refused with HTTP 404, code EntityNotFound and the message 'The specified trust store does not exist.' (fixed text, no variable part).
Emulator: every DeleteTrustStore call answers HTTP 501 with error code NotImplemented. The check's wrong-ETag call therefore surfaces NotImplemented instead of InvalidIfMatchVersion ('AssertionError: expected InvalidIfMatchVersion, got NotImplemented'), and the success path, the ETag-unchanged invariant and the post-delete EntityNotFound can never be exercised. services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_trust_store.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() returns crate::scaffolded::Scaffolded::not_implemented("DeleteTrustStore").
Why: a client that provisions mTLS trust stores cannot delete one, and cannot distinguish a stale-ETag conflict (retry after re-reading the ETag) from a server that has no such feature. 501/NotImplemented is not in the operation's modeled error set, so SDK retry and error-handling branches take no sensible path at all, and cleanup code leaks trust stores.
Fix: the DeleteTrustStore handler and its optimistic-concurrency check — remove the scaffold stub, resolve Id in the CloudFront service state, compare IfMatch against the stored ETag, refuse a mismatch with InvalidIfMatchVersion without mutating state, delete on a match and answer the modeled 204, and make GetTrustStore answer EntityNotFound afterwards.
Traces: cloudfront.op-contract-deleteTrustStore.

Cause 2 of 3: a viewer-request CloudFront Function that throws does not make the edge answer 503.
AWS: recorded behavior only (this is the CloudFront data plane; the Smithy model describes only the control plane). The recording shows: a cloudfront-js-2.0 function published to LIVE and associated as viewer-request on a behaviour using the managed CachingDisabled cache policy. For a request whose path takes the function's throwing branch, the edge answers HTTP 503, serves NONE of the origin object's bytes (the origin is never contacted) and sets the response header 'x-cache: FunctionExecutionError from cloudfront' — that header value is fixed text for this failure class. The same function's non-throwing branch on the same distribution keeps serving HTTP 200 with the origin body, both before and after the 503, so one failing invocation does not poison the distribution. TestFunction on the same LIVE function reports the thrown error in TestResult.FunctionErrorMessage.
Emulator: no edge response is produced at all. The check resolves the distribution's DomainName through ctx.endpoint_route() and polls GET {edge}/index.txt for the 200 positive control; that poll never succeeds, so the check is killed at the harness cap ('Check timed out after 60.0s') before it can even reach the throwing branch. Neither the 503, nor the x-cache classification, nor the 'origin bytes must not be served' invariant is ever produced.
Why: a client that installs a viewer-request function to guard its origin gets no edge behaviour at all — a malformed or failing function is indistinguishable from a healthy one, and code that branches on 503 / x-cache=FunctionExecutionError (alerting, retry, fallback) is never exercised. Worse, the origin-protection guarantee (a thrown exception must never leak origin bytes) cannot be verified.
Fix: the CloudFront edge data plane — the distribution's served endpoint and its function-association execution step: run the associated LIVE viewer-request function, and on an unhandled exception short-circuit the request with 503, the x-cache FunctionExecutionError classification and no origin fetch.
Traces: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected.

Cause 3 of 3: a Web ACL associated with a distribution is not enforced at the edge.
AWS: recorded behavior only for the data-plane effect; the control-plane operations are modeled (AssociateDistributionWebACL / DisassociateDistributionWebACL, both taking Id and IfMatch). The recording shows: a WAFv2 Web ACL with Scope=CLOUDFRONT, DefaultAction Allow and one ByteMatchStatement on UriPath with Action Block, associated to the distribution with AssociateDistributionWebACL. After the distribution returns to Deployed, a request to the matching path is refused AT THE EDGE with HTTP 403 and never reaches the origin, while a non-matching path continues to serve the origin object with HTTP 200. After DisassociateDistributionWebACL, the previously blocked path serves its real origin content again with HTTP 200.
Emulator: nothing is served through the distribution's endpoint, so the positive control (GET /index.html expected to return the origin body) never succeeds and the check is killed at the harness cap ('Check timed out after 60.0s'). The Web ACL association therefore has no observable effect: neither the 403 for the blocked path nor the restored 200 after disassociation is ever produced.
Why: a client relying on WAF to block traffic at the edge is silently unprotected — the association is accepted by the control plane but enforces nothing, so a security control that AWS applies before the origin is reached is absent, and the disassociation path cannot be verified either.
Fix: the CloudFront edge data plane's request-admission step — evaluate the distribution's associated WAFv2 Web ACL (default action plus rule statements) before the origin fetch, refuse a Block match with 403, and drop the evaluation when the Web ACL is disassociated.
Traces: cloudfront.e2e-distribution-webacl-association-blocks-request.

## Evidence
Cause 1 — cloudfront.op-contract-deleteTrustStore, verdict mismatch:
  File "check_cloudfront_op_contract_deleteTrustStore.py", line 60, in run
    assert code == "InvalidIfMatchVersion", f"expected InvalidIfMatchVersion, got {code}"
  AssertionError: expected InvalidIfMatchVersion, got NotImplemented
Expected: HTTP 400 / InvalidIfMatchVersion / 'The If-Match version is missing or not valid for the resource.' for cf.delete_trust_store(Id=<id>, IfMatch=<not the current ETag>); then GetTrustStore unchanged with the same ETag; then delete with the current ETag succeeds (204) and GetTrustStore answers HTTP 404 / EntityNotFound / 'The specified trust store does not exist.'
Actual: HTTP 501 / NotImplemented for the DeleteTrustStore call. Handler: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_trust_store.rs:183 returns Scaffolded::not_implemented("DeleteTrustStore"); the SCAFFOLDED marker is at line 4.

Cause 2 — cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected, verdict mismatch:
  Error: Check timed out after 60.0s
Expected (from the check body): GET {edge}/index.txt -> 200 with the origin body; GET {edge}/boom/index.txt -> 503, response body containing none of the origin bytes, and header x-cache == 'FunctionExecutionError from cloudfront'; GET {edge}/index.txt again -> 200; TestFunction FunctionErrorMessage reporting the thrown error.
Actual: the first edge GET never returns a usable response, so the check never reaches any of those assertions and is killed at the 60 s harness cap.

Cause 3 — cloudfront.e2e-distribution-webacl-association-blocks-request, verdict mismatch:
  Error: Check timed out after 60.0s
Expected (from the check body): with the CLOUDFRONT-scoped Web ACL associated, GET {edge}/index.html -> 'allowed-content' (200) and GET {edge}/blockme.html -> HTTP 403; after DisassociateDistributionWebACL, GET {edge}/blockme.html -> 'blocked-path-content' (200).
Actual: the positive-control GET never succeeds, so neither the 403 nor the restored 200 is ever observed and the check is killed at the 60 s harness cap.

## Steps to Reproduce
Cause 1: create an S3 bucket holding a PEM CA bundle, call cloudfront CreateTrustStore with CaCertificatesBundleSource.CaCertificatesBundleS3Location pointing at it, poll GetTrustStore until Status is active, then call DeleteTrustStore(Id=<trust store id>, IfMatch="E000000000FAKEBOGUS"). Observe HTTP 501 / NotImplemented instead of HTTP 400 / InvalidIfMatchVersion / 'The If-Match version is missing or not valid for the resource.'. Then call DeleteTrustStore with the current ETag and observe it is refused the same way instead of answering 204.

Cause 2: create a private bucket with objects index.txt and boom/index.txt, create an OAC (s3/always/sigv4), CreateFunction with runtime cloudfront-js-2.0 whose handler throws for URIs under /boom/, PublishFunction to LIVE, CreateDistribution over the bucket with the managed CachingDisabled cache policy (4135ea2d-6df8-44a3-9df3-4b5a84be39ad) and a viewer-request FunctionAssociation, wait for Deployed, then GET /index.txt and /boom/index.txt through the distribution's DomainName. Observe that no edge response is served at all, instead of 200 then 503 with x-cache 'FunctionExecutionError from cloudfront'.

Cause 3: create a private bucket with index.html and blockme.html, an OAC, and a distribution over the bucket; wait for Deployed; create a wafv2 Web ACL in us-east-1 with Scope=CLOUDFRONT, DefaultAction Allow and a Block rule matching UriPath CONTAINS 'blockme'; call AssociateDistributionWebACL(Id, WebACLArn, IfMatch) and wait for Deployed; GET /index.html and /blockme.html through the distribution's DomainName. Observe nothing is served, instead of 200 for /index.html and 403 for /blockme.html; then DisassociateDistributionWebACL and observe /blockme.html still serves nothing instead of 200 with the origin body.
