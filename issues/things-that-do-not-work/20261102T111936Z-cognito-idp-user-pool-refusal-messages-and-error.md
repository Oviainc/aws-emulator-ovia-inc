Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-acr-traj-pool-id-mistakes
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-acr-traj-pool-id-mistakes cognito-idp-osc-dashboards-client-callback-diagnosis cognito-idp-acr-traj-sms-mfa-config-mistakes cognito-idp-acr-traj-forgot-password-without-app-client cognito-idp-user-pool-replica-error-paths
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DescribeUserPool|4xx->4xx|$.message
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->4xx|$.message
Issue-Key: cognito-idp|ForgotPassword|4xx->4xx|$.message
Issue-Key: cognito-idp|DeleteUserPoolReplica|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261029T184956Z-cognito-idp-seven-refusals-are-raised-as-the.md
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# cognito-idp: user-pool refusal messages and error classes diverge from AWS

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPool

## Description
Four independent causes (all cognito-idp). Each refusal reaches the right HTTP status but carries a message — and in one case an error class — the emulator invented rather than AWS's. Net-new: the open issues 'cognito-idp: validation and auth refusal message rendering diverges from AWS' (20261029T184956Z-cognito-idp-validation-and-auth-refusal-message-rendering.md), 'cognito-idp: ten not-found refusal messages diverge from AWS wording' (20261029T184956Z-cognito-idp-ten-not-found-refusal-messages-diverge.md) and 'cognito-idp: seven refusals are raised as the wrong modeled error class' (20261029T184956Z-cognito-idp-seven-refusals-are-raised-as-the.md) cover different refusals and carry none of the replay keys below.

Cause 1 of 4: DescribeUserPool with a null UserPoolId is diagnosed with the generic constraint template instead of AWS's null-check message.
AWS: when UserPoolId is absent/null, DescribeUserPool answers HTTP 400 with message 'UserPoolId must not be null' — fixed text apart from the member name 'UserPoolId', which is the request member that was null. AWS uses this short null-check form rather than the '1 validation error detected: ...' constraint-report form it uses for values that are present but invalid. Smithy: DescribeUserPool.UserPoolId -> UserPoolIdType, @required, @length 1..55, @pattern ^[\w-]+_[0-9a-zA-Z]+$; modeled error InvalidParameterException.
Emulator: answers the same 400 but with "1 validation error detected: Value null at 'userPoolId' failed to satisfy constraint: Member must not be null".
Why: a client (or a test) that matches on AWS's refusal text to tell 'you forgot the pool id' from 'the pool id is malformed' matches neither branch, and the lowercased member name 'userPoolId' leaks a different naming convention than the wire member.
Fix: the shared request-validation message renderer — the null/missing-member path must emit '<Member> must not be null' rather than the constraint-report template.
Traces: cognito-idp-acr-traj-pool-id-mistakes#task#2 (cluster also covers cognito-idp-osc-dashboards-client-callback-diagnosis#task#5).

Cause 2 of 4: enum-constraint refusals render the allowed value set in the emulator's own order.
AWS: SetUserPoolMfaConfig refuses MfaConfiguration='on' with HTTP 400 and "1 validation error detected: Value 'on' at 'mfaConfiguration' failed to satisfy constraint: Member must satisfy enum value set: [OPTIONAL, OFF, ON]". Fixed text is the template; the variable parts are the offending value from the request ('on'), the member path, and the allowed value set — which AWS renders in its own fixed order, recorded as [OPTIONAL, OFF, ON]. Smithy: UserPoolMfaType is an enum with values OFF, ON, OPTIONAL — note the model's declaration order is NOT the order AWS prints, so the renderer cannot derive it by sorting or by model order.
Emulator: prints the set as [OFF, ON, OPTIONAL], i.e. sorted/model order.
Why: a client asserting on the refusal text to surface the legal values to a user sees a different string for every enum, so message-equality checks fail and generated user-facing help diverges from AWS's.
Fix: the enum-constraint branch of the shared validation message renderer (the allowed-value-set ordering), not the per-operation handler. Compare the same defect filed for a sibling service: 20261031T043446Z-cognito-identity-validation-error-messages-render-constraint-collections.md.
Traces: cognito-idp-acr-traj-sms-mfa-config-mistakes#task#3 (cluster also covers that fixture's task#5).

Cause 3 of 4: ForgotPassword against a nonexistent app client reports the client id instead of AWS's combination message.
AWS: ForgotPassword with a ClientId that does not exist answers HTTP 400 with message 'Username/client id combination not found.' — entirely fixed text. AWS deliberately does not disclose which half of the pair was wrong, and does not echo the client id. Smithy: ForgotPassword.ClientId -> ClientIdType (@required, @sensitive), ForgotPassword.Username -> UsernameType (@required, @sensitive); modeled errors ResourceNotFoundException / InvalidParameterException.
Emulator: answers 400 with 'User pool client <clientId> does not exist.', interpolating the requested client id.
Why: the emulator both breaks text matching and discloses the existence/absence of a @sensitive client id, inverting AWS's deliberate non-disclosure for this flow — a client testing user-existence-leak behavior gets the wrong answer.
Fix: ForgotPassword's handler/refusal mapping for an unresolvable ClientId.
Traces: cognito-idp-acr-traj-forgot-password-without-app-client#task#5.

Cause 4 of 4: DeleteUserPoolReplica raises the wrong modeled error for a replica that is not present in the requested region.
AWS: DeleteUserPoolReplica for a region that holds no replica of the pool answers __type ResourceNotFoundException with message 'Replica not found in the requested region' (fixed text) and header x-amzn-errortype 'ResourceNotFoundException:'. Smithy: DeleteUserPoolReplica input UserPoolId (@required) + RegionName (@required, @length 5..32); modeled errors include ResourceNotFoundException and InvalidParameterException.
Emulator: answers InvalidParameterException with 'The replica of user pool <poolId> in <region> is ACTIVE; only a replica in the INACTIVE status can be deleted.' and x-amzn-errortype 'InvalidParameterException:' — a status-precondition message (with pool id and region interpolated) where AWS reports a missing resource.
Why: SDK clients branch on the modeled error class; a caller retrying/ignoring ResourceNotFoundException for an idempotent teardown instead sees a non-retryable InvalidParameterException and aborts, and the message asserts a replica state that does not exist.
Fix: DeleteUserPoolReplica's handler — resolve the replica for the requested region first and raise ResourceNotFoundException when there is none, reserving the ACTIVE/INACTIVE precondition for a replica that does exist.
Traces: cognito-idp-user-pool-replica-error-paths#task#5.

## Evidence
Cause 1 — DescribeUserPool, cognito-idp-acr-traj-pool-id-mistakes#task#2 (2 steps, both 400->400): $.message expected 'UserPoolId must not be null', got "1 validation error detected: Value null at 'userPoolId' failed to satisfy constraint: Member must not be null".
Cause 2 — SetUserPoolMfaConfig, cognito-idp-acr-traj-sms-mfa-config-mistakes#task#3 (2 steps, 400->400): $.message expected "...enum value set: [OPTIONAL, OFF, ON]", got "...enum value set: [OFF, ON, OPTIONAL]".
Cause 3 — ForgotPassword, cognito-idp-acr-traj-forgot-password-without-app-client#task#5 (400->400): $.message expected 'Username/client id combination not found.', got 'User pool client 1example23456789abcdefghij does not exist.' (the client id shown is this run's value; the defect is that any client id is interpolated at all).
Cause 4 — DeleteUserPoolReplica, cognito-idp-user-pool-replica-error-paths#task#5 (400->400): $.__type expected 'ResourceNotFoundException', got 'InvalidParameterException'; $.message expected 'Replica not found in the requested region', got 'The replica of user pool <poolId> in us-east-1 is ACTIVE; only a replica in the INACTIVE status can be deleted.'; x-amzn-errortype expected 'ResourceNotFoundException:', got 'InvalidParameterException:'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay cognito-idp-acr-traj-pool-id-mistakes step task#2 — DescribeUserPool with UserPoolId null.
3. Cause 2: replay cognito-idp-acr-traj-sms-mfa-config-mistakes step task#3 — SetUserPoolMfaConfig with MfaConfiguration='on'.
4. Cause 3: replay cognito-idp-acr-traj-forgot-password-without-app-client step task#5 — ForgotPassword with an unknown ClientId.
5. Cause 4: replay cognito-idp-user-pool-replica-error-paths step task#5 — DeleteUserPoolReplica for a region with no replica.
6. Compare $.__type, $.message and the x-amzn-errortype header of each response against the recorded step.
