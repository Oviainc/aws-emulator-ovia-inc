Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.overly-permissive-function-publish-stale-etag-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.overly-permissive-function-publish-stale-etag-rejected|run#9|AssertionError|assert code == "PreconditionFailed", (
Issue-Commit: b495a64ceea4ad5841973a813296398afeecb563

# CloudFront PublishFunction answers 501 NotImplemented, so a stale If-Match is never refused with PreconditionFailed and nothing is ever promoted to LIVE

> _Note: this issue was identified against the code base at commit `b495a64ceea4ad5841973a813296398afeecb563`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishFunction

## Description
One root cause.

Cause 1 of 1: CloudFront `PublishFunction` is still the generated scaffold stub, so every call is refused with 501 NotImplemented instead of either publishing the function or returning one of the operation's modeled refusals. Failing check id: cloudfront.overly-permissive-function-publish-stale-etag-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json) models PublishFunction as POST /2020-05-31/function/{Name}/publish with success status 200. Input: `Name` (FunctionName, @httpLabel, required, @length min 1 max 64, @pattern ^[a-zA-Z0-9-_]{1,64}$) and `IfMatch` (@httpHeader "If-Match", required). Output: `FunctionSummary` (@httpPayload). Modeled errors: InvalidArgument (400), InvalidIfMatchVersion (400), NoSuchFunctionExists (404), PreconditionFailed (412 — model doc: "The precondition in one or more of the request fields evaluated to false."), UnsupportedOperation (400). The required If-Match header makes this an optimistic-concurrency operation: a publish whose If-Match is not the function's CURRENT ETag must be refused with error code PreconditionFailed at HTTP 412, and that refusal must have no effect — nothing is promoted to the LIVE stage. A publish carrying the current ETag must succeed with 200 and report FunctionSummary.FunctionMetadata.Stage == "LIVE", after which GetFunction(Stage="LIVE") serves the revision that was actually published, never an earlier one a refused call tried to promote.

Emulator: every PublishFunction request is refused with error code `NotImplemented` (501) and the scaffold message "operation PublishFunction is scaffolded but not implemented in this emulator". The handler body is the untouched generator stub, whose only statement is `Err(crate::scaffolded::Scaffolded::not_implemented("PublishFunction"))`. Consequently none of the modeled outcomes can ever occur: the stale-If-Match path yields NotImplemented rather than PreconditionFailed, and the success path never promotes a function to LIVE.

Why: a client that relies on AWS behavior cannot promote a CloudFront Function to LIVE at all, so no function-at-edge flow can be exercised end to end. Worse for correctness-sensitive clients, the optimistic-concurrency contract is silently absent: code that publishes with a held ETag and expects PreconditionFailed when another writer has updated the function in between gets a 501 NotImplemented instead, which is a different error class (server-side "operation missing") than a 412 client precondition failure. Retry/conflict-resolution logic keyed on PreconditionFailed therefore never engages, and the check can neither observe the refusal nor observe that the refusal left the LIVE stage untouched.

Fix: implement the PublishFunction handler — the operation's handler and its If-Match precondition validation, plus the DEVELOPMENT→LIVE stage promotion in the CloudFront function state. The handler must compare the request's If-Match against the function's current version token and return the modeled PreconditionFailed when they differ (leaving the stored function and its LIVE stage unmodified), return NoSuchFunctionExists for an unknown Name, and on success copy the current DEVELOPMENT revision to LIVE and serialize FunctionSummary with FunctionMetadata.Stage == "LIVE". Delete the SCAFFOLDED marker once implemented.

Related already-open issue: 20261024T182555Z-cloudfront-publishfunction-stub.md records that this operation is an unimplemented stub, but it does not state the If-Match/PreconditionFailed precondition contract or the effect-absence and LIVE-promotion requirements above; this issue carries that behavioral contract and the failing check that proves it.

Traces: cloudfront.overly-permissive-function-publish-stale-etag-rejected. Stub handler: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/publish_function.rs lines 152-170 (the `handle` body returns `Scaffolded::not_implemented`).

## Evidence
Failing behavioral check `cloudfront.overly-permissive-function-publish-stale-etag-rejected` (verdict: mismatch).

Expected (AWS, per the Smithy model): PublishFunction with a stale If-Match is refused with error code `PreconditionFailed` at HTTP 412; the function does NOT appear under Stage=LIVE; re-issuing the same call with the current ETag returns 200 with FunctionSummary.FunctionMetadata.Stage == "LIVE" and GetFunction(Stage="LIVE") serves the newly published code.

Actual (emulator): the call is refused with error code `NotImplemented` (HTTP 501), message "operation PublishFunction is scaffolded but not implemented in this emulator".

Replay assertion:
  AssertionError: PublishFunction with a stale IfMatch must fail with PreconditionFailed, got 'NotImplemented'
  at check_cloudfront_overly_permissive_function_publish_stale_etag_rejected.py line 40

Source evidence: services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/publish_function.rs line 169 — `Err(crate::scaffolded::Scaffolded::not_implemented("PublishFunction"))` is the entire handler body, so no modeled error (PreconditionFailed / InvalidIfMatchVersion / NoSuchFunctionExists / InvalidArgument / UnsupportedOperation) and no success response can ever be produced.

## Steps to Reproduce
1. Point an AWS client at the emulator's CloudFront endpoint.
2. CreateFunction(Name=<name>, FunctionConfig={Comment:"v1", Runtime:"cloudfront-js-2.0"}, FunctionCode=<v1 code>) and keep the returned ETag as `stale_etag`.
3. UpdateFunction(Name=<name>, IfMatch=stale_etag, FunctionConfig={Comment:"v2", Runtime:"cloudfront-js-2.0"}, FunctionCode=<v2 code>) and keep the returned ETag as `current_etag`. `stale_etag` is now out of date.
4. Call PublishFunction(Name=<name>, IfMatch=stale_etag).
   Expected: error code PreconditionFailed, HTTP 412; the function must not appear in ListFunctions(Stage="LIVE").
   Observed: error code NotImplemented, HTTP 501.
5. Call PublishFunction(Name=<name>, IfMatch=current_etag).
   Expected: HTTP 200 with FunctionSummary.FunctionMetadata.Stage == "LIVE", and GetFunction(Name=<name>, Stage="LIVE") returning the v2 code.
   Observed: error code NotImplemented, HTTP 501.

Equivalently, replay the behavioral check `cloudfront.overly-permissive-function-publish-stale-etag-rejected`.
