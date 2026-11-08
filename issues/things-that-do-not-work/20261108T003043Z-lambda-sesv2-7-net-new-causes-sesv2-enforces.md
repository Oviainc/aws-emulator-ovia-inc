Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.lambda-sesv2-account-suppression-list-permission-negative lambda__sesv2.lambda-sesv2-configuration-set-missing-permission-negative lambda__sesv2.lambda-sesv2-create-contact-list-missing-permission-negative lambda__sesv2.lambda-sesv2-create-contact-missing-permission-negative lambda__sesv2.lambda-sesv2-create-email-identity-missing-permission-negative lambda__sesv2.lambda-sesv2-delete-contact-missing-permission-negative lambda__sesv2.lambda-sesv2-get-email-identity-missing-permission-negative lambda__sesv2.lambda-sesv2-get-identity-verification-missing-permission-negative lambda__sesv2.lambda-sesv2-list-email-identities-missing-permission-negative lambda__sesv2.lambda-sesv2-list-suppressed-destinations-missing-permission-negative lambda__sesv2.lambda-sesv2-put-mail-from-domain-missing-permission-negative lambda__sesv2.lambda-sesv2-tag-resource-missing-permission-negative lambda__sesv2.lambda-sesv2-update-configuration-set-event-destination-missing-permission-negative lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive lambda__sesv2.cfn-deploy-lambda-sesv2-missing-iam-policy-negative lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative lambda__sesv2.cfn-stack-update-lambda-sesv2-add-permission-positive lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive lambda__sesv2.cfn-deploy-lambda-sesv2-configuration-set-positive lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative lambda__sesv2.lambda-sesv2-get-email-identity-positive lambda__sesv2.lambda-sesv2-put-suppressed-destination-missing-permission-negative lambda__sesv2.lambda-sesv2-send-templated-email-positive lambda__sesv2.lambda-sesv2-wrong-region-endpoint-negative
Issue-Key: lambda__sesv2.lambda-sesv2-account-suppression-list-permission-negative|run#11|AssertionError|assert body["ok"] is False, f"role lacks ses:ListSuppressedDestinations, expected a refusal: {body}"
Issue-Key: lambda__sesv2.lambda-sesv2-configuration-set-missing-permission-negative|run#8|AssertionError|assert body["success"] is False, body
Issue-Key: lambda__sesv2.lambda-sesv2-create-contact-list-missing-permission-negative|run#8|AssertionError|assert body["success"] is False, body
Issue-Key: lambda__sesv2.lambda-sesv2-create-contact-missing-permission-negative|run#11|AssertionError|assert body["ok"] is False, f"expected create_contact to be denied, got: {body}"
Issue-Key: lambda__sesv2.lambda-sesv2-create-email-identity-missing-permission-negative|run#9|AssertionError|assert bad_body["success"] is False, bad_body
Issue-Key: lambda__sesv2.lambda-sesv2-delete-contact-missing-permission-negative|run#13|AssertionError|assert body["success"] is False, body
Issue-Key: lambda__sesv2.lambda-sesv2-get-email-identity-missing-permission-negative|run#10|AssertionError|assert bad_body["success"] is False, bad_body
Issue-Key: lambda__sesv2.lambda-sesv2-get-identity-verification-missing-permission-negative|run#12|AssertionError|assert body["ok"] is False, f"expected the ungranted role to be denied; got {body}"
Issue-Key: lambda__sesv2.lambda-sesv2-list-email-identities-missing-permission-negative|run#8|AssertionError|assert denied_result["ok"] is False, f"expected denied invoke to fail, got {denied_result}"
Issue-Key: lambda__sesv2.lambda-sesv2-list-suppressed-destinations-missing-permission-negative|run#9|AssertionError|assert body["ok"] is False, f"expected denied role's list call to fail, got: {body}"
Issue-Key: lambda__sesv2.lambda-sesv2-put-mail-from-domain-missing-permission-negative|run#17|AssertionError|assert restricted_payload["ok"] is False, f"put should have been denied: {restricted_payload}"
Issue-Key: lambda__sesv2.lambda-sesv2-tag-resource-missing-permission-negative|run#12|AssertionError|assert bad_body["success"] is False, bad_body
Issue-Key: lambda__sesv2.lambda-sesv2-update-configuration-set-event-destination-missing-permission-negative|run#12|AssertionError|assert body["ok"] is False, f"expected the unprivileged call to be denied, got: {body}"
Issue-Key: lambda__sesv2.cli-lambda-sesv2-update-role-policy-positive|run#12|AssertionError|assert baseline["body"]["ok"] is False, (
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-iam-policy-negative|run#8|AssertionError|assert payload["errorCode"] == "AccessDeniedException", payload
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-missing-policy-negative|run#11|AssertionError|assert denied_body["errorCode"] == "AccessDeniedException", (
Issue-Key: lambda__sesv2.cfn-stack-update-lambda-sesv2-add-permission-positive|run#6|AssertionError|assert before["code"] == "AccessDeniedException", f"unexpected code before update: {before}"
Issue-Key: lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive|run#10|AssertionError|assert denied_body["errorCode"] == "AccessDeniedException", (
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-configuration-set-positive|run#10|AssertionError|assert payload["outcome"] == "read", payload
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution|run#6|AssertionError|assert body["ok"] is True, f"expected GetConfigurationSet to succeed, got {body}"
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative|run#14|AssertionError|assert valid_body["ok"] is True, f"expected success against the real identity, got {valid_body}"
Issue-Key: lambda__sesv2.lambda-sesv2-get-email-identity-positive|run#11|AssertionError|assert result["VerifiedForSendingStatus"] is False, (
Issue-Key: lambda__sesv2.lambda-sesv2-put-suppressed-destination-missing-permission-negative|run#12|AssertionError|assert body["error_code"] == "AccessDeniedException", body
Issue-Key: lambda__sesv2.lambda-sesv2-send-templated-email-positive|run#16|AssertionError|assert body["ok"] is True, f"send_email call failed: {body}"
Issue-Key: lambda__sesv2.lambda-sesv2-wrong-region-endpoint-negative|run#12|AssertionError|assert body["sent"] is False, (
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# lambda__sesv2: 7 net-new causes — SESv2 enforces no IAM authorization, SendEmail rejects verified identities, CFN-created SESv2 resources read back NotFound, verification status is always true, suppression writes hit a sandbox gate, templated sends are NotImplemented, and cross-region sends are not isolated

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Seven distinct root causes behind the 25 failing `### lambda__sesv2` checks that the suite marks neither ALREADY FILED nor OWNED. Open issues 20261106T151148Z-lambda-sesv2-boto3-client-sesv2-and-boto3-client.md, 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md and 20261029T202037Z-lambda-sesv2-3-net-new-root-causes-a.md own other checks in this service and cover none of the seven below.

Cause 1 of 7: SESv2 performs no caller authorization — a role with no `ses:*` statement succeeds on every operation.
AWS: an unauthorized principal is refused `403 AccessDeniedException` with the implicit-deny sentence `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: ses:<Action> on resource: <resource-arn> because no identity-based policy allows the ses:<Action> action` — fixed text, with principal, action and resource taken from the request and IAM state. This applies to `ListSuppressedDestinations`, `GetConfigurationSet`, `CreateContactList`, `CreateContact`, `CreateEmailIdentity`, `DeleteContact`, `GetEmailIdentity`, `ListEmailIdentities`, `PutEmailIdentityMailFromAttributes`, `TagResource`, `UpdateConfigurationSetEventDestination` and `SendEmail` alike. Smithy: none of these operations models a success for an unauthorized caller; `AccessDeniedException` is the service-common IAM refusal.
Emulator: each ungranted call returns a success document — `{'success': True}`, `{'ok': True}`, `{'ok': True, 'addresses': []}`, `{'ok': True, 'identities': [...], 'found': True}` — and the SendEmail arms get as far as a MessageRejected (Cause 2) instead of being denied.
Why: least-privilege policies on SES are not enforced, so a function can create identities, read suppression lists, tag resources and send mail its role never granted; every negative authorization test gives a false pass.
Fix: the sesv2 operation handlers' authorization step must evaluate the caller's identity policy and refuse with `AccessDeniedException` before performing the operation.
Traces: lambda-sesv2-account-suppression-list-permission-negative, lambda-sesv2-configuration-set-missing-permission-negative, lambda-sesv2-create-contact-list-missing-permission-negative, lambda-sesv2-create-contact-missing-permission-negative, lambda-sesv2-create-email-identity-missing-permission-negative, lambda-sesv2-delete-contact-missing-permission-negative, lambda-sesv2-get-email-identity-missing-permission-negative, lambda-sesv2-get-identity-verification-missing-permission-negative, lambda-sesv2-list-email-identities-missing-permission-negative, lambda-sesv2-list-suppressed-destinations-missing-permission-negative, lambda-sesv2-put-mail-from-domain-missing-permission-negative, lambda-sesv2-tag-resource-missing-permission-negative, lambda-sesv2-update-configuration-set-event-destination-missing-permission-negative, cli-lambda-sesv2-update-role-policy-positive.

Cause 2 of 7: SendEmail refuses an identity the account has created/verified with `MessageRejected: Email address is not verified...`.
AWS: once the From address's identity exists and is verified, `SendEmail` returns `200` with a `MessageId`. `MessageRejected` with the sentence `Email address is not verified. The following identities failed the check in region <REGION>: <address>` (fixed text; the region and the address list come from the request and state) is correct ONLY for an unverified identity. Smithy: `SendEmail` models `MessageRejected` (400) and returns `MessageId` on success.
Emulator: addresses created in the same stack/run are still rejected with that sentence, so the positive arms cannot send and the negative arms report `MessageRejected` where `AccessDeniedException` was due.
Why: no SES send path works for an identity the client just verified; the refusal blames verification state that the account does in fact hold.
Fix: `SendEmail`'s From-identity verification check must consult the stored identity verification state written by CreateEmailIdentity / the CFN `AWS::SESv2::EmailIdentity` provisioner.
Traces: cfn-deploy-lambda-sesv2-missing-iam-policy-negative, cfn-deploy-lambda-sesv2-missing-policy-negative, cfn-stack-update-lambda-sesv2-add-permission-positive, cfn-stack-update-lambda-sesv2-permission-add-positive.

Cause 3 of 7: SESv2 resources created by CloudFormation are not addressable through the SESv2 API.
AWS: a configuration set created as `AWS::SESv2::ConfigurationSet` is readable by `GetConfigurationSet` by name, and an identity created as `AWS::SESv2::EmailIdentity` is addressable by `PutEmailIdentityDkimAttributes`; `NotFoundException` (404) is reserved for a name that does not exist.
Emulator: both answer `NotFoundException` — `Configuration set <name> does not exist.` and `Email identity <name> does not exist.` — for resources CloudFormation reported as created, i.e. the CFN provisioner's writes are not visible to the SESv2 service state.
Why: a stack that reports CREATE_COMPLETE leaves nothing a client can use; every SESv2-backed deployment is a phantom.
Fix: the CloudFormation resource provisioners for `AWS::SESv2::ConfigurationSet` / `AWS::SESv2::EmailIdentity` must write into the same sesv2 service state the API reads.
Traces: cfn-deploy-lambda-sesv2-configuration-set-positive, cfn-deploy-lambda-sesv2-env-configset-arn-resolution, cfn-deploy-lambda-sesv2-wrong-arn-negative.

Cause 4 of 7: GetEmailIdentity reports `VerifiedForSendingStatus: true` for a never-verified domain identity.
AWS: `VerifiedForSendingStatus` is false until the identity's verification completes (for a domain, until its DKIM/TXT records validate). Smithy: `GetEmailIdentity` output member `VerifiedForSendingStatus` targets `Enabled` (boolean, default false), alongside `VerificationStatus`.
Emulator: a domain identity that was never verified reads back `VerifiedForSendingStatus: true` (and `verified_for_sending: True` from inside a handler).
Why: a client gating sends on the identity's own verification flag is told an unverified domain is ready; the flag carries no information.
Fix: the `GetEmailIdentity` handler must derive `VerifiedForSendingStatus` from the identity's stored verification state instead of defaulting it true.
Traces: lambda-sesv2-get-email-identity-positive.

Cause 5 of 7: PutSuppressedDestination is refused with a sandbox gate.
AWS: `PutSuppressedDestination` adds an address to the account-level suppression list and returns `200` with an empty body; its modeled errors are `BadRequestException` (400), `NotFoundException` (404) and `TooManyRequestsException` (429). Sandbox status gates *sending*, not suppression-list writes, and `Your account is still in the sandbox.` is not AWS's wording for any PutSuppressedDestination refusal.
Emulator: the call is refused `BadRequestException` with message `Your account is still in the sandbox.`
Why: the account suppression list cannot be written at all, so suppression workflows (and the IAM negative that should have been an AccessDeniedException) are unreachable.
Fix: the `PutSuppressedDestination` handler must not apply the sending-sandbox gate; it should write the suppression entry and return 200.
Traces: lambda-sesv2-put-suppressed-destination-missing-permission-negative.

Cause 6 of 7: SendEmail with a template answers `NotImplemented`.
AWS: `SendEmail` with `Content.Template` renders the named template and returns `200` with a `MessageId`; `NotImplemented` is not a modeled error for `SendEmail` and 501 is not a status it can answer.
Emulator: the in-handler send returns `{'ok': False, 'error_code': 'NotImplemented'}`.
Why: templated sending — the whole SES template feature — is unusable, and the error code is unmodeled so no SDK can classify it.
Fix: implement the templated-content branch of the `SendEmail` handler. Note the related open issue 20261027T161051Z-sesv2-sns-sqs-sesv2-sendemail-with-content-template-is-refused-501.md records the same refusal for a different service's checks; it does not carry this check's replay key.
Traces: lambda-sesv2-send-templated-email-positive.

Cause 7 of 7: an identity verified in one region is accepted for a send against another region's endpoint.
AWS: SES identities are per-region. Sending from the `us-east-1` endpoint with a From address verified only in `us-west-2` is refused `MessageRejected` with `Email address is not verified. The following identities failed the check in region US-EAST-1: <address>` (fixed text; region and address from the request and state).
Emulator: the cross-region send succeeds — `{'sent': True, 'message_id': ...}` — so identity state is not region-scoped.
Why: a client cannot rely on regional isolation of verified identities; a send that AWS refuses is accepted, hiding a misconfigured region in deployment.
Fix: the sesv2 identity store and `SendEmail`'s verification check must be scoped by region.
Traces: lambda-sesv2-wrong-region-endpoint-negative.

## Evidence
Cause 1 — e.g. `AssertionError: role lacks ses:ListSuppressedDestinations, expected a refusal: {'ok': True, 'addresses': []}`; `AssertionError: {'success': True}` (configuration-set, create-contact-list, create-email-identity, delete-contact, get-email-identity, tag-resource); `AssertionError: expected create_contact to be denied, got: {'ok': True}`; `AssertionError: expected denied invoke to fail, got {'ok': True, 'identities': [...], 'found': True}`; `AssertionError: put should have been denied: {'ok': True, 'result': {}}`; `AssertionError: expected the unprivileged call to be denied, got: {'ok': True}`; `AssertionError: expected baseline denial, got: {'ok': True, 'messageId': '...'}`.

Cause 2 — `AssertionError: expected AccessDeniedException (no ses:SendEmail statement), got 'MessageRejected': {'ok': False, 'errorCode': 'MessageRejected', 'message': "An error occurred (MessageRejected) when calling the SendEmail operation: Email address is not verified. The following identities failed the check in region US-WEST-2: <address>"}`; `AssertionError: unexpected code before update: {'ok': False, 'code': 'MessageRejected', 'message': 'Email address is not verified. ...'}`.

Cause 3 — `AssertionError: {'outcome': 'error', 'errorCode': 'NotFoundException'}`; `AssertionError: expected GetConfigurationSet to succeed, got {'ok': False, 'errorCode': 'NotFoundException', 'message': '... Configuration set <name> does not exist.'}`; `AssertionError: expected success against the real identity, got {'ok': False, 'errorCode': 'NotFoundException', 'message': '... PutEmailIdentityDkimAttributes operation: Email identity <name> does not exist.'}`.

Cause 4 — `AssertionError: expected VerifiedForSendingStatus=False for a never-verified domain, got True`; `AssertionError: expected the ungranted role to be denied; got {'ok': True, 'verified_for_sending': True}`.

Cause 5 — `AssertionError: {'success': False, 'error_code': 'BadRequestException', 'message': 'Your account is still in the sandbox.'}`.

Cause 6 — `AssertionError: send_email call failed: {'ok': False, 'error_code': 'NotImplemented'}`.

Cause 7 — `AssertionError: expected send from us-east-1 (unverified) to be rejected, got {'sent': True, 'message_id': '...'}`.

## Steps to Reproduce
1. Cause 1: create a python Lambda whose role has no `ses:*` statement and whose handler calls each of GetConfigurationSet / CreateContactList / CreateContact / CreateEmailIdentity / DeleteContact / GetEmailIdentity / ListEmailIdentities / ListSuppressedDestinations / PutEmailIdentityMailFromAttributes / TagResource / UpdateConfigurationSetEventDestination; each must be refused AccessDeniedException.
2. Cause 2: `CreateEmailIdentity` for an address (or deploy `AWS::SESv2::EmailIdentity`), then `SendEmail` from it — it must return a MessageId, not MessageRejected.
3. Cause 3: deploy a stack with `AWS::SESv2::ConfigurationSet` and `AWS::SESv2::EmailIdentity`, then call `GetConfigurationSet` by name and `PutEmailIdentityDkimAttributes` on the identity — both must succeed, not 404.
4. Cause 4: `CreateEmailIdentity` for a domain and immediately `GetEmailIdentity` — `VerifiedForSendingStatus` must be false.
5. Cause 5: `PutSuppressedDestination` with EmailAddress and Reason=BOUNCE — it must return 200, not BadRequestException 'Your account is still in the sandbox.'
6. Cause 6: `CreateEmailTemplate`, then `SendEmail` with `Content.Template` — it must return a MessageId, not NotImplemented.
7. Cause 7: verify an identity in us-west-2 and `SendEmail` from it against the us-east-1 endpoint — it must be refused MessageRejected naming region US-EAST-1.
