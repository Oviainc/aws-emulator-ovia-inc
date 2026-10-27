Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.op-contract-describe-risk-configuration
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|DescribeRiskConfiguration|UserPoolAddOnNotEnabledException
Issue-Commit: c7f894d9cab6281b7f6a51d1fbce512135c74b00

# cognito-idp: DescribeRiskConfiguration refuses a pool without the advanced-security add-on, and SetRiskConfiguration gates an AccountTakeover-only write on it

> _Note: this issue was identified against the code base at commit `c7f894d9cab6281b7f6a51d1fbce512135c74b00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRiskConfiguration

## Description
Three causes in the cognito-idp threat-protection (risk configuration) surface, all exercised by the single failing check cognito-idp.op-contract-describe-risk-configuration. Not covered by any open issue (the only issues matching 'risk' are wellarchitected lens-review issues, 20261024T063221Z-wellarchitected-getlensreview-wrong-behavior.md and 20261024T170707Z-wellarchitected-listlensreviews-wrong-behavior.md), and the check is neither marked ALREADY FILED nor OWNED in the replay report.

Cause 1 of 3 — DescribeRiskConfiguration is refused on a pool whose add-ons are off (this is the cause that aborted the replay).
AWS: DescribeRiskConfiguration is a READ and succeeds (HTTP 200) on a user pool that never enabled the advanced-security/threat-protection add-on. Smithy: DescribeRiskConfiguration input {UserPoolId required, UserPoolIdType len 1..55 pattern ^[\w-]+_[0-9a-zA-Z]+$; ClientId optional}, output {RiskConfiguration required, RiskConfigurationType}; it does model UserPoolAddOnNotEnabledException (client, HTTP 400, "This exception is thrown when user pool add-ons aren't enabled."), but recorded behavior shows that error is NOT raised for the read path — only for the gated write path. Recorded behavior: the read returns 200 and simply omits the RiskConfiguration member for a pool that has never had a risk configuration written.
Emulator: DescribeRiskConfiguration answers HTTP 400 UserPoolAddOnNotEnabledException with message "User pool add-ons is not enabled." for any pool whose add-ons are off, before it ever looks at stored configuration. The check creates a plain pool with CreateUserPool (no UserPoolAddOns override) and the immediately following DescribeRiskConfiguration raises botocore.errorfactory.UserPoolAddOnNotEnabledException: An error occurred (UserPoolAddOnNotEnabledException) when calling the DescribeRiskConfiguration operation: User pool add-ons is not enabled. Message anatomy: the fixed text is "User pool add-ons is not enabled." with no variable parts — the message itself matches AWS for the error class, the defect is that this error class is produced at all on a read.
Why: a client that reads back the risk/threat-protection posture of an ordinary user pool (console-style inventory, drift detection, CDK/CFN readers, compliance sweeps) gets a hard 400 instead of a 200 it can interpret. Code written against AWS treats UserPoolAddOnNotEnabledException as a signal that the paid add-on is missing on a WRITE; seeing it on a read makes every un-upgraded pool look like an error rather than an unconfigured pool, and callers that do not special-case it abort the whole sweep.
Fix: the DescribeRiskConfiguration handler's add-on precondition — remove the add-on gate from the read path (keep the ResourceNotFoundException lookup for an absent pool/app client ahead of it, which is already correct).

Cause 2 of 3 — the read synthesizes a default RiskConfiguration for a level nothing ever configured.
AWS: for a pool (or app client) that has never had a risk configuration written, the DescribeRiskConfiguration response omits the RiskConfiguration member entirely rather than reporting a default/empty structure. Recorded behavior only (the model marks RiskConfiguration required on the output shape, so the wire shape AWS actually sends is narrower than the model).
Emulator: when nothing is stored at the named level it manufactures a placeholder structure instead of omitting the member, so a caller sees a RiskConfiguration that was never configured. Not reached by the replay because Cause 1 refused the call first; it is the next assertion the check makes (assert "RiskConfiguration" not in response_plain) and will fail once Cause 1 is fixed.
Why: a client cannot distinguish "never configured" from "configured to defaults", so drift detection and "is threat protection set up?" checks report a configuration that does not exist, and idempotent reconcilers skip writes they must perform.
Fix: the DescribeRiskConfiguration handler's response construction — make the output's RiskConfiguration absent when the named level holds no stored configuration, rather than substituting a synthesized unconfigured value.

Cause 3 of 3 — SetRiskConfiguration gates every write on the add-on, including an AccountTakeover-only write.
AWS: SetRiskConfiguration carrying only AccountTakeoverRiskConfiguration is ACCEPTED (HTTP 200) on a pool that never enabled the advanced-security add-on and the write is persisted and readable back through DescribeRiskConfiguration; only the CompromisedCredentialsRiskConfiguration write path is refused with UserPoolAddOnNotEnabledException (HTTP 400, "User pool add-ons is not enabled."). Smithy: SetRiskConfiguration input {UserPoolId required, ClientId, CompromisedCredentialsRiskConfiguration, AccountTakeoverRiskConfiguration, RiskExceptionConfiguration}, errors include UserPoolAddOnNotEnabledException (client, HTTP 400); the model does not say which member triggers it, so the per-member rule is recorded behavior only.
Emulator: the SetRiskConfiguration handler applies one add-on precondition to the whole operation and refuses every write on a pool whose add-ons are off, whatever members the request carries. Not reached by the replay because Cause 1 aborted the check earlier; it is the final leg of the same check (set_risk_configuration with AccountTakeoverRiskConfiguration only on the plain pool, then a read-back of the persisted Actions).
Why: a client configuring account-takeover actions on an ordinary pool — a path AWS allows without the paid add-on — is told it needs an add-on it does not need, so the configuration is never applied and the caller's setup step fails.
Fix: the SetRiskConfiguration handler's add-on precondition — scope it to the CompromisedCredentialsRiskConfiguration member instead of the whole request.

## Evidence
Failing check: cognito-idp.op-contract-describe-risk-configuration ([error], not marked ALREADY FILED or OWNED).

Cause 1, observed: after CreateUserPool with no UserPoolAddOns, DescribeRiskConfiguration(UserPoolId=<plain pool>) ->
  botocore.errorfactory.UserPoolAddOnNotEnabledException: An error occurred (UserPoolAddOnNotEnabledException) when calling the DescribeRiskConfiguration operation: User pool add-ons is not enabled.
Expected: HTTP 200 with no RiskConfiguration member in the body (the check asserts `"RiskConfiguration" not in response_plain`).
Actual: HTTP 400, error code UserPoolAddOnNotEnabledException, message "User pool add-ons is not enabled." (fixed text; no request- or state-derived parts).
The earlier legs of the check passed against the emulator: CreateUserPool with UserPoolAddOns={"AdvancedSecurityMode":"AUDIT"}, SetRiskConfiguration(CompromisedCredentialsRiskConfiguration) on that pool, and DescribeRiskConfiguration on it returning a RiskConfiguration whose UserPoolId matches — so the divergence is specific to the un-upgraded pool, not to the operation being unimplemented.

Cause 2, from the same check's next assertion and the handler source: the read substitutes a synthesized "unconfigured" RiskConfiguration for a level with nothing stored (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/describe_risk_configuration.rs lines 196-223) instead of omitting the member.

Cause 3, from the same check's final leg and the handler source: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_risk_configuration.rs lines 224-229 refuse the whole operation when the pool's add-ons are off, so an AccountTakeoverRiskConfiguration-only write on a plain pool is rejected with UserPoolAddOnNotEnabledException (HTTP 400) where AWS accepts and persists it.

Supporting file/line citations: describe_risk_configuration.rs lines 186-188 (`if !pool.add_ons_enabled() { return Err(add_ons_not_enabled()); }`) and lines 244-253 (the UserPoolAddOnNotEnabledException constructor), set_risk_configuration.rs lines 227-229.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a cognito-idp client at it.
2. CreateUserPool(PoolName=<any>) with NO UserPoolAddOns member -> note the returned UserPool.Id (the "plain" pool).
3. DescribeRiskConfiguration(UserPoolId=<plain pool id>).
   Observed: HTTP 400 UserPoolAddOnNotEnabledException "User pool add-ons is not enabled."
   Expected (AWS): HTTP 200 and the body contains no RiskConfiguration member.
4. After step 3 is fixed, SetRiskConfiguration(UserPoolId=<plain pool id>, AccountTakeoverRiskConfiguration={Actions:{LowAction:{Notify:false,EventAction:"NO_ACTION"},MediumAction:{...},HighAction:{...}}}).
   Expected (AWS): HTTP 200, accepted and persisted.
   Observed: HTTP 400 UserPoolAddOnNotEnabledException.
5. DescribeRiskConfiguration(UserPoolId=<plain pool id>) again -> expect the persisted AccountTakeoverRiskConfiguration.Actions with EventAction NO_ACTION for Low/Medium/High.
6. Control (already passing, must keep passing): CreateUserPool with UserPoolAddOns={"AdvancedSecurityMode":"AUDIT"}, SetRiskConfiguration(CompromisedCredentialsRiskConfiguration={Actions:{EventAction:"NO_ACTION"}}), then DescribeRiskConfiguration -> 200 with RiskConfiguration.UserPoolId equal to that pool id.
7. Replay cognito-idp.op-contract-describe-risk-configuration to confirm.
