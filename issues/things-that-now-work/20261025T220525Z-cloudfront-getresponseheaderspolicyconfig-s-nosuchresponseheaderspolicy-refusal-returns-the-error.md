Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-getResponseHeadersPolicyConfig
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-getResponseHeadersPolicyConfig|run#15|AssertionError|assert msg == "Id format is invalid", f"unexpected NoSuchResponseHeadersPolicy message: {msg!r}"
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# CloudFront GetResponseHeadersPolicyConfig's NoSuchResponseHeadersPolicy refusal returns the error shape's documentation sentence instead of AWS's "Id format is invalid" message

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResponseHeadersPolicyConfig

## Description
One net-new root cause in the CloudFront emulator, grounded in behavioral replay.

=== Cause 1 of 1: the NoSuchResponseHeadersPolicy message is the Smithy error shape's documentation text, not the message CloudFront actually sends ===

AWS: Smithy: GetResponseHeadersPolicyConfig, GET /2020-05-31/response-headers-policy/{Id}/config, success status 200; input Id (string, @required, @httpLabel); output ResponseHeadersPolicyConfig (@httpPayload) + ETag (@httpHeader "ETag"); modeled errors AccessDenied (HTTP 403) and NoSuchResponseHeadersPolicy (HTTP 404). Smithy shape NoSuchResponseHeadersPolicy: client error, httpError 404, single member Message (string), shape documentation "<p>The response headers policy does not exist.</p>". The model does NOT state what text AWS puts in that Message member — the shape doc is documentation of the error class, not the wire message. The recorded behavior is the authority for the message: CloudFront answers a GetResponseHeadersPolicyConfig for an Id that is not a well-formed response-headers-policy identifier with HTTP 404, Error.Code "NoSuchResponseHeadersPolicy" and Error.Message exactly "Id format is invalid". That message is entirely fixed text — it interpolates nothing from the request (it does not echo the supplied Id) — so it is reproducible verbatim and is the requirement for this error class.

Emulator: the status (404) and the code (NoSuchResponseHeadersPolicy) are right, but the message is wrong: the emulator answers Error.Message "The response headers policy does not exist." — the error shape's own documentation sentence, lifted verbatim out of the model. The happy-path legs of the same check all passed (CreateResponseHeadersPolicy then GetResponseHeadersPolicyConfig round-trips Name, Comment and RemoveHeadersConfig.Quantity/Items[0].Header exactly), so this operation is implemented and the divergence is confined to the refusal's message. Note that the open issue 20261024T182555Z-cloudfront-getresponseheaderspolicyconfig-stub.md ("Implement cloudfront GetResponseHeadersPolicyConfig: unimplemented scaffold stub") no longer describes this operation — it answers from state now — and it does not cover a message divergence.

Why: an error message is part of the error contract. A client, SDK retry/branching wrapper or operator runbook that distinguishes "the id you gave is syntactically not a CloudFront policy id" from "a well-formed id that names nothing" has only the message to go on, because AWS reuses one code (NoSuchResponseHeadersPolicy) and one status (404) for both. Against this emulator every such client sees the generic not-found sentence, so the malformed-id class becomes indistinguishable from the absent-policy class, string assertions on AWS's message fail, and anything that logs or surfaces the message shows text AWS never sends.

Fix: the operation's error construction, and the shared refusal helper behind it — services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_response_headers_policy_config.rs lines 158-165 build the refusal from crate::state::no_such_response_headers_policy() (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs), which sources its Message from the model's shape documentation. The helper must carry CloudFront's own message text, and the id-format case must be distinguished from the well-formed-but-unknown-id case. Because the helper is shared, the sibling response-headers-policy readers/mutators (GetResponseHeadersPolicy, UpdateResponseHeadersPolicy, DeleteResponseHeadersPolicy) will carry the same wrong message and should be corrected with it. This is the same defect class already filed for another CloudFront operation in 20261025T130131Z-cloudfront-verifydnsconfiguration-s-invalidargument-refusal-uses-the-error-shape.md (InvalidArgument built from the error shape's documentation plus an emulator-authored reason); that issue names VerifyDnsConfiguration only and does not carry this check's replay key, so this cause is filed separately.

Traces: cloudfront.op-contract-getResponseHeadersPolicyConfig.

## Evidence
Cause 1 — check cloudfront.op-contract-getResponseHeadersPolicyConfig [mismatch]

  Error: File "check_cloudfront_op_contract_getResponseHeadersPolicyConfig.py", line 46, in run
    assert msg == "Id format is invalid", f"unexpected NoSuchResponseHeadersPolicy message: {msg!r}"
  AssertionError: unexpected NoSuchResponseHeadersPolicy message: 'The response headers policy does not exist.'

Expected (AWS, per the check's recorded oracle): GetResponseHeadersPolicyConfig(Id=<an id that is not a well-formed CloudFront response-headers-policy id>) -> HTTP 404, Error.Code "NoSuchResponseHeadersPolicy", Error.Message "Id format is invalid" (fixed text; no part of it is derived from the request or from emulator state).

Actual (emulator): HTTP 404, Error.Code "NoSuchResponseHeadersPolicy" (correct), Error.Message "The response headers policy does not exist." — byte-for-byte the documentation of the NoSuchResponseHeadersPolicy shape in this service's Smithy model ("<p>The response headers policy does not exist.</p>" with the HTML stripped).

Controls that passed in the same check, which is what localizes the defect to the message: CreateResponseHeadersPolicy succeeded; GetResponseHeadersPolicyConfig on the created id returned ResponseHeadersPolicyConfig with Name == the created name, Comment == "sim2real behavioral check config", RemoveHeadersConfig.Quantity == 1 and RemoveHeadersConfig.Items[0].Header == "X-Powered-By"; and the refusal's CODE assertion (code == "NoSuchResponseHeadersPolicy") also passed. Only the message assertion failed.

Source confirmation (citation, not a patch): services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/get_response_headers_policy_config.rs:158-165 returns GetResponseHeadersPolicyConfigErrorModeled::NoSuchResponseHeadersPolicy(crate::state::no_such_response_headers_policy()) when store.readable_response_headers_policy(&input.id) is None; the same file's modeled-contract header records the model's error documentation at line 76 as "NoSuchResponseHeadersPolicy (HTTP 404) — The response headers policy does not exist.", which is the string that reaches the wire.

## Steps to Reproduce
1. CreateResponseHeadersPolicy with ResponseHeadersPolicyConfig = {Name: <unique>, Comment: "sim2real behavioral check config", RemoveHeadersConfig: {Quantity: 1, Items: [{Header: "X-Powered-By"}]}} and keep the returned ResponseHeadersPolicy.Id.
2. GetResponseHeadersPolicyConfig(Id=<that id>) -> expect 200 with the config round-tripped (this passes today).
3. GetResponseHeadersPolicyConfig(Id="NOSUCHPOLICYID000001") — an id that is not a well-formed CloudFront response-headers-policy identifier.
   Expect (AWS): HTTP 404, Error.Code "NoSuchResponseHeadersPolicy", Error.Message "Id format is invalid".
   Observed: HTTP 404, Error.Code "NoSuchResponseHeadersPolicy", Error.Message "The response headers policy does not exist."
4. Replay cloudfront.op-contract-getResponseHeadersPolicyConfig to confirm.
