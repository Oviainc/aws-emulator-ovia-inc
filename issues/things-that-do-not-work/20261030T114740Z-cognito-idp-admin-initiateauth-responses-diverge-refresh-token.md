Issue-Source: trace-sweep-divergence
Issue-Trace: cip-autherr-admin-reset-password-required
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-autherr-admin-reset-password-required cip-autherr-enabled-ghost-not-authorized cip-autherr-enabled-wrong-password-matches-ghost cip-autherr-legacy-ghost-user-not-found cip-chal-mfa-setup-required cognito-idp-account-recovery-and-attribute-verification cognito-idp-self-service-account-deletion-lifecycle cip-cfg-case-insensitive-username-resolution cip-cfg-revoke-token-disabled-on-client cip-cfg-revoke-token-rejects-access-token cognito-idp-admin-user-auth-workflow
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.AuthenticationResult.RefreshToken
Issue-Key: cognito-idp|AdminInitiateAuth|2xx->2xx|$.AuthenticationResult.RefreshToken
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.ChallengeParameters.MFAS_CAN_SETUP
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.ChallengeParameters.USER_ID_FOR_SRP
Issue-Related: 20261029T184956Z-cognito-idp-revocation-provider-linking-risk-and-mfa.md
Issue-Commit: 2dabe34243f31609074debaf0ad21d5984b0ddd7

# cognito-idp: (Admin)InitiateAuth responses diverge — refresh token shape and missing MFA_SETUP challenge (3 causes)

> _Note: this issue was identified against the code base at commit `2dabe34243f31609074debaf0ad21d5984b0ddd7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Three net-new wire root causes on the cognito-idp authentication surface, all in the (Admin)InitiateAuth response path. Cause 3 lets a pool that requires MFA complete authentication with no second factor, so the bundle is HIGH. A fixer must close all THREE causes before submitting; each names its own fixture and step below.

Related open issues that do NOT cover these replay keys: 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md and 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md are both behavioral-corpus issues (Issue-Corpus: simparity-behavioral-aws-v1); neither carries a wire fixture/step key, which is why the replay reports these three clusters as unowned. They must be re-replayed against the wire fixtures named here.

Cause 1 of 3 — InitiateAuth's AuthenticationResult.RefreshToken does not have AWS's refresh-token shape.
AWS: Smithy: InitiateAuth output AuthenticationResult -> AuthenticationResultType.RefreshToken targets TokenModelType with @pattern ^[A-Za-z0-9-_=.]+$ and @sensitive — the '.' in that character class is what admits the JOSE compact serialization Cognito actually vends. Recorded behavior only for the structure: the recorded response carries a refresh token in JOSE compact form, i.e. several base64url segments joined by '.', whose leading segment decodes to a JOSE header; the fixture therefore asserts the token's SHAPE rather than its literal value (expected '<shaped-not-asserted>', which is per-run).
Emulator: InitiateAuth returns a single undelimited base64 blob containing no '.' segment separator at all, so it fails the recorded shape assertion. Hypothesis: the handler mints an opaque random-bytes token and base64-encodes it instead of building a JOSE compact serialization.
Why: any client, proxy or test that parses the refresh token structurally — splitting on '.', reading the header segment, or merely asserting the token looks like the token AWS vends — breaks against the emulator while passing against AWS. The token is also indistinguishable in form from the access/ID tokens, which AWS always serves dot-delimited.
Fix: the refresh-token minting/serialization step of the token issuer shared by the InitiateAuth handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/initiate_auth.rs and the token builder it calls). The structure of the emitted token is the requirement; the bytes inside it are per-run and are not.
Traces: cip-autherr-admin-reset-password-required#task#5 (cluster of 8: also cip-autherr-enabled-ghost-not-authorized#task#7, cip-autherr-enabled-wrong-password-matches-ghost#task#5, cip-autherr-legacy-ghost-user-not-found#task#7, cip-chal-mfa-setup-required#task#11, cognito-idp-account-recovery-and-attribute-verification#task#7, cognito-idp-self-service-account-deletion-lifecycle#setup#5).

Cause 2 of 3 — AdminInitiateAuth's AuthenticationResult.RefreshToken has the same wrong shape.
AWS: Smithy: AdminInitiateAuth output AuthenticationResult -> the same AuthenticationResultType.RefreshToken / TokenModelType @pattern ^[A-Za-z0-9-_=.]+$. Recorded behavior only: the admin flow vends a refresh token of the same dot-delimited JOSE compact form as the user flow.
Emulator: AdminInitiateAuth likewise returns one undelimited base64 blob with no '.' separators, failing the recorded shape assertion.
Why: same client breakage as cause 1, on the admin authentication path. It is reported as a separate cluster because it is a separate response serializer — fixing only the InitiateAuth path leaves the admin path diverging.
Fix: the admin authentication handler's token path (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/admin_initiate_auth.rs); if both handlers already share one token builder, the single fix must be confirmed on BOTH fixtures below.
Traces: cip-cfg-case-insensitive-username-resolution#task#9 (cluster of 4: also cip-cfg-revoke-token-disabled-on-client#task#5, cip-cfg-revoke-token-rejects-access-token#task#5, cognito-idp-admin-user-auth-workflow#task#7).

Cause 3 of 3 — InitiateAuth never issues the MFA_SETUP challenge; it returns a bare 200 with no challenge at all.
AWS: Smithy: ChallengeNameType includes MFA_SETUP; InitiateAuth output members ChallengeName, Session (SessionType, @length 20..4096) and ChallengeParameters (map<string,string>). When the pool requires MFA and the authenticating user has no enrolled factor, the password flow does not complete: the response carries ChallengeName='MFA_SETUP', a Session to carry into AssociateSoftwareToken/RespondToAuthChallenge, and ChallengeParameters containing MFAS_CAN_SETUP — a JSON-array string of the factor names the pool allows the user to enrol, e.g. ["SOFTWARE_TOKEN_MFA"] — together with USER_ID_FOR_SRP set to the request's resolved username. The fixed text here is the key names (ChallengeName, Session, MFAS_CAN_SETUP, USER_ID_FOR_SRP) and the enum value MFA_SETUP; the username in USER_ID_FOR_SRP and the factor list in MFAS_CAN_SETUP are values taken from the request and the pool's MFA configuration, not constants.
Emulator: status 200 with ChallengeName absent (None), Session absent, and no ChallengeParameters entries — i.e. no challenge is raised and the sign-in is treated as complete.
Why: two failures for a client. First, the entire TOTP enrolment-on-first-sign-in flow is unreachable: there is no Session to pass to AssociateSoftwareToken, so a user can never enrol a factor. Second, and worse, a pool configured to require MFA authenticates the user without one — code that relies on Cognito to gate sign-in behind a second factor is silently unprotected against the emulator.
Fix: the challenge-selection step of the InitiateAuth handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/initiate_auth.rs) must consult the pool's MfaConfiguration and the user's enrolled factors before emitting AuthenticationResult, and emit the MFA_SETUP challenge (with Session and ChallengeParameters) instead.
Traces: cip-chal-mfa-setup-required#task#7.

## Evidence
Verbatim from the wire replay (trace set simparity-wire-aws-v1):

Cause 1 — fixture cip-autherr-admin-reset-password-required, step task#5, InitiateAuth, expected status 200, actual status 200:
  '$.AuthenticationResult.RefreshToken: value_mismatch (expected '<shaped-not-asserted>', got 'Zit2SzR2T0hpL3VYWG05MFJ0UitMZVhGLzdNUERXaUhGOHpVRTh6a3dHRmZYZ2IyblgvMGdkQUROZ29SbVBRVGZDeFpWMVduYjREeEpiUnJlZFBTL2lkLyt1ZjVyQnM1ZTJsUVRGRC9NMlVCdXVCaVQwb3MrR0lISnRjN3NpcEk4ZUhYMzBERVBJTGxUbHZlRjMzSjN5a01qbjNiN0lYcSsvaDVxL2I0YTlFR3lCZTZaaHhNZ3B2NVNheHc0bUZ4RFRSdXFWa3VHQ09mZ1g1OVJJUVpyRUtQd2RDek16RkJRVGdBY0J6YnhyV0FiVXVlVWM0VjlKTzZJVnJYRVhEMXZxcy9...')'
  divergence_key: service=cognito-idp|InitiateAuth|status=200->200|keys=$.AuthenticationResult.RefreshToken ; affected_count 8.
  The emulator value is reproduced only to show it carries NO '.' separator — the byte content itself is per-run and is not a requirement.

Cause 2 — fixture cip-cfg-case-insensitive-username-resolution, step task#9, AdminInitiateAuth, expected status 200, actual status 200:
  '$.AuthenticationResult.RefreshToken: value_mismatch (expected '<shaped-not-asserted>', got 'RXBSdnZQMENlL3V4Tzg4ZUpUS2U2SlNPMGdVYno4enRCOFhTV1hXbVZpaENyVXRxV0FyNm81RW9Fa1A2SHhQWFdrSENubllTYlp4QU9Ydmt4aDk3RHBWMXJhQ08zOUd1OVlYQy9GWHgzdHZiQ2FHR3RSOWo5YTJmOHI5NFp3djB3TGZuSXBYNFJ5VUNuZ3FkQkFaQUxTZHcv...')'
  divergence_key: service=cognito-idp|AdminInitiateAuth|status=200->200|keys=$.AuthenticationResult.RefreshToken ; affected_count 4.

Cause 3 — fixture cip-chal-mfa-setup-required, step task#7, InitiateAuth, expected status 200, actual status 200:
  '$.ChallengeName: missing (expected 'MFA_SETUP', got None)'
  '$.ChallengeParameters.MFAS_CAN_SETUP: missing (expected '["SOFTWARE_TOKEN_MFA"]', got None)'
  '$.ChallengeParameters.USER_ID_FOR_SRP: missing (expected 'sim2real-probe-cipchal-user-7f7f344a', got None)'  <- the username is the request's value filling the template, not a constant
  '$.Session: missing (expected '<shaped-not-asserted>', got None)'
  divergence_key: service=cognito-idp|InitiateAuth|status=200->200|keys=$.ChallengeName,$.ChallengeParameters.MFAS_CAN_SETUP,$.ChallengeParameters.USER_ID_FOR_SRP,$.Session ; affected_count 1.

Model citations: Smithy AuthenticationResultType.RefreshToken -> TokenModelType, @pattern ^[A-Za-z0-9-_=.]+$, @sensitive. Smithy ChallengeNameType values include MFA_SETUP. Smithy InitiateAuth/AdminInitiateAuth output = {ChallengeName, Session (@length 20..4096), ChallengeParameters, AuthenticationResult, AvailableChallenges}.

## Steps to Reproduce
Cause 1: replay fixture cip-autherr-admin-reset-password-required and compare step task#5 — CreateUserPool + CreateUserPoolClient with ALLOW_USER_PASSWORD_AUTH, create and confirm a user, then InitiateAuth(AuthFlow=USER_PASSWORD_AUTH) and inspect AuthenticationResult.RefreshToken: it must have AWS's JOSE compact shape (base64url segments joined by '.'), not a single undelimited base64 blob.

Cause 2: replay fixture cip-cfg-case-insensitive-username-resolution and compare step task#9 — same pool/client setup, then AdminInitiateAuth(AuthFlow=ADMIN_USER_PASSWORD_AUTH) and apply the same shape check to AuthenticationResult.RefreshToken.

Cause 3: replay fixture cip-chal-mfa-setup-required and compare step task#7 — create a pool whose MFA is required with SOFTWARE_TOKEN_MFA available, create a confirmed user with NO enrolled factor, then InitiateAuth(AuthFlow=USER_PASSWORD_AUTH) with the correct password. Expect HTTP 200 carrying ChallengeName='MFA_SETUP', a Session (20..4096 chars), ChallengeParameters.MFAS_CAN_SETUP listing the enrollable factors as a JSON-array string and ChallengeParameters.USER_ID_FOR_SRP equal to the resolved username — and NO AuthenticationResult.

All three steps are in trace set simparity-wire-aws-v1; re-replay all three fixtures together to confirm the bundle is closed.
