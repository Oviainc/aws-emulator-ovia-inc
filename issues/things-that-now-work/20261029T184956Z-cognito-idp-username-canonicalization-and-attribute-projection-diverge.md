Issue-Source: trace-sweep-divergence
Issue-Trace: cip-cfg-case-insensitive-username-resolution
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-cfg-case-insensitive-username-resolution cip-cfg-username-attributes-email-rejects-non-email-username cognito-idp-admin-user-signup-confirm-and-mfa cognito-idp-self-service-signup-and-token-session cognito-idp-empty-vs-absent-custom-attribute-distinction cip-cfg-alias-email-duplicate-signup-and-alias-claim
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminGetUser|2xx->2xx|$.Username
Issue-Key: cognito-idp|AdminGetUser|2xx->2xx|$.UserAttributes[*]
Issue-Key: cognito-idp|GetUser|2xx->2xx|$.UserAttributes[Name="email_verified"]
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|$.Enabled
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|$.UserAttributes
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|$.UserCreateDate
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|$.UserLastModifiedDate
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|$.UserStatus
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|$.Username
Issue-Key: cognito-idp|AdminGetUser|2xx->4xx|status
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: username canonicalization and attribute projection diverge in user reads

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminGetUser

## Description
Six recorded causes in the user read model: the Username a read returns, which attributes it projects, and whether an alias-claimed user can be resolved at all.

Cause 1 of 6: AdminGetUser on a pool with case-insensitive usernames returns the stored mixed-case spelling. AWS returns the pool's canonical (case-folded) username — the value the service resolved the lookup to, derived from the stored user, not the request's spelling. Emulator returns 'CaseUser-Mixed' where AWS returns 'caseuser-mixed'. Why: clients key caches and group membership on the returned Username; a case-varying value breaks equality against the same user fetched by a different spelling. Fix: AdminGetUser projection must emit the canonical username from the user record. Fixture cip-cfg-case-insensitive-username-resolution, step task#7.
Cause 2 of 6: on a pool with UsernameAttributes=email, AdminGetUser must return Username = the user's `sub` and project `email` plus `email_verified` in UserAttributes. The emulator returns Username = the email address and projects only `sub`. Why: on such a pool the email is an alias, not the username; a client that uses the returned Username for AdminSetUserPassword or AdminAddUserToGroup sends the wrong identifier. Fix: the username-attributes resolution in the user state and the AdminGetUser projection. Fixture cip-cfg-username-attributes-email-rejects-non-email-username, step task#6.
Cause 3 of 6: after SignUp, AdminGetUser does not project `email_verified` — AWS returns it with value 'false' until the address is verified, and places it before the service-generated `sub`. The emulator returns `sub` in that position and no `email_verified` at all. Fix: the SignUp path must seed `email_verified` and the projection must order service attributes as the service does. Fixture cognito-idp-admin-user-signup-confirm-and-mfa, step task#6.
Cause 4 of 6: the same omission on the self-service read — GetUser does not project `email_verified` for an unverified signup. Fixture cognito-idp-self-service-signup-and-token-session, step task#10.
Cause 5 of 6: a custom attribute written with an empty string is returned as an attribute with an empty Value. AWS treats an empty write as absent and omits the attribute, returning only `sub`. Why: clients distinguish 'attribute cleared' from 'attribute set to empty'; projecting the empty attribute inverts that test. Fix: empty-value normalization in the attribute state. Fixture cognito-idp-empty-vs-absent-custom-attribute-distinction, step task#5.
Cause 6 of 6: after an email alias is claimed by a second signup, AdminGetUser for the original username answers 404 with no body. AWS answers 200 with the user — Enabled true, UserStatus CONFIRMED, the email/email_verified/sub attributes, Username, UserCreateDate and UserLastModifiedDate. Why: the user still exists; a 404 makes an existing account unmanageable. Fix: alias-claim handling in the user state must not unlink or hide the original user record. Fixture cip-cfg-alias-email-duplicate-signup-and-alias-claim, step task#9 (2 recorded steps).

Related open issue that does not carry these replay keys: 20261029T182116Z-cognito-idp-stored-state-diverges-empty-absent-normalization.md.

## Evidence
AdminGetUser (cip-cfg-case-insensitive-username-resolution#task#7, 200 both sides): $.Username expected 'caseuser-mixed', got 'CaseUser-Mixed'.
AdminGetUser (cip-cfg-username-attributes-email-rejects-non-email-username#task#6): $.Username expected the user's sub, got the email address; $.UserAttributes[0] expected {'Name':'email',...} got {'Name':'sub',...}; $.UserAttributes[1] expected {'Name':'email_verified','Value':'false'} got None.
AdminGetUser (cognito-idp-admin-user-signup-confirm-and-mfa#task#6): $.UserAttributes[1] expected {'Name':'email_verified','Value':'false'}, got the sub attribute.
GetUser (cognito-idp-self-service-signup-and-token-session#task#10): $.UserAttributes[Name="email_verified"] expected {'Name':'email_verified','Value':'false'}, got None.
AdminGetUser (cognito-idp-empty-vs-absent-custom-attribute-distinction#task#5): $.UserAttributes[0] extra — expected only the sub attribute, got {'Name':'custom:note','Value':''}.
AdminGetUser (cip-cfg-alias-email-duplicate-signup-and-alias-claim#task#9): status expected 200 got 404; Enabled/UserAttributes/UserStatus/Username/UserCreateDate/UserLastModifiedDate all missing.

## Steps to Reproduce
1. Replay cip-cfg-case-insensitive-username-resolution step task#7 and compare the returned Username with the recording.
2. Replay cip-cfg-username-attributes-email-rejects-non-email-username step task#6 on a UsernameAttributes=email pool.
3. Replay cognito-idp-admin-user-signup-confirm-and-mfa step task#6 and cognito-idp-self-service-signup-and-token-session step task#10 and look for email_verified.
4. Replay cognito-idp-empty-vs-absent-custom-attribute-distinction step task#5 and cip-cfg-alias-email-duplicate-signup-and-alias-claim step task#9.
