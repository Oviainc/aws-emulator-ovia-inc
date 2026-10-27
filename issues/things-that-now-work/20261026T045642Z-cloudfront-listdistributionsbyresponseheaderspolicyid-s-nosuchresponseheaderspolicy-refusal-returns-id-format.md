Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-distributions-by-response-headers-policy-id-index-consistency
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.list-distributions-by-response-headers-policy-id-index-consistency|run#8|AssertionError|assert message == "The specified response headers policy does not exist.", f"unexpected message {message!r}"
Issue-Commit: d20f3863f60340619cdbd48f5982ebeef462a933

# CloudFront ListDistributionsByResponseHeadersPolicyId's NoSuchResponseHeadersPolicy refusal returns "Id format is invalid" instead of "The specified response headers policy does not exist."

> _Note: this issue was identified against the code base at commit `d20f3863f60340619cdbd48f5982ebeef462a933`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDistributionsByResponseHeadersPolicyId

## Description
One net-new root cause in the CloudFront emulator, grounded in behavioral replay.

=== Cause 1 of 1: the shared NoSuchResponseHeadersPolicy helper applies an id-SHAPE precheck that ListDistributionsByResponseHeadersPolicyId does not have on AWS, so an unknown policy id is refused with the malformed-identifier sentence instead of the absent-policy sentence ===

AWS: Smithy: ListDistributionsByResponseHeadersPolicyId, GET /2020-05-31/distributionsByResponseHeadersPolicyId/{ResponseHeadersPolicyId}, success status 200; input ResponseHeadersPolicyId (string, @required, @httpLabel), Marker (@httpQuery), MaxItems (integer, @httpQuery); output DistributionIdList (@httpPayload); modeled errors AccessDenied (HTTP 403), InvalidArgument (HTTP 400), NoSuchResponseHeadersPolicy (HTTP 404). Smithy shape NoSuchResponseHeadersPolicy: client error, httpError 404, single member Message (string), shape documentation "<p>The response headers policy does not exist.</p>". The model does NOT state the wire text of that Message member, so the recording is the authority. The recorded behavior for THIS operation: a ResponseHeadersPolicyId that names no policy — including one that is not in the shape CloudFront mints policy ids in — is answered with HTTP 404, Error.Code "NoSuchResponseHeadersPolicy" and Error.Message exactly "The specified response headers policy does not exist." That message is entirely fixed text: it states a property of the store (no policy by that id), interpolates nothing, and in particular never echoes the submitted id back, so it is the verbatim requirement for this error class on this operation. Note the contrast that makes this a per-operation fact rather than a service-wide one: the open issue 20261025T220525Z-cloudfront-getresponseheaderspolicyconfig-s-nosuchresponseheaderspolicy-refusal-returns-the-error.md records that GetResponseHeadersPolicyConfig answers a malformed id with the DIFFERENT fixed sentence "Id format is invalid". AWS therefore does not share one message between the two reads; the listing operation answers the absent-policy sentence whatever the id's shape.

Emulator: the status (404) and the code (NoSuchResponseHeadersPolicy) are right — the check's code assertion passed — but the message is wrong: the emulator answers Error.Message "Id format is invalid". The cause is the SHARED refusal helper, which gates the message on the id's shape for every caller: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs:7173-7181 defines no_such_response_headers_policy(id) as `if is_minted_uuid(id) { NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE } else { RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE }`, where RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE (state.rs:7191) is "Id format is invalid" and NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE (state.rs:7287-7288) is the correct "The specified response headers policy does not exist." The listing handler calls that helper with the raw input id (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distributions_by_response_headers_policy_id.rs:204-216), so any id outside the minted-UUID shape takes the format branch. The correct string is already present in the service and reachable — only the per-operation routing to it is wrong. The rest of the operation is implemented and correct: the same check's happy-path legs passed (CreateResponseHeadersPolicy, CreateDistribution naming the policy in DefaultCacheBehavior.ResponseHeadersPolicyId, TagResource, then the paged ListDistributionsByResponseHeadersPolicyId walk enumerating the distribution id for the real policy id), which localizes the defect to the refusal's message.

Why: an error message is part of the error contract, and here it is the ONLY signal available. AWS reuses one code (NoSuchResponseHeadersPolicy) and one status (404) for both "the id you gave is not shaped like a CloudFront policy id" and "a well-formed id that names nothing", so a client, SDK retry/branching wrapper, IaC provider or operator runbook that must tell those apart has nothing but the message. Against this emulator every caller that passes a non-UUID policy id to the listing — e.g. a stale or hand-written id, or a name mistakenly used where an id belongs — receives the malformed-identifier sentence where AWS sends the absent-policy sentence. String assertions on AWS's message fail, branching on the message routes to the wrong handler, and anything that logs or surfaces the message to a user shows text AWS never sends for this call.

Fix: the operation's error construction and the shared refusal helper behind it. The id-shape branch in crate::state::no_such_response_headers_policy must not apply to every caller of the helper: ListDistributionsByResponseHeadersPolicyId must answer NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE for any id the store holds no readable policy for, regardless of whether the id matches the minted-id shape, while the operation that genuinely serves "Id format is invalid" (GetResponseHeadersPolicyConfig, per the open issue named above) keeps that text. Whichever way the split is expressed — a per-operation message argument, or two distinct helpers — every sibling caller of the helper (GetResponseHeadersPolicy, UpdateResponseHeadersPolicy, DeleteResponseHeadersPolicy) must be checked against its own recording rather than inheriting one shape-based rule. This is a message-text divergence only; the status, the code and the page walk are already right. The open issue 20261025T220525Z-cloudfront-getresponseheaderspolicyconfig-s-nosuchresponseheaderspolicy-refusal-returns-the-error.md names GetResponseHeadersPolicyConfig only and does not carry this check's replay key, so this cause is filed separately — but the two are coupled and a fix for one must not regress the other.

Traces: cloudfront.list-distributions-by-response-headers-policy-id-index-consistency.

## Evidence
Cause 1 — check cloudfront.list-distributions-by-response-headers-policy-id-index-consistency [mismatch]

  Error: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_cloudfront_list_distributions_by_response_headers_policy_id_index_consistency.py", line 154, in run
    assert message == "The specified response headers policy does not exist.", f"unexpected message {message!r}"
  AssertionError: unexpected message 'Id format is invalid'

Expected (AWS, per the check's recorded oracle): ListDistributionsByResponseHeadersPolicyId(ResponseHeadersPolicyId=<an id no policy exists for>) -> HTTP 404, Error.Code "NoSuchResponseHeadersPolicy", Error.Message "The specified response headers policy does not exist."
  - Fixed text: the whole sentence "The specified response headers policy does not exist." — every character of it.
  - Variable parts: NONE. Nothing from the request (not the submitted ResponseHeadersPolicyId) and nothing from emulator state appears in the message.

Actual (emulator): HTTP 404, Error.Code "NoSuchResponseHeadersPolicy" (correct), Error.Message "Id format is invalid" — a different fixed sentence, the one this service reserves for an identifier outside the minted-id shape. The code assertion immediately before (line 153: code == "NoSuchResponseHeadersPolicy") PASSED; only the message assertion failed.

Controls that passed in the same check, localizing the defect to the refusal's message:
  - CreateResponseHeadersPolicy with ResponseHeadersPolicyConfig {Name: <unique>, Comment: "sim2real behavioral check response headers policy", CustomHeadersConfig: {Quantity: 1, Items: [{Header: "X-Sim2Real-Test", Value: "true", Override: false}]}} succeeded and returned an Id.
  - CreateDistribution with DefaultCacheBehavior.ResponseHeadersPolicyId = that id succeeded, and TagResource on the distribution ARN succeeded.
  - The paged walk of ListDistributionsByResponseHeadersPolicyId (MaxItems="100", following NextMarker) enumerated the created distribution id for the REAL policy id — so the operation, its index and its pagination all work.

Source confirmation (citations, not a patch):
  - services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_distributions_by_response_headers_policy_id.rs:204-216 — when store.readable_response_headers_policy(&input.response_headers_policy_id) is None, returns NoSuchResponseHeadersPolicy(crate::state::no_such_response_headers_policy(&input.response_headers_policy_id)).
  - services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs:7173-7181 — the helper selects its Message with `if is_minted_uuid(id)`: the true branch yields NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE, the false branch yields RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE.
  - services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs:7191 — RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE = "Id format is invalid" (the string that reached the wire).
  - services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs:7287-7288 — NO_SUCH_RESPONSE_HEADERS_POLICY_MESSAGE = "The specified response headers policy does not exist." (the required string, already present but not selected for this call).

Cross-reference: open issue 20261025T220525Z-cloudfront-getresponseheaderspolicyconfig-s-nosuchresponseheaderspolicy-refusal-returns-the-error.md records the opposite expectation for GetResponseHeadersPolicyConfig (AWS sends "Id format is invalid" there). Together the two recordings show the message is per-operation, not a single shape-based rule, which is why the shared helper cannot decide it from the id alone.

## Steps to Reproduce
1. CreateResponseHeadersPolicy with ResponseHeadersPolicyConfig = {Name: <unique>, Comment: "sim2real behavioral check response headers policy", CustomHeadersConfig: {Quantity: 1, Items: [{Header: "X-Sim2Real-Test", Value: "true", Override: false}]}}; keep ResponseHeadersPolicy.Id.
2. CreateDistribution with Enabled=false, one S3 origin, and DefaultCacheBehavior = {TargetOriginId: "primary-origin", ViewerProtocolPolicy: "allow-all", CachePolicyId: "658327ea-f89d-4fab-a63d-7e88639e58f6", ResponseHeadersPolicyId: <that policy id>}; keep Distribution.Id.
3. ListDistributionsByResponseHeadersPolicyId(ResponseHeadersPolicyId=<that policy id>, MaxItems="100"), following NextMarker -> expect the distribution id in DistributionIdList.Items (this passes today).
4. ListDistributionsByResponseHeadersPolicyId(ResponseHeadersPolicyId="not-a-real-policy-id") — an id no policy exists for and which is not in the minted-id shape.
   Expect (AWS): HTTP 404, Error.Code "NoSuchResponseHeadersPolicy", Error.Message "The specified response headers policy does not exist."
   Observed: HTTP 404, Error.Code "NoSuchResponseHeadersPolicy", Error.Message "Id format is invalid".
5. Re-run step 3 as the positive control: the real policy id must still resolve to the distribution id after the rejected call.
6. Replay cloudfront.list-distributions-by-response-headers-policy-id-index-consistency to confirm. Also replay cloudfront.op-contract-getResponseHeadersPolicyConfig to confirm the coupled GetResponseHeadersPolicyConfig expectation ("Id format is invalid") was not regressed by the fix.
