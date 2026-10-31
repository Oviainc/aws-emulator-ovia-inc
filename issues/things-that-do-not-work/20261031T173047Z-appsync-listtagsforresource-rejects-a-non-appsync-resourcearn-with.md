Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.op-contract-listTagsForResource
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.op-contract-listTagsForResource|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: cb7739335bb9fe0dace5d2e35069409724bd616f

# AppSync ListTagsForResource rejects a non-AppSync resourceArn with a truncated BadRequestException sentence (missing the ": expected vendor appsync" diagnosis)

> _Note: this issue was identified against the code base at commit `cb7739335bb9fe0dace5d2e35069409724bd616f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One net-new root cause in the AppSync tagging plane's ARN validation.

Cause 1 of 1: ListTagsForResource's refusal of a resourceArn that is not an AppSync resource ARN carries only the first half of AWS's sentence, dropping the clause that says WHICH part of the ARN was wrong.

AWS: ListTagsForResource takes resourceArn as an httpLabel constrained to @length min 70 / max 75 and @pattern ^arn:aws:appsync:[A-Za-z0-9_/.-]{0,63}:\d{12}:apis/[0-9A-Za-z_-]{26}$ (Smithy: ListTagsForResource.resourceArn on ResourceArn; modeled errors AccessDeniedException 403, BadRequestException 400, InternalFailureException 500, LimitExceededException 429, NotFoundException 404, UnauthorizedException 401; http GET /v1/tags/{resourceArn}). A value that clears the client-side minimum length but whose ARN vendor/service field is not `appsync` is refused server-side with HTTP 400, error code BadRequestException, and the message "Invalid input resource arn: expected vendor appsync" (recorded behavior only — the model documents the pattern but not the sentence). Fixed text: "Invalid input resource arn: expected vendor "; the variable part is the vendor/service token AWS expected for the operation being called, i.e. `appsync` for this service's ARNs. Nothing in the message is taken from the request, so no recorded request value belongs in the template.

Emulator: the same call answers BadRequestException with the message "Invalid input resource arn" — the sentence is cut off at the colon, so the ": expected vendor <vendor>" clause that names the failing ARN field is never emitted. The status (400) and the error code (BadRequestException) are correct; only the message diverges.

Why: the error message is a requirement. A client (or an SDK-level test, or a support runbook) that matches AWS's refusal text for a malformed tagging ARN does not match the emulator's, and a human debugging a mistyped ARN loses the one piece of information the message exists to carry — that it was the ARN's vendor segment, not its length, account id or resource id, that failed. The refusal is indistinguishable from any other malformed-ARN rejection.

Fix: the ListTagsForResource handler's resourceArn validation stage (the shared AppSync ARN parser it calls) must build the full refusal sentence, appending ": expected vendor appsync" when the parsed ARN's service/vendor field is not appsync, rather than returning the bare prefix. The happy path of the same check passes — CreateGraphqlApi's tags round-trip correctly through ListTagsForResource — so only the refusal wording is wrong.

Traces: appsync.op-contract-listTagsForResource.

Related open issues that do NOT cover this: "AppSync validation refusals are emulator-authored: wrong message text, wrong error class, and one check missing (10 causes)" (20261029T200818Z) carries no ListTagsForResource replay key (its keys are CreateDataSource/UpdateDataSource/CreateResolver/ListDataSources/CreateFunction/ListApis/GetIntrospectionSchema/CreateApi); "AppSync tagging operations are not gated by identity-based IAM" (20261027T222245Z) is about the missing IAM deny gate on TagResource/UntagResource/ListTagsForResource, not about ARN-validation wording; "AppSync control-plane error semantics: not-found/bad-request codes inverted..." (20261029T174359Z) concerns wrong error CODES, while here the code is already correct.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check appsync.op-contract-listTagsForResource (verdict: mismatch, group error-semantics).

Assertion from the replay:
  File "check_appsync_op_contract_listTagsForResource.py", line 38, in run
    assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
  AssertionError: expected message 'Invalid input resource arn: expected vendor appsync', got 'Invalid input resource arn'

Expected (AWS): HTTP 400, Error.Code = BadRequestException, Error.Message = "Invalid input resource arn: expected vendor appsync".
Actual (emulator): HTTP 400, Error.Code = BadRequestException (both correct), Error.Message = "Invalid input resource arn" — the ": expected vendor appsync" clause is absent.

The preceding assertions in the same check pass: CreateGraphqlApi(name=..., authenticationType=API_KEY, tags={"Team":"checkout","Env":"test"}) followed by ListTagsForResource(resourceArn=<the api's arn>) returns exactly those two tags, and the error code assertion (code == "BadRequestException") also passes. The sole divergence is the message text.

## Steps to Reproduce
1. Clone the repo and boot the emulator; replay the behavioral suite simparity-behavioral-aws-v1.
2. Check appsync.op-contract-listTagsForResource reports verdict `mismatch` with the AssertionError above.
3. By hand, against the emulator's appsync endpoint:
   a. appsync CreateGraphqlApi with name=<any>, authenticationType=API_KEY, tags={"Team":"checkout","Env":"test"}; note graphqlApi.arn and graphqlApi.apiId.
   b. appsync ListTagsForResource with resourceArn=<that arn> -> 200 with both tags (this part is correct).
   c. appsync ListTagsForResource with resourceArn="arn:aws:s3:::not-an-appsync-resource-padded-to-clear-the-minimum-arn-length-check" (long enough to clear the modeled @length min 70, but whose vendor segment is `s3`, not `appsync`).
   d. Observe HTTP 400 / BadRequestException with message "Invalid input resource arn"; AWS answers the same status and code with "Invalid input resource arn: expected vendor appsync".
4. Clean up: appsync DeleteGraphqlApi with the apiId from step 3a.
