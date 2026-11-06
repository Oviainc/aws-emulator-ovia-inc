Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-role-positive lambda__sesv2.cfn-lambda-sesv2-trust-policy-positive lambda__sesv2.cli-lambda-sesv2-missing-policy-negative lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive lambda__sesv2.lambda-sesv2-create-email-identity-missing-permission-negative lambda__sesv2.lambda-sesv2-create-email-identity-positive lambda__sesv2.lambda-sesv2-delete-email-identity-positive lambda__sesv2.lambda-sesv2-get-account-positive lambda__sesv2.lambda-sesv2-get-email-identity-missing-permission-negative lambda__sesv2.lambda-sesv2-get-email-identity-positive lambda__sesv2.lambda-sesv2-list-email-identities-positive lambda__sesv2.lambda-sesv2-put-configuration-set-positive lambda__sesv2.lambda-sesv2-put-mail-from-domain-missing-permission-negative lambda__sesv2.lambda-sesv2-put-suppression-attributes-missing-permission-negative lambda__sesv2.lambda-sesv2-send-email-response-format lambda__sesv2.lambda-sesv2-tag-resource-missing-permission-negative lambda__sesv2.lambda-wrong-trust-policy-sesv2-negative
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-positive|run#9|AssertionError|assert payload.get("ok") is True, f"send_email did not succeed: {payload}"
Issue-Key: lambda__sesv2.cfn-lambda-sesv2-trust-policy-positive|run#19|AssertionError|assert payload.get("ok") is True, f"send_email did not succeed: {payload}"
Issue-Key: lambda__sesv2.cli-lambda-sesv2-missing-policy-negative|run#4|AssertionError|assert broken_result["body"]["errorCode"] == "AccessDeniedException", (
Issue-Key: lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive|run#13|AssertionError|assert baseline["body"]["errorCode"] == "AccessDeniedException", (
Issue-Key: lambda__sesv2.lambda-sesv2-create-email-identity-missing-permission-negative|run#10|AssertionError|assert bad_body["error_code"] == "AccessDeniedException", bad_body
Issue-Key: lambda__sesv2.lambda-sesv2-create-email-identity-positive|run#6|AssertionError|assert body["success"] is True, body
Issue-Key: lambda__sesv2.lambda-sesv2-delete-email-identity-positive|run#8|AssertionError|assert body["success"] is True, body
Issue-Key: lambda__sesv2.lambda-sesv2-get-account-positive|run#4|AssertionError|assert body["success"] is True, body
Issue-Key: lambda__sesv2.lambda-sesv2-get-email-identity-missing-permission-negative|run#11|AssertionError|assert bad_body["error_code"] == "AccessDeniedException", bad_body
Issue-Key: lambda__sesv2.lambda-sesv2-get-email-identity-positive|run#8|AssertionError|assert payload["ok"] is True, f"unexpected failure: {payload}"
Issue-Key: lambda__sesv2.lambda-sesv2-list-email-identities-positive|run#8|AssertionError|assert body["success"] is True, body
Issue-Key: lambda__sesv2.lambda-sesv2-put-configuration-set-positive|run#7|AssertionError|assert create_body["success"] is True, create_body
Issue-Key: lambda__sesv2.lambda-sesv2-put-mail-from-domain-missing-permission-negative|run#9|AssertionError|assert control_payload["ok"] is True, f"positive control unexpectedly failed: {control_payload}"
Issue-Key: lambda__sesv2.lambda-sesv2-put-suppression-attributes-missing-permission-negative|run#10|AssertionError|assert bad_body["error_code"] == "AccessDeniedException", (
Issue-Key: lambda__sesv2.lambda-sesv2-send-email-response-format|run#5|AssertionError|assert payload["ok"] is True, f"unexpected failure: {payload}"
Issue-Key: lambda__sesv2.lambda-sesv2-tag-resource-missing-permission-negative|run#13|AssertionError|assert bad_body["error_code"] == "AccessDeniedException", bad_body
Issue-Key: lambda__sesv2.lambda-wrong-trust-policy-sesv2-negative|run#20|AssertionError|assert ok_body["success"] is True, ok_body
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# lambda__sesv2: boto3.client('sesv2') and boto3.client('sts') cannot be built inside a Lambda, and the failure is reported with no error code

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One root cause behind 17 failing checks.

Cause 1 of 1: the in-process Lambda Python runtime's bundled AWS SDK ships a four-service data catalog (`sqs, events, eventbridge, dynamodb`), so a handler's `boto3.client('sesv2')` (and `boto3.client('sts')`) raises before any request is sent — and in several checks the resulting failure is surfaced with `error_code: None`, i.e. no error code at all.
AWS: a handler that builds an `sesv2` client can call SendEmail, CreateEmailIdentity, GetEmailIdentity, DeleteEmailIdentity, ListEmailIdentities, GetAccount, PutConfigurationSet*, PutEmailIdentityMailFromAttributes, PutAccountSuppressionAttributes and TagResource; each returns its modelled 200 output. With an execution role that does not grant the action, the call is refused with HTTP 403 and error code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: ses:<Action> on resource: <identity-or-configset-arn> because no identity-based policy allows the ses:<Action> action` — fixed text whose account, role name, function name, action and resource ARN come from the request and the role. A failure always carries a real AWS error code; `UnknownServiceError` is a client-side botocore error AWS never emits, and `None` is not a code. Model: SES v2 is present in this repo (services/service-com-amazonaws-sesv2-simpleemailservice-v2), as is STS (services/service-com-amazonaws-sts-awssecuritytokenservicev20110615), so clients for both are constructible on the wire.
Emulator: every one of these handlers fails with errorType `UnknownServiceError`, errorMessage `Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb` (or `'sts'`), returned as FunctionError `Unhandled` on a 200 invoke. Several checks additionally observe `{'success': False, 'error_code': None, ...}` — the error code is dropped entirely on the way back to the caller.
Why: no function can send mail or manage an SES identity, suppression list, contact list or configuration set — the entire SESv2-from-Lambda surface is dead, including the CloudFormation- and CLI-deployed variants. And because every refusal collapses to one client-construction error (or to no code at all), an application cannot distinguish a missing permission (AccessDeniedException), an unverified sender (MessageRejected) or a bad identity (NotFoundException); none of the negative checks prove anything about authorization, and a role deliberately scoped down looks the same as one that is correct.
Fix: the in-process Lambda Python runtime's bundled AWS SDK — its service-model data catalog must cover every service the emulator serves (sesv2 and sts among them) and the client factory must stop refusing them — plus the error path that must carry the service's own error code and message back to the invoker instead of `None`. A related open issue, 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md, covers other checks of this service; it does not carry these 17 replay keys.
Traces: the 17 failing checks in trace_ids, under `### lambda__sesv2`.

## Evidence
Expected (AWS): SendEmail/CreateEmailIdentity/GetAccount etc. from the handler return their modelled 200 output; with no ses grant the call is refused with HTTP 403 `AccessDeniedException` and the `... is not authorized to perform: ses:<Action> on resource: <arn> because no identity-based policy allows the ses:<Action> action` message.

Actual (emulator), representative lines:
- cfn-deploy-lambda-sesv2-role-positive -> send_email did not succeed: {'ok': False, 'error': 'UnknownServiceError', 'code': 'UnknownServiceError'}
- cfn-lambda-sesv2-trust-policy-positive -> same
- lambda-sesv2-create-email-identity-positive / -missing-permission-negative, lambda-sesv2-get-email-identity-positive / -missing-permission-negative, lambda-sesv2-delete-email-identity-positive, lambda-sesv2-get-account-positive, lambda-sesv2-list-email-identities-positive, lambda-sesv2-put-configuration-set-positive, lambda-sesv2-put-mail-from-domain-missing-permission-negative, lambda-sesv2-send-email-response-format -> {'success': False, 'error_code': None, 'error_message': "Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb"}
- cli-lambda-sesv2-missing-policy-negative -> expected AccessDeniedException from the sesv2 call inside the broken-role function, got 'UnknownServiceError'
- cli-lambda-sesv2-update-role-policy-positive -> expected AccessDeniedException with no sesv2 permission, got errorCode='UnknownServiceError'
- lambda-wrong-trust-policy-sesv2-negative, lambda-sesv2-put-suppression-attributes-missing-permission-negative, lambda-sesv2-tag-resource-missing-permission-negative -> same UnknownServiceError family
- iam-role-trust-lambda-service-positive family also shows `Unknown service: 'sts'`

## Steps to Reproduce
1. Create a Lambda execution role granting ses:* .
2. Create a python Lambda whose handler does `boto3.client('sesv2').send_email(FromEmailAddress=..., Destination=..., Content=...)` and returns the MessageId.
3. Invoke it. Expected: 200 with a MessageId. Observed: FunctionError Unhandled, `Unknown service: 'sesv2'. Valid service names are: sqs, events, eventbridge, dynamodb` — and in several variants the invoker sees error_code None.
4. Repeat with a role granting no ses action. Expected AccessDeniedException; observed the same UnknownServiceError.
