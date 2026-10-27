Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging cognito-idp.update-user-pool-replica-reflects-region-config-change
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging|_error#17|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.update-user-pool-replica-reflects-region-config-change|run#22|AssertionError|assert initial_status == "INACTIVE", f"expected new replica to start INACTIVE, got {initial_status}"
Issue-Commit: 4c7fecaf2f3855c98f90c94a7d1b066468cbdd29

# cognito-idp: two causes — SetUserPoolMfaConfig does not gate Email MFA on the pool's feature tier, and a new user pool replica starts ACTIVE instead of INACTIVE

> _Note: this issue was identified against the code base at commit `4c7fecaf2f3855c98f90c94a7d1b066468cbdd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetUserPoolMfaConfig

## Description
Two independent net-new root causes in the cognito-idp emulator, both grounded in failing behavioral checks that the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 2: SetUserPoolMfaConfig accepts EmailMfaConfiguration on a LITE-tier user pool and persists it, instead of refusing with FeatureUnavailableInTierException.
Failing check: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging

AWS: Email message MFA is a feature-plan–gated capability. Smithy (com.amazonaws.cognitoidentityprovider): SetUserPoolMfaConfig models the error FeatureUnavailableInTierException (errorKind client, httpError 403), whose documentation states it "is thrown when a feature that you attempted to use or configure isn't included in your user pool's current feature plan". EmailMfaConfigType's own documentation states "To activate this setting, your user pool must be in the …" feature plan above Lite. So on a pool created with UserPoolTier=LITE, a SetUserPoolMfaConfig call carrying EmailMfaConfiguration must be refused with HTTP 403, error code FeatureUnavailableInTierException, and the recorded message "The following feature is not available for the LITE pricing tier configured: Email MFA". In that message the fixed text is "The following feature is not available for the " + "<tier> pricing tier configured: " + "<feature name>"; the variable parts are the pool's configured tier token taken from state (LITE here) and the gated feature's display name taken from which request member triggered the gate ("Email MFA" for EmailMfaConfiguration, "Managed Login Version 2" for ManagedLoginVersion=2, "Log Streaming" for a userAuthEvents LogConfiguration). The refusal must have no side effect: a following GetUserPoolMfaConfig must show no EmailMfaConfiguration.

Emulator: the call SUCCEEDS with HTTP 200 and echoes the configuration back — the replay recorded {'EmailMfaConfiguration': {'Message': 'Your verification code is {####}', 'Subject': 'Your verification code'}, 'MfaConfiguration': 'OPTIONAL'} with HTTPStatusCode 200 — so no error code and no error message are produced at all, and email MFA is left activated on a Lite pool. Note this is specific to SetUserPoolMfaConfig: the same check's earlier tier gates on CreateUserPoolDomain and UpdateUserPoolDomain (ManagedLoginVersion=2 on the same LITE pool) were refused correctly with FeatureUnavailableInTierException and the matching message, and the positive control SetUserPoolMfaConfig(MfaConfiguration="OFF") on the same pool also behaves, so the gap is the tier precondition on the EmailMfaConfiguration member rather than the operation as a whole.

Why: a client that relies on the tier boundary gets the opposite answer. Code written against AWS treats FeatureUnavailableInTierException as the signal to upgrade the pool's feature plan or fall back to another MFA factor; against the emulator the configuration silently lands, so tests that assert the Lite-tier refusal pass wrongly, and an application believes email OTP MFA is active on a pool where AWS would never have enabled it. It is an overly-permissive divergence: the emulator admits a configuration AWS rejects, and it also mutates pool state that AWS leaves untouched.

Fix: the tier precondition belongs in the SetUserPoolMfaConfig handler's validation, alongside the tier check that CreateUserPoolDomain/UpdateUserPoolDomain already perform — read the pool's UserPoolTier from state and, when EmailMfaConfiguration is present on a pool below ESSENTIALS, return FeatureUnavailableInTierException (403) with the feature-name message template above and commit nothing. The same validation path should cover the userAuthEvents/"Log Streaming" gate on SetLogDeliveryConfiguration, which this check exercises after the failing step and therefore never reached.

Traces: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging (fails at the third tier assertion, SetUserPoolMfaConfig with EmailMfaConfiguration on a LITE pool).

Cause 2 of 2: a replica created by CreateUserPoolReplica is reported as ACTIVE on its first ListUserPoolReplicas read instead of INACTIVE.
Failing check: cognito-idp.update-user-pool-replica-reflects-region-config-change

AWS: Smithy UserPoolReplicaType.Status targets ReplicaStatusType and its documentation enumerates the lifecycle explicitly — CREATING ("The replica is being created"), INACTIVE ("The replica has been created, but is not accepting requests for end-users. Administrator configuration operations are supported"), ACTIVE ("The replica is available for both end-user and administrator operations"), DELETING. A newly created replica therefore settles in INACTIVE; it only becomes ACTIVE when the caller asks for it, via UpdateUserPoolReplica, whose required Status member targets UpdateReplicaStatusType with exactly the values ACTIVE and INACTIVE. CreateUserPoolReplica does not take a Status at all (its input is UserPoolId, RegionName, UserPoolTags), so there is no way to request an active replica at creation time — the ACTIVE state is reachable only through the subsequent update.

Emulator: the replica reaches a terminal status of ACTIVE directly out of CreateUserPoolReplica. The check polls ListUserPoolReplicas until the replica's Status leaves CREATING and asserts the settled value; the replay recorded "expected new replica to start INACTIVE, got ACTIVE". The emulator thus skips the INACTIVE resting state and makes the ACTIVE transition unobservable.

Why: the replica state machine is wrong in the direction that grants capability nobody asked for. A client that creates a replica and then checks whether it is serving end-user traffic is told yes before any UpdateUserPoolReplica(Status=ACTIVE) call, so an activation step can be dropped against the emulator and will then fail in real AWS; conversely a client that waits for INACTIVE before configuring the replica waits forever. It also hides the UpdateUserPoolReplica(ACTIVE) transition entirely, because the target state is already the initial one.

Fix: the replica state machine in the CreateUserPoolReplica handler and the replica record in cognito-idp state — a created replica must settle in INACTIVE, and only UpdateUserPoolReplica may move it to ACTIVE (and back to INACTIVE), with each transition visible to a fresh ListUserPoolReplicas read. Note the open issue 20261026T235252Z-cognito-idp-createuserpoolreplica-stub.md covers CreateUserPoolReplica answering as an unimplemented stub; that is a different defect and does not cover this status value — in this run the operation answered successfully and returned the wrong lifecycle state.

Traces: cognito-idp.update-user-pool-replica-reflects-region-config-change (fails on the first settled-status assertion after CreateUserPoolReplica).

## Evidence
Cause 1 — cognito-idp.feature-unavailable-in-tier-domain-mfa-logging [mismatch]
Expected (AWS): SetUserPoolMfaConfig(UserPoolId=<LITE pool>, EmailMfaConfiguration={Message:'…{####}…', Subject:'…'}, MfaConfiguration='OPTIONAL') -> HTTP 403, Error.Code = 'FeatureUnavailableInTierException', Error.Message = 'The following feature is not available for the LITE pricing tier configured: Email MFA'; a following GetUserPoolMfaConfig carries no EmailMfaConfiguration.
Actual (emulator): HTTP 200 with body echoing the configuration. Harness output:
  AssertionError: expected a rejection, but the call SUCCEEDED and returned {'EmailMfaConfiguration': {'Message': 'Your verification code is {####}', 'Subject': 'Your verification code'}, 'MfaConfiguration': 'OPTIONAL', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Controls in the same check that DID behave (so the finding is narrow): CreateUserPoolDomain(ManagedLoginVersion=2) and UpdateUserPoolDomain(ManagedLoginVersion=2) on the same LITE pool were both refused with FeatureUnavailableInTierException and the matching '…LITE pricing tier configured: Managed Login Version 2' message; SetUserPoolMfaConfig(MfaConfiguration='OFF') on the same pool succeeded as expected.
Model basis: Smithy SetUserPoolMfaConfig errors include FeatureUnavailableInTierException (client, httpError 403); EmailMfaConfigType documents that the feature requires a higher feature plan than Lite.

Cause 2 — cognito-idp.update-user-pool-replica-reflects-region-config-change [mismatch]
Expected (AWS): after CreateUserPoolReplica(UserPoolId=<PLUS pool with multi-Region CMK>, RegionName=<partner region>), the replica's Status on ListUserPoolReplicas settles at 'INACTIVE'; UpdateUserPoolReplica(Status='ACTIVE') then moves it to 'ACTIVE' and UpdateUserPoolReplica(Status='INACTIVE') back.
Actual (emulator): the settled status is 'ACTIVE' immediately. Harness output:
  AssertionError: expected new replica to start INACTIVE, got ACTIVE
Model basis: Smithy UserPoolReplicaType.Status (ReplicaStatusType) documents CREATING / INACTIVE ("created, but is not accepting requests for end-users") / ACTIVE / DELETING; CreateUserPoolReplica's input has no Status member, and UpdateReplicaStatusType = [ACTIVE, INACTIVE] is the only way to set it.

## Steps to Reproduce
Cause 1 (SetUserPoolMfaConfig tier gate):
1. cognito-idp CreateUserPool(PoolName=<any>, UserPoolTier='LITE').
2. SetUserPoolMfaConfig(UserPoolId=<pool>, MfaConfiguration='OFF') — succeeds on AWS and on the emulator (control).
3. SetUserPoolMfaConfig(UserPoolId=<pool>, MfaConfiguration='OPTIONAL', EmailMfaConfiguration={'Message': '<text with {####}>', 'Subject': '<text>'}).
4. AWS: HTTP 403 FeatureUnavailableInTierException, message 'The following feature is not available for the LITE pricing tier configured: Email MFA'. Emulator: HTTP 200 and the config is accepted.
5. GetUserPoolMfaConfig(UserPoolId=<pool>) — AWS shows no EmailMfaConfiguration; the emulator shows the one just written.
Replay: run_behavioral_suite and inspect check cognito-idp.feature-unavailable-in-tier-domain-mfa-logging.

Cause 2 (replica initial status):
1. kms CreateKey(MultiRegion=true, SYMMETRIC_DEFAULT) with a policy allowing cognito-idp.amazonaws.com and identitystore.amazonaws.com, then ReplicateKey into a second Region and wait for the replica key to be Enabled.
2. cognito-idp CreateUserPool(UserPoolTier='PLUS', KeyConfiguration={'KeyType':'CUSTOMER_MANAGED_KEY','KmsKeyArn':<multi-Region key ARN>}).
3. CreateUserPoolReplica(UserPoolId=<pool>, RegionName=<second region>).
4. Poll ListUserPoolReplicas(UserPoolId=<pool>) until the entry for that Region leaves CREATING.
5. AWS: Status == 'INACTIVE'. Emulator: Status == 'ACTIVE'.
6. UpdateUserPoolReplica(Status='ACTIVE') then ('INACTIVE') and re-read ListUserPoolReplicas to see each transition.
Replay: run_behavioral_suite and inspect check cognito-idp.update-user-pool-replica-reflects-region-config-change.
