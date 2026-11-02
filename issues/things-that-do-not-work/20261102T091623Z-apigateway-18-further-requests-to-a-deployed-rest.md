Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam.authorization-is-decided-before-route-matching-for-an-undefined-verb apigateway__iam.double-slash-in-the-stage-path-is-normalized-before-authorization apigateway__iam.exact-agreement-grant-admits-that-agreement-and-denies-another apigateway__iam.explicit-deny-on-one-agreement-overrides-a-wildcard-allow apigateway__iam.grant-scoped-to-another-api-id-is-denied-on-this-api apigateway__iam.granted-get-on-the-methodless-parent-resource-is-rejected-not-found apigateway__iam.invoke-grant-naming-a-nonexistent-api-is-accepted-by-iam-and-denied-at-the-stage apigateway__iam.lowercase-verb-in-the-invoke-arn-is-denied-case-sensitively apigateway__iam.parent-path-grant-without-a-wildcard-does-not-cover-the-child-denied apigateway__iam.post-scoped-invoke-grant-denies-the-get-only-agreement-endpoint apigateway__iam.rewriting-the-post-grant-to-get-admits-a-fresh-session-and-reverting-denies-again apigateway__iam.session-policy-naming-post-denies-get-that-the-role-grants apigateway__iam.stage-scoped-grant-for-another-stage-is-denied apigateway__iam.unsigned-request-to-the-iam-method-is-rejected-missing-authentication-token apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage
Issue-Key: apigateway__iam.authorization-is-decided-before-route-matching-for-an-undefined-verb|_await#6|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.double-slash-in-the-stage-path-is-normalized-before-authorization|_await#8|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.exact-agreement-grant-admits-that-agreement-and-denies-another|_await#4|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.explicit-deny-on-one-agreement-overrides-a-wildcard-allow|_await#4|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.grant-scoped-to-another-api-id-is-denied-on-this-api|_await#6|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.granted-get-on-the-methodless-parent-resource-is-rejected-not-found|_await#4|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.invoke-grant-naming-a-nonexistent-api-is-accepted-by-iam-and-denied-at-the-stage|_await#12|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.lowercase-verb-in-the-invoke-arn-is-denied-case-sensitively|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.parent-path-grant-without-a-wildcard-does-not-cover-the-child-denied|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.post-scoped-invoke-grant-denies-the-get-only-agreement-endpoint|_await#6|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.rewriting-the-post-grant-to-get-admits-a-fresh-session-and-reverting-denies-again|_await#7|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.session-policy-naming-post-denies-get-that-the-role-grants|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.stage-scoped-grant-for-another-stage-is-denied|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__iam.unsigned-request-to-the-iam-method-is-rejected-missing-authentication-token|_await#4|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage|_await#6|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# apigateway: 18 further requests to a deployed REST API stage never produce any HTTP response (60 s timeout)

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api, deployed stage)

## Description
This issue carries 2 root causes, both on the execute-api data plane of a deployed REST API stage. The open issues 20261023T210711Z-apigateway-iam-all-nine-iam-authorization-checks-against.md and 20261023T210711Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-lifecycle.md describe the same surface but carry only other checks' replay keys, so they do NOT cover the 18 checks listed here; these must be closed alongside them.

Cause 1 of 2: a request to a deployed stage whose method has authorizationType=AWS_IAM never returns.
AWS: recorded behavior only for each case's status and code; what is structural is that every request that reaches a deployed stage produces an HTTP response. The recorded cases are: an identity-policy allow admits exactly the granted method/path and denies another; an explicit Deny on one resource overrides a wildcard Allow; a grant naming another api id or another stage is denied; a path grant without a wildcard does not cover a child path; a lowercase verb in the execute-api ARN does not match (matching is case-sensitive); a grant naming a nonexistent api is accepted by IAM and denied at the stage; a GET on a methodless parent resource answers not-found; a double slash in the stage path is normalized before authorization; authorization is decided before route matching for an undefined verb; an unsigned request is refused MissingAuthenticationToken; a session policy naming POST denies a GET the role grants.
Emulator: no HTTP response is produced for any of them. Each check exhausts the harness budget and the suite reports exactly 'Error: Check timed out after 60.0s' — no status line and no body ever arrive.
Why: a client calling an IAM-authorized API stalls until its own timeout instead of being allowed or refused, and the whole execute-api authorization model (identity policy, resource policy, deny precedence, ARN/verb/path matching, SigV4 verification) is unobservable.
Fix: the execute-api data-plane request path — SigV4 verification plus identity/resource-policy evaluation at the stage — must always produce a response.
Traces: apigateway__iam.authorization-is-decided-before-route-matching-for-an-undefined-verb, apigateway__iam.double-slash-in-the-stage-path-is-normalized-before-authorization, apigateway__iam.exact-agreement-grant-admits-that-agreement-and-denies-another, apigateway__iam.explicit-deny-on-one-agreement-overrides-a-wildcard-allow, apigateway__iam.grant-scoped-to-another-api-id-is-denied-on-this-api, apigateway__iam.granted-get-on-the-methodless-parent-resource-is-rejected-not-found, apigateway__iam.invoke-grant-naming-a-nonexistent-api-is-accepted-by-iam-and-denied-at-the-stage, apigateway__iam.lowercase-verb-in-the-invoke-arn-is-denied-case-sensitively, apigateway__iam.parent-path-grant-without-a-wildcard-does-not-cover-the-child-denied, apigateway__iam.post-scoped-invoke-grant-denies-the-get-only-agreement-endpoint, apigateway__iam.rewriting-the-post-grant-to-get-admits-a-fresh-session-and-reverting-denies-again, apigateway__iam.session-policy-naming-post-denies-get-that-the-role-grants, apigateway__iam.stage-scoped-grant-for-another-stage-is-denied, apigateway__iam.unsigned-request-to-the-iam-method-is-rejected-missing-authentication-token.

Cause 2 of 2: the same stage fronting an AWS_PROXY (Lambda proxy) integration over a DynamoDB-backed handler never returns either.
AWS: recorded behavior only; a stage with a Lambda proxy integration answers every request — 200 with the handler's body for a granted caller, the handler's 500 error body when the backend role lacks the DynamoDB read, and a 500 at the stage once the lambda:InvokeFunction permission is removed.
Emulator: all four checks report exactly 'Error: Check timed out after 60.0s'; no response is produced at any point of the proxy path.
Why: a client of a proxy-integrated API cannot distinguish success, a backend failure and a missing invoke permission — it simply hangs.
Fix: the execute-api data-plane path for AWS_PROXY integrations, including the integration invoke and the error-body mapping back to the stage response.
Traces: apigateway__dynamodb__iam__lambda.agreement-rewritten-in-dynamodb-is-served-on-the-next-signed-get, apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body, apigateway__dynamodb__iam__lambda.granted-caller-reads-the-seeded-agreement-verbatim-through-the-lambda-proxy, apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage.

## Evidence
All 18 failing checks report the identical detail: 'Error: Check timed out after 60.0s'. No HTTP status line, no headers and no body are ever recorded from the deployed stage invoke URL, for signed, unsigned and mis-signed requests alike, and for both plain and Lambda-proxy integrations.

## Steps to Reproduce
1. Create a REST API with a resource and a method whose authorizationType is AWS_IAM; create a deployment and a stage.
2. Send a SigV4-signed request to https://<api-id>.execute-api.<region>.<host>/<stage>/<path> and observe that no response arrives within 60 s (repeat unsigned and with a wrong secret).
3. Repeat with the method wired to an AWS_PROXY Lambda integration.
4. Or replay the 18 check ids in trace_ids.
