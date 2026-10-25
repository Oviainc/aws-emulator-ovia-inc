Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission|run#15|AssertionError|assert (no_policy.get("Code"), no_policy.get("Message")) == (
Issue-Commit: 62a489da4fc45e0eadb1189eb41434873a830e66

# ses receipt-rule LambdaAction check cannot start: Lambda GetPolicy on a policy-less function answers the generic not-found refusal (request-derived ARN suffix) instead of the no-policy refusal

> _Note: this issue was identified against the code base at commit `62a489da4fc45e0eadb1189eb41434873a830e66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: GetPolicy's refusal for an EXISTING function that carries no resource policy is served with the generic resource-not-found message template, with a request-derived resource ARN appended, instead of the fixed no-policy text. This is the setup precondition of the SES receipt-rule LambdaAction authorization check, so the whole check — the only check in the corpus that exercises SES's LambdaAction pre-flight against a Lambda resource policy — dies before a single SES call is made.

AWS:      Lambda GetPolicy is modeled with ResourceNotFoundException at HTTP 404 (Smithy: GetPolicy errors = InvalidParameterValueException 400, ResourceNotFoundException 404, TooManyRequestsException 429, ServiceException 500; shape ResourceNotFoundException doc "The resource specified in the request does not exist."). The model does not state message text, so the exact wording is recorded behavior only: for a function that EXISTS but has no resource policy attached, AWS answers 404 ResourceNotFoundException with the service's fixed sentence "The resource you requested does not exist." — a constant with no request-derived suffix. The request-identifying form (the one naming the function ARN) is the refusal AWS uses when the FUNCTION itself cannot be resolved; the two cases are distinguishable by that rule. Note the grounding check is marked an unblessed draft, so treat the exact text as the recorded/expected example and the RULE — a fixed constant for the no-policy case, distinct from the function-not-found case — as the requirement.

Emulator: GetPolicy on a function that was just created (and is readable back via GetFunctionConfiguration with the same FunctionArn) answers ResourceNotFoundException with a message built from the request: "The resource you requested does not exist. Resource: <function ARN>" (observed: "...Resource: arn:aws:lambda:<region>:<account>:function:sim2real-behav-ses-inbound-fn-<suffix>"; the host, region, account and generated function name are environment values, not requirements). So the no-policy case is served by the generic/not-found refusal path rather than its own.

Why:     A client cannot tell "this function has no resource policy yet" from "this function does not exist". Permission-bootstrapping code (the standard add-permission / get-policy reconcile loop used before wiring SES receipt rules, S3 notifications or EventBridge targets to a function) branches on exactly that distinction: it either creates the function again, or aborts, where AWS would have it proceed to AddPermission. In this corpus it also means the SES LambdaAction authorization behavior behind it is never graded at all.

Fix:     The lambda GetPolicy handler's refusal path — it must emit the no-resource-policy refusal (fixed message, no request-derived resource suffix) when the function resolves but has no policy, and reserve the function-identifying not-found message for the case where the function name/ARN does not resolve. Status (404) and code (ResourceNotFoundException) are already correct; only the selection of refusal template is wrong.

Traces:  ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission — fails at the setup precondition assertion ("the fresh function must carry NO resource policy...; GetPolicy answered {'Message': 'The resource you requested does not exist. Resource: arn:aws:lambda:...:function:sim2real-behav-ses-inbound-fn-...', 'Code': 'ResourceNotFoundException'}").

## Evidence
Check ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission, verdict [mismatch].

Sequence: iam CreateRole -> lambda CreateFunction (python3.12, zipped handler) -> lambda GetFunctionConfiguration (succeeds, FunctionArn matches) -> lambda GetPolicy.

Expected (AWS): 404 ResourceNotFoundException, Error.Code = "ResourceNotFoundException", Error.Message = the fixed no-policy sentence "The resource you requested does not exist." (constant; no request-derived suffix).

Actual (emulator): Error.Code = "ResourceNotFoundException" (correct), Error.Message = "The resource you requested does not exist. Resource: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-ses-inbound-fn-b8a5d0473652" — the generic not-found template with the requested function's ARN appended, i.e. the refusal AWS reserves for an unresolvable function, returned for a function the emulator itself had just created and served back.

Assertion raised: "setup precondition: the fresh function must carry NO resource policy, so the only thing broken in the negative legs is the grant; GetPolicy answered {'Message': 'The resource you requested does not exist. Resource: arn:aws:lambda:...', 'Code': 'ResourceNotFoundException'}" (check file line 175).

Consequence: none of the check's SES legs run — CreateReceiptRule with a LambdaAction against (a) a function with no ses.amazonaws.com grant, (b) a grant whose SourceArn names a different receipt rule, (c) the correctly scoped grant — so SES's LambdaAction pre-flight is entirely ungraded by this run.

## Steps to Reproduce
1. Point a botocore/AWS SDK client at the emulator (lambda + iam, same region).
2. iam CreateRole with the lambda.amazonaws.com AssumeRole trust policy.
3. lambda CreateFunction (FunctionName=<new name>, Runtime=python3.12, Handler=handler.handler, Code.ZipFile=<zip containing handler.py>, Role=<role ARN>).
4. lambda GetFunctionConfiguration(FunctionName=<new name>) — observe it succeeds and returns the FunctionArn, proving the function exists.
5. lambda GetPolicy(FunctionName=<new name>) with NO AddPermission call made.
6. Observe the ClientError: Code is ResourceNotFoundException (correct) but Message is the generic not-found text with " Resource: <function ARN>" appended, instead of AWS's fixed no-policy sentence "The resource you requested does not exist." with no request-derived suffix.

Equivalently: replay behavioral check ses.receipt-rule-lambda-action-requires-a-rule-scoped-invoke-permission; it must get past the GetPolicy setup precondition and go on to exercise the three CreateReceiptRule LambdaAction legs.
