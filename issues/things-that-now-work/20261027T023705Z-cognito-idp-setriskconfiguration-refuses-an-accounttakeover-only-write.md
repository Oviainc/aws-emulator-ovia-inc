Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.code-delivery-failure-error
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|SetRiskConfiguration|UserPoolAddOnNotEnabledException
Issue-Commit: ce0180504904cd03f4c7508d1c0d16e7616d1185

# cognito-idp: SetRiskConfiguration refuses an AccountTakeover-only write on a user pool whose advanced-security add-on is off

> _Note: this issue was identified against the code base at commit `ce0180504904cd03f4c7508d1c0d16e7616d1185`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetRiskConfiguration

## Description
One net-new root cause, grounded in the failing check cognito-idp.code-delivery-failure-error ([error], marked neither ALREADY FILED nor OWNED in the replay report). Related open issue: 20261026T000005Z-cognito-idp-describeriskconfiguration-refuses-a-pool-without-the.md describes the same emulator gate as its Cause 3, but its replay key is the check cognito-idp.op-contract-describe-risk-configuration, not this one; this issue records the second check that the same gate aborts so the fix is re-replayed against it as well. The two should be closed together.

Cause 1 of 1: the SetRiskConfiguration handler applies a single advanced-security/add-on precondition to the whole operation, so a write that carries only AccountTakeoverRiskConfiguration is refused on an ordinary user pool.
AWS: SetRiskConfiguration accepts (HTTP 200) a request whose only risk member is AccountTakeoverRiskConfiguration on a user pool created with no UserPoolAddOns override, persists it, and returns the stored RiskConfiguration; the configuration is then readable through DescribeRiskConfiguration. Smithy: SetRiskConfiguration input {UserPoolId required (UserPoolIdType, len 1..55, pattern ^[\w-]+_[0-9a-zA-Z]+$), ClientId optional, CompromisedCredentialsRiskConfiguration, AccountTakeoverRiskConfiguration, RiskExceptionConfiguration}, output {RiskConfiguration required}; modeled errors include UserPoolAddOnNotEnabledException (client, HTTP 400, "This exception is thrown when user pool add-ons aren't enabled."). The model does not say which input member the add-on gate applies to, so the per-member rule is recorded behavior only: the recording shows the AccountTakeover-only write succeeding on a plain pool, while the add-on refusal belongs to the CompromisedCredentialsRiskConfiguration path.
Emulator: the call is refused with HTTP 400, error code UserPoolAddOnNotEnabledException and message "User pool add-ons is not enabled." (message anatomy: that sentence is fixed text, with no request- or state-derived parts — the wording matches AWS for this error class; the defect is that the error class is produced at all for an AccountTakeover-only write on a pool whose add-ons are off). The refusal happens before any member inspection, so no configuration is stored and the following DescribeRiskConfiguration read-back is never reached.
Why: a client configuring account-takeover actions (Low/Medium/High EventAction) on an ordinary user pool — a path AWS allows without the paid add-on — is told it needs an add-on it does not need. The configuration is silently never applied, and setup/reconcile code that treats UserPoolAddOnNotEnabledException as "the customer has not bought the add-on" aborts the whole provisioning step rather than retrying.
Fix: the SetRiskConfiguration handler's add-on precondition — scope the gate to the CompromisedCredentialsRiskConfiguration member instead of rejecting the whole request, keeping the ResourceNotFoundException pool/app-client lookup ahead of it.
Traces: cognito-idp.code-delivery-failure-error (the check's earlier legs — SignUp on a deliverable email, ResendConfirmationCode leaving the user UNCONFIRMED, AdminCreateUser with DesiredDeliveryMediums=["SMS"] and no phone_number refused with InvalidParameterException, GetUserAttributeVerificationCode for a valueless attribute refused with InvalidParameterException and the email positive control — all PASSED against the emulator, so this is the only divergence the check observed).

## Evidence
Failing check: cognito-idp.code-delivery-failure-error, verdict [error].

Replay output (tail):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.UserPoolAddOnNotEnabledException: An error occurred (UserPoolAddOnNotEnabledException) when calling the SetRiskConfiguration operation: User pool add-ons is not enabled.

The request that was refused (from the check body): set_risk_configuration(UserPoolId=<pool created by CreateUserPool with Policies/UserPoolTags/DeletionProtection=INACTIVE and AutoVerifiedAttributes=["email"], and NO UserPoolAddOns member>, ClientId=<app client of that pool>, AccountTakeoverRiskConfiguration={"Actions": {"LowAction": {"Notify": false, "EventAction": "NO_ACTION"}, "MediumAction": {...}, "HighAction": {...}}}).

Expected (AWS): HTTP 200, the AccountTakeoverRiskConfiguration persisted; the following DescribeRiskConfiguration(UserPoolId, ClientId) returns RiskConfiguration.AccountTakeoverRiskConfiguration.Actions with EventAction == "NO_ACTION" for LowAction, MediumAction and HighAction.
Actual (emulator): HTTP 400, code UserPoolAddOnNotEnabledException, message "User pool add-ons is not enabled."; nothing persisted, the read-back never reached.

Corroboration that this is a whole-operation gate rather than an unimplemented operation: the same emulator accepts SetRiskConfiguration on a pool created with UserPoolAddOns={"AdvancedSecurityMode":"AUDIT"} (see the passing legs recorded in open issue 20261026T000005Z-cognito-idp-describeriskconfiguration-refuses-a-pool-without-the.md), and that issue's Cause 3 cites services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_risk_configuration.rs lines 224-229 as the unconditional precondition.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a cognito-idp client at it.
2. CreateUserPool(PoolName=<unique>, AutoVerifiedAttributes=["email"], DeletionProtection="INACTIVE") with NO UserPoolAddOns member; note UserPool.Id.
3. CreateUserPoolClient(UserPoolId=<pool id>, ClientName=<unique>, GenerateSecret=false, ExplicitAuthFlows=["ALLOW_USER_PASSWORD_AUTH","ALLOW_ADMIN_USER_PASSWORD_AUTH"]); note ClientId.
4. SetRiskConfiguration(UserPoolId=<pool id>, ClientId=<client id>, AccountTakeoverRiskConfiguration={Actions:{LowAction:{Notify:false,EventAction:"NO_ACTION"},MediumAction:{Notify:false,EventAction:"NO_ACTION"},HighAction:{Notify:false,EventAction:"NO_ACTION"}}}).
   Observed: HTTP 400 UserPoolAddOnNotEnabledException "User pool add-ons is not enabled."
   Expected (AWS): HTTP 200, write accepted and persisted.
5. DescribeRiskConfiguration(UserPoolId=<pool id>, ClientId=<client id>) -> expect the persisted AccountTakeoverRiskConfiguration.Actions with EventAction "NO_ACTION" at all three levels.
6. Control that must keep working: on a pool created with UserPoolAddOns={"AdvancedSecurityMode":"AUDIT"}, SetRiskConfiguration with CompromisedCredentialsRiskConfiguration still succeeds; and the same CompromisedCredentials write on the plain pool of step 2 must still be refused with UserPoolAddOnNotEnabledException (HTTP 400).
7. Replay cognito-idp.code-delivery-failure-error to confirm (it must run past SetRiskConfiguration to the DescribeRiskConfiguration read-back).
