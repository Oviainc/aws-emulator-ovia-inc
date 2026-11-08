Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.get-user-pool-mfa-config-reflects-latest-set-config
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|SetUserPoolMfaConfig|InvalidParameterException
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# cognito-idp: SetUserPoolMfaConfig refuses de-activating TOTP while MfaConfiguration=OPTIONAL, so the pool's MFA posture is one-way (InvalidParameterException instead of 200)

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetUserPoolMfaConfig

## Description
One net-new root cause, observed from the behavioral corpus.

Cause 1 of 1: SetUserPoolMfaConfig rejects a well-formed request that de-activates software-token (TOTP) MFA while leaving MfaConfiguration=OPTIONAL, so the per-method MFA configuration of a user pool can be turned on but never back off, and the subsequent GetUserPoolMfaConfig still reports the stale activated state.
  AWS: Smithy (com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json): SetUserPoolMfaConfig input = {UserPoolId (required, UserPoolIdType, len 1..55, pattern ^[\w-]+_[0-9a-zA-Z]+$), SmsMfaConfiguration (SmsMfaConfigType), SoftwareTokenMfaConfiguration (SoftwareTokenMfaConfigType), EmailMfaConfiguration (EmailMfaConfigType), MfaConfiguration (UserPoolMfaType enum, one of OFF | ON | OPTIONAL), WebAuthnConfiguration}; output echoes the same members. SoftwareTokenMfaConfigType.Enabled is the activation state of TOTP MFA, and the model couples it to nothing: Enabled=false is a legal value and OPTIONAL is a legal MfaConfiguration, and the documented semantic of OPTIONAL is that "your application must make a client-side determination of whether a user wants to register an MFA device" — i.e. OPTIONAL does not require the pool to currently offer any factor. The operation is documented as the way to set a pool's MFA configuration, so a call carrying MfaConfiguration=OPTIONAL together with SoftwareTokenMfaConfiguration={"Enabled": false} is accepted with HTTP 200 echoing the stored configuration (MfaConfiguration "OPTIONAL", SoftwareTokenMfaConfiguration {"Enabled": false}), and the following GetUserPoolMfaConfig projects that stored configuration back (SoftwareTokenMfaConfiguration.Enabled = false). The modeled 400s for this input (InvalidParameterException, InvalidSmsRoleAccessPolicyException, InvalidSmsRoleTrustRelationshipException, ConcurrentModificationException) describe invalid parameters and bad SMS roles, none of which a syntactically valid de-activation is. Recorded behavior confirms the accept: the replayed check applies MfaConfiguration=OPTIONAL with SoftwareTokenMfaConfiguration.Enabled=true, then re-applies MfaConfiguration=OPTIONAL with SoftwareTokenMfaConfiguration.Enabled=false and expects the second write to land and be read back.
  Emulator: the second SetUserPoolMfaConfig is refused with HTTP 400 InvalidParameterException and the message "Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled." — that message is entirely fixed text with no variable part taken from the request or from pool state, and AWS does not return it for this request class at all. Nothing is stored, so the pool keeps the previous configuration and GetUserPoolMfaConfig keeps answering SoftwareTokenMfaConfiguration.Enabled=true. The emulator's handler enforces an at-least-one-factor rule for every MfaConfiguration value other than OFF, treating a present-but-disabled SoftwareTokenMfaConfiguration as enabling nothing; AWS applies no such rule to OPTIONAL.
  Why: a client relying on AWS's behavior cannot ever disable TOTP on a pool — switching a pool to SMS-only or email-only MFA, rolling back a mis-applied TOTP activation, or driving a pool's MFA posture declaratively all fail on a 400 that AWS never returns, and any converge/verify loop that re-reads GetUserPoolMfaConfig to confirm the change never terminates because the read keeps reporting the opposite, stale value. Code that branches on the modeled error set sees InvalidParameterException for a request it knows is valid and has no correct retry.
  Fix: SetUserPoolMfaConfig's handler validation (the at-least-one-factor gate and its MFA_METHOD_REQUIRED_MESSAGE refusal in services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_user_pool_mfa_config.rs, around the mfa_configuration != MFA_OFF check and the enables_an_mfa_factor helper) and the user-pool MFA state it writes: accept MfaConfiguration=OPTIONAL with all per-method factors absent or disabled, store the supplied SoftwareTokenMfaConfiguration (including Enabled=false), and let GetUserPoolMfaConfig project the last stored configuration.
  Traces: behavioral check cognito-idp.get-user-pool-mfa-config-reflects-latest-set-config.

Relation to already-filed work: the open issue 20261108T024537Z-cognito-idp-user-pool-mfa-configuration-cannot-be.md ("user-pool MFA configuration cannot be turned back off") describes the same emulator defect from the WIRE corpus and carries only wire replay keys (fixture cognito-idp-user-pool-mfa-config-reflects-latest-set, steps task#4/task#5); the behavioral replay did not report this check as owned by it, so this issue is filed to attach the behavioral check id as a replay key. A fixer closing one should re-replay both. The adjacent open issues 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md and 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md do not cover a valid de-activating SetUserPoolMfaConfig being refused.

## Evidence
Behavioral check cognito-idp.get-user-pool-mfa-config-reflects-latest-set-config — verdict: error.

Sequence exercised (one freshly created user pool):
  1. SetUserPoolMfaConfig(UserPoolId=<pool>, SoftwareTokenMfaConfiguration={"Enabled": true}, MfaConfiguration="OPTIONAL")  -> accepted
  2. GetUserPoolMfaConfig(UserPoolId=<pool>) -> MfaConfiguration "OPTIONAL", SoftwareTokenMfaConfiguration.Enabled true
  3. SetUserPoolMfaConfig(UserPoolId=<pool>, SoftwareTokenMfaConfiguration={"Enabled": false}, MfaConfiguration="OPTIONAL")

Step 3:
  expected: HTTP 200, body echoing MfaConfiguration = "OPTIONAL" and SoftwareTokenMfaConfiguration = {"Enabled": false}; the following GetUserPoolMfaConfig then returns MfaConfiguration "OPTIONAL" and SoftwareTokenMfaConfiguration.Enabled = false.
  actual:   HTTP 400, error code InvalidParameterException, message "Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled." (raised by botocore as botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the SetUserPoolMfaConfig operation: Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled.). The whole message is fixed emulator text; it interpolates nothing from the request or state. No write lands, so GetUserPoolMfaConfig keeps reporting SoftwareTokenMfaConfiguration.Enabled = true.

Emulator source backing the diagnosis — services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_user_pool_mfa_config.rs:
  lines 262-270: `if mfa_configuration != crate::state::MFA_OFF && !enables_an_mfa_factor(...) { return Err(invalid_parameter(MFA_METHOD_REQUIRED_MESSAGE.to_string())); }`
  lines 351-352: MFA_METHOD_REQUIRED_MESSAGE = "Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled."
  lines 364-372: enables_an_mfa_factor treats `SoftwareTokenMfaConfiguration{Enabled:false}` as enabling nothing, so the gate fires for MfaConfiguration=OPTIONAL.

Model evidence (smithy_query on com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json): SetUserPoolMfaConfig input MfaConfiguration -> UserPoolMfaType enum {OFF, ON, OPTIONAL}; SoftwareTokenMfaConfiguration -> SoftwareTokenMfaConfigType; output echoes SmsMfaConfiguration / SoftwareTokenMfaConfiguration / EmailMfaConfiguration / MfaConfiguration / WebAuthnConfiguration; GetUserPoolMfaConfig output carries the same members. No modeled error, and no documented rule, couples MfaConfiguration=OPTIONAL to at least one enabled factor.

## Steps to Reproduce
1. git_clone the project repo and let the behavioral suite build and boot the emulator.
2. Run the behavioral suite (simparity-behavioral-aws-v1).
3. Inspect check cognito-idp.get-user-pool-mfa-config-reflects-latest-set-config, or reproduce by hand against the booted emulator:
   a. cognito-idp CreateUserPool(PoolName=<any>) -> note UserPoolId.
   b. SetUserPoolMfaConfig(UserPoolId=<id>, SoftwareTokenMfaConfiguration={"Enabled": true}, MfaConfiguration="OPTIONAL") -> 200.
   c. SetUserPoolMfaConfig(UserPoolId=<id>, SoftwareTokenMfaConfiguration={"Enabled": false}, MfaConfiguration="OPTIONAL") -> observe HTTP 400 InvalidParameterException "Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled." where AWS answers 200 echoing MfaConfiguration "OPTIONAL" and SoftwareTokenMfaConfiguration {"Enabled": false}.
   d. GetUserPoolMfaConfig(UserPoolId=<id>) -> observe SoftwareTokenMfaConfiguration.Enabled still true instead of false.
4. After fixing, re-replay cognito-idp.get-user-pool-mfa-config-reflects-latest-set-config and confirm step (c) returns 200 with the stored configuration and step (d) reads back Enabled=false.
