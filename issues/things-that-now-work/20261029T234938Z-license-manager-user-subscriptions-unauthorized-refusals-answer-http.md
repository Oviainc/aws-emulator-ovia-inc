Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance license-manager-user-subscriptions-deregister-identity-provider-not-found license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp license-manager-user-subscriptions-update-identity-provider-settings-unregistered
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: license-manager-user-subscriptions|AssociateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|AssociateUser|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|DisassociateUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DisassociateUser|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|DisassociateUser|4xx->4xx|$.message
Issue-Key: license-manager-user-subscriptions|DeregisterIdentityProvider|4xx->4xx|$.message
Issue-Key: license-manager-user-subscriptions|DeregisterIdentityProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeregisterIdentityProvider|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|DeleteLicenseServerEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|DeleteLicenseServerEndpoint|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|ListUserAssociations|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|ListUserAssociations|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|ListProductSubscriptions|4xx->4xx|$.message
Issue-Key: license-manager-user-subscriptions|ListProductSubscriptions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|ListProductSubscriptions|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|StartProductSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|StartProductSubscription|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|StopProductSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|StopProductSubscription|4xx->4xx|status
Issue-Key: license-manager-user-subscriptions|UpdateIdentityProviderSettings|4xx->4xx|$.message
Issue-Key: license-manager-user-subscriptions|UpdateIdentityProviderSettings|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: license-manager-user-subscriptions|UpdateIdentityProviderSettings|4xx->4xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# license-manager-user-subscriptions Unauthorized refusals answer HTTP 400, and four paths never reach the gate

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateUser

## Description
SHARED FAULT (10 causes, all in the license-manager-user-subscriptions crate): every refusal this service answers with UnauthorizedException is served as HTTP 400 instead of HTTP 401, and on four of the ten paths the Unauthorized refusal is never produced at all because a locally-authored ValidationException preempts it.

BACKGROUND USED FOR EVERY CAUSE. Smithy (services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions): the service is restJson1 and binds these operations to AccessDeniedException, ConflictException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException and ValidationException only (UpdateIdentityProviderSettings: AccessDeniedException/ThrottlingException/ValidationException; ListLicenseServerEndpoints has no ResourceNotFoundException). AccessDeniedException and ValidationException both carry httpError 400 (defaulted), ResourceNotFoundException 404. UnauthorizedException is NOT in the model at all, so for the error class and its status this is RECORDED BEHAVIOR ONLY: the corpus shows the real service answering HTTP 401 with header x-amzn-errortype: UnauthorizedException and body {"message": "Unauthorized. Please check the request and access policy and try again."}. That message is entirely fixed text — it has no variable parts taken from the request or from state, so it must be reproduced verbatim, with no resource name, id or ARN interpolated into it.

Header note for the causes below: several recorded responses carry x-amzn-errortype twice, which is why the comparator reports the expected value as 'UnauthorizedException, UnauthorizedException' against the emulator's single 'UnauthorizedException'. The requirement is the header VALUE UnauthorizedException; the duplication is an artifact of the recording and must not be emulated by emitting the header twice. Where the detail says '<shaped-not-asserted>' the header was not asserted at all.

Cause 1 of 10 - AssociateUser against an instance the caller is not authorized for.
AWS: recorded behavior only - HTTP 401, x-amzn-errortype UnauthorizedException, message "Unauthorized. Please check the request and access policy and try again.".
Emulator: the same error type and message, but HTTP 400.
Why: SDKs and HTTP clients classify 401 as an authentication/authorization outcome (credential refresh, auth-failure branch) and 400 as a malformed-request outcome. A client that branches on status - or that reports "bad request" to its user - takes the wrong branch for every denial this service issues.
Fix: the crate's error-to-HTTP mapping / serializer for the service's UnauthorizedException variant, which must map to 401 rather than inheriting the 400 default used by the modeled client errors.
Traces: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#1.

Cause 2 of 10 - DisassociateUser with Username/InstanceId for an unauthorized instance.
AWS: recorded behavior only - HTTP 401, UnauthorizedException, same fixed message.
Emulator: HTTP 400 with UnauthorizedException.
Why: as cause 1.
Fix: same error-to-HTTP mapping; the DisassociateUser handler's refusal arm itself is correct.
Traces: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#2.

Cause 3 of 10 - DisassociateUser addressed by InstanceUserArn: input validation preempts the authorization gate.
AWS: recorded behavior only - for an InstanceUserArn naming an instance/user the caller is not authorized for, the service answers HTTP 401 UnauthorizedException with the fixed message, EVEN THOUGH the ARN does not satisfy the modeled Arn pattern (Smithy: DisassociateUserRequest.InstanceUserArn @pattern ^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}$ - a user-association ARN carries two slashes in its resource part). On this operation the authorization decision is taken before the ARN is pattern-checked.
Emulator: HTTP 400, x-amzn-errortype ValidationException, with the generic constraint sentence "1 validation error detected: Value '<arn>' at 'instanceUserArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" - a different status, a different error class and a different message.
Why: a client that handles the denial (re-authorize, pick another instance) instead sees a malformed-input error it cannot act on, and never learns the call was refused for authorization.
Fix: the DisassociateUser handler's gate ordering - run the instance/user authorization check before the ARN pattern validation on the ARN-addressed path.
Traces: license-manager-user-subscriptions-associate-disassociate-user-unknown-instance#task#3.

Cause 4 of 10 - DeregisterIdentityProvider for an identity provider that is not registered.
AWS: recorded behavior only - HTTP 401 UnauthorizedException with the fixed message "Unauthorized. Please check the request and access policy and try again.".
Emulator: HTTP 400, x-amzn-errortype ValidationException, message "Identity provider is not valid" - an emulator-authored sentence that appears nowhere in the model or the recording.
Why: the caller cannot distinguish a refusal from a bad parameter, and the invented sentence breaks any message-based handling.
Fix: the DeregisterIdentityProvider handler's unregistered-provider arm - raise the service's UnauthorizedException (401, fixed message) instead of a locally worded ValidationException.
Traces: license-manager-user-subscriptions-deregister-identity-provider-not-found#task#2.

Cause 5 of 10 - DeleteLicenseServerEndpoint refusal in the endpoint-lifecycle error path.
AWS: recorded behavior only - HTTP 401 with UnauthorizedException (the errortype header is not asserted on this step).
Emulator: HTTP 400 with UnauthorizedException.
Why: as cause 1.
Fix: same error-to-HTTP mapping for UnauthorizedException.
Traces: license-manager-user-subscriptions-license-server-endpoint-lifecycle-errors#task#5.

Cause 6 of 10 - ListUserAssociations for an unregistered identity provider.
AWS: recorded behavior only - HTTP 401, UnauthorizedException, fixed message.
Emulator: HTTP 400 with UnauthorizedException.
Why: as cause 1.
Fix: same error-to-HTTP mapping.
Traces: license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp#task#1.

Cause 7 of 10 - ListProductSubscriptions for an unregistered identity provider.
AWS: recorded behavior only - HTTP 401, UnauthorizedException, message "Unauthorized. Please check the request and access policy and try again.".
Emulator: HTTP 400, ValidationException, message "Identity provider is not valid".
Why: a read path that AWS refuses as unauthorized is reported as a malformed request, so a client retries with different parameters instead of fixing access.
Fix: the ListProductSubscriptions handler's unregistered-provider arm (same fault as cause 4).
Traces: license-manager-user-subscriptions-list-associations-and-subscriptions-unregistered-idp#task#2.

Cause 8 of 10 - StartProductSubscription for an unregistered identity provider.
AWS: recorded behavior only - HTTP 401, UnauthorizedException, fixed message.
Emulator: HTTP 400 with UnauthorizedException.
Why: as cause 1.
Fix: same error-to-HTTP mapping.
Traces: license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp#task#1.

Cause 9 of 10 - StopProductSubscription for an unregistered identity provider.
AWS: recorded behavior only - HTTP 401, UnauthorizedException, fixed message.
Emulator: HTTP 400 with UnauthorizedException.
Why: as cause 1.
Fix: same error-to-HTTP mapping.
Traces: license-manager-user-subscriptions-start-stop-product-subscription-unregistered-idp#task#2.

Cause 10 of 10 - UpdateIdentityProviderSettings for an identity provider that is not registered.
AWS: recorded behavior only - HTTP 401, UnauthorizedException, fixed message. (Smithy binds this operation to AccessDeniedException/ThrottlingException/ValidationException only, so the 401 class is recorded, not modeled.)
Emulator: HTTP 400, ValidationException, message "Identity provider is not valid".
Why: as cause 7, on a write path - the caller believes its Settings payload was malformed when the call was actually refused.
Fix: the UpdateIdentityProviderSettings handler's unregistered-provider arm (same fault as causes 4 and 7).
Traces: license-manager-user-subscriptions-update-identity-provider-settings-unregistered#task#1.

A fixer must close all TEN causes: the shared 401 mapping (causes 1, 2, 5, 6, 8, 9), the three ValidationException-instead-of-Unauthorized arms (causes 4, 7, 10) and the DisassociateUser gate-ordering cause (3). No open issue carries these replay keys; the nearest open ones (20261024T010614Z-license-manager-user-subscriptions-registeridentityprovider-wrong-behavior.md, 20261029T165944Z-license-manager-user-subscriptions-stopproductsubscription-error-response.md) are about other paths and do not cover any of them.

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 37 steps, 23 passed, 14 mismatched, score 62.2%). Per cause, expected vs actual:
1. associate-disassociate-user-unknown-instance#task#1 AssociateUser: status expected 401, got 400; x-amzn-errortype expected 'UnauthorizedException, UnauthorizedException' (recorded twice), got 'UnauthorizedException'.
2. ...#task#2 DisassociateUser: status expected 401, got 400; same header note.
3. ...#task#3 DisassociateUser: status expected 401, got 400; $.message expected 'Unauthorized. Please check the request and access policy and try again.', got "1 validation error detected: Value 'arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:user-association/i-sim2realprobelice/sim2real-probe-...-user-215aff7c' at 'instanceUserArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...$"; errortype expected UnauthorizedException, got ValidationException.
4. deregister-identity-provider-not-found#task#2 DeregisterIdentityProvider: status expected 401, got 400; $.message expected 'Unauthorized. Please check the request and access policy and try again.', got 'Identity provider is not valid'; errortype got ValidationException.
5. license-server-endpoint-lifecycle-errors#task#5 DeleteLicenseServerEndpoint: status expected 401, got 400; errortype not asserted, emulator sent UnauthorizedException.
6. list-associations-and-subscriptions-unregistered-idp#task#1 ListUserAssociations: status expected 401, got 400.
7. list-associations-and-subscriptions-unregistered-idp#task#2 ListProductSubscriptions: status expected 401, got 400; $.message expected 'Unauthorized. Please check the request and access policy and try again.', got 'Identity provider is not valid'; errortype got ValidationException.
8. start-stop-product-subscription-unregistered-idp#task#1 StartProductSubscription: status expected 401, got 400.
9. start-stop-product-subscription-unregistered-idp#task#2 StopProductSubscription: status expected 401, got 400.
10. update-identity-provider-settings-unregistered#task#1 UpdateIdentityProviderSettings: status expected 401, got 400; $.message expected 'Unauthorized. Please check the request and access policy and try again.', got 'Identity provider is not valid'; errortype got ValidationException.
Model evidence: the service model declares no UnauthorizedException; AccessDeniedException and ValidationException are httpError 400 (defaulted), ResourceNotFoundException 404; DisassociateUserRequest.InstanceUserArn carries the anchored Arn @pattern quoted above.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative: fixture license-manager-user-subscriptions-associate-disassociate-user-unknown-instance, step task#1 - POST /user/AssociateUser naming an EC2 instance the caller is not authorized for. Observe HTTP 400; AWS recorded HTTP 401 with x-amzn-errortype UnauthorizedException and message 'Unauthorized. Please check the request and access policy and try again.'.
4. Repeat for the other nine bundled steps listed in trace_ids: the user-plane refusals (task#2 of the same fixture, license-server-endpoint-lifecycle-errors#task#5, list-associations-and-subscriptions-unregistered-idp#task#1, start-stop-product-subscription-unregistered-idp#task#1 and #task#2) all return 400 instead of 401; deregister-identity-provider-not-found#task#2, list-associations-and-subscriptions-unregistered-idp#task#2 and update-identity-provider-settings-unregistered#task#1 return 400 ValidationException 'Identity provider is not valid' instead of 401 UnauthorizedException; associate-disassociate-user-unknown-instance#task#3 (DisassociateUser by InstanceUserArn) returns the generic 400 pattern-validation sentence instead of the 401 refusal.
