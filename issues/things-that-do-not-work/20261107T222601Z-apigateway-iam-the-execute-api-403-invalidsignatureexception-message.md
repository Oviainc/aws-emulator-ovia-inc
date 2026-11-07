Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected|run#14|AssertionError|assert "/{}/widgets".format(_STAGE) in message, (
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# apigateway__iam: the execute-api 403 InvalidSignatureException message omits the canonical-string detail, so it never names the canonical request path

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the execute-api data plane's signature-mismatch refusal carries a truncated message that drops the canonical-string detail.
Failing check: apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected
  AWS: a request to a deployed REST stage signed with a genuine access key id and session token but the wrong secret key is refused 403 with x-amzn-ErrorType: InvalidSignatureException. Recorded behavior: the JSON body's `message` begins with the fixed sentence "The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method." and then continues with the canonical-string detail that echoes the canonical request AWS computed — which contains the canonical URI, i.e. the stage-qualified request path ("/<stage>/<resource>", here "/test/widgets"). The fixed text is the sentence above plus the canonical-string preamble; the variable parts are the HTTP method, the canonical URI/path, the canonical query string, the signed headers and the payload hash, all taken from the request being refused — never the literal values of one recording.
  Emulator: status 403 and x-amzn-ErrorType: InvalidSignatureException are correct, but the message is only "The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details." — the canonical-string detail is absent, so the message never echoes the canonical path "/test/widgets" of the request it refused. The check's assertion that the canonical path appears in the message fails.
  Why: SigV4 clients and SDK debug tooling diff the canonical string AWS reports against the one they signed to locate the mismatch (wrong path normalization, wrong query canonicalization, wrong signed-header set). With that detail stripped, a signing bug is indistinguishable from a wrong secret, and a caller cannot tell which path the gateway actually canonicalized.
  Fix: the execute-api data plane's SigV4 verification error serializer — the component that builds the InvalidSignatureException body must append the canonical-string detail computed from the incoming request, not a fixed generic sentence.
  Traces: apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected

Note: the correctly signed positive controls in the same check (200 with {"served": "GET"}) and the 403/x-amzn-ErrorType assertions did not fail, so only the message text diverges. No open issue covers this message; the nearest open issues are about JWT/authorizer signature handling (20261105T232928Z-apigateway-cognito-identity-cognito-idp-a-cognito-user.md) and do not cover the execute-api SigV4 refusal body.

## Evidence
Check apigateway__iam.xsvc-apigateway-3-iam-wrong-secret-signature-mismatch-rejected, GET on a deployed stage signed with a real access key id + session token and a same-length wrong secret:
  Expected: 403, header x-amzn-ErrorType: InvalidSignatureException, body message starting with "The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method." AND containing the canonical request path of the refused request — "/<stage>/widgets" (here "/test/widgets").
  Actual: 403 with the right error type, but message = 'The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details.' — no canonical-string detail, no path.
  AssertionError: signature-mismatch message does not echo the canonical path.

## Steps to Reproduce
1. CreateRestApi (REGIONAL) with a GET /widgets method, authorizationType=AWS_IAM, any integration that returns {"served": "GET"}; CreateDeployment(stageName='test').
2. Create an IAM role allowing execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<api-id>/test/GET/widgets and AssumeRole it via STS.
3. SigV4-sign GET https://<api-id>.execute-api.<region>.<host>/test/widgets with those credentials — expect 200 {"served": "GET"}.
4. Re-sign the same request keeping the genuine access key id and session token but replacing the secret key with a different same-length string.
5. Issue it: the response is 403 with x-amzn-ErrorType: InvalidSignatureException, but the JSON `message` is the generic sentence only and contains neither the canonical-string detail nor the request path "/test/widgets".
