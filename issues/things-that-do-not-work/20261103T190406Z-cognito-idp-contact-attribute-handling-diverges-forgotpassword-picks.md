Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-acr-traj-forgot-password-falls-to-email
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-acr-traj-forgot-password-falls-to-email cip-autherr-forgot-password-ghost-enabled-fabricated cip-autherr-forgot-password-ghost-legacy-not-found cognito-idp-attribute-management-edge-cases-and-invalid-input
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|ForgotPassword|2xx->2xx|$.CodeDeliveryDetails.Destination
Issue-Key: cognito-idp|UpdateUserAttributes|4xx->2xx|$
Issue-Related: 20261029T184956Z-cognito-idp-ten-request-validation-and-precondition-gates.md
Issue-Related: 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md
Issue-Commit: 17c02f8ec3af0127c9a5240741027d05fc85e076

# cognito-idp: contact-attribute handling diverges — ForgotPassword picks the wrong code-delivery destination and UpdateUserAttributes accepts a malformed phone number (3 causes)

> _Note: this issue was identified against the code base at commit `17c02f8ec3af0127c9a5240741027d05fc85e076`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
Three net-new wire divergences in how cognito-idp handles a user's verified contact attributes (email / phone_number): which one ForgotPassword selects as the code-delivery destination, how a fabricated destination is rendered for a non-existent user, and whether UpdateUserAttributes validates a phone number at all. All three are cognito-idp and all three must be closed before this issue is submitted.

Cause 1 of 3 — ForgotPassword ignores the user pool's AccountRecoverySetting fallback and always delivers to the highest-priority mechanism (operation: ForgotPassword).
AWS: Smithy: ForgotPassword output CodeDeliveryDetails -> CodeDeliveryDetailsType {Destination, DeliveryMedium (enum DeliveryMediumType = SMS|EMAIL), AttributeName}. The pool's AccountRecoverySettingType.RecoveryMechanisms is an ordered, priority-ranked list of RecoveryOptionType {Priority 1..2, Name in [verified_email, verified_phone_number, admin_only]}, documented as: "if verified_email has a priority of 1 and verified_phone_number has a priority of 2, your user pool sends account-recovery messages to a verified email address but falls back to an SMS message if the user has a verified phone number." So the recovery mechanism is chosen by priority but only among mechanisms the user can actually satisfy — a priority-1 mechanism whose underlying attribute is absent or unverified is skipped and the next priority is used. The recorded response for this step is HTTP 200 with CodeDeliveryDetails.AttributeName = "email", DeliveryMedium = "EMAIL" and a masked email Destination.
Emulator: answers HTTP 200 but with AttributeName = "phone_number", DeliveryMedium = "SMS" and a masked phone Destination — i.e. it resolves the pool's priority-1 recovery mechanism unconditionally and never falls back to the next priority when the priority-1 attribute is not a usable verified attribute for that user.
Why: a client that reads DeliveryMedium/AttributeName to tell the user where the reset code went instructs them to check SMS when AWS sent email (and vice versa), and a client that branches on DeliveryMedium to pick the next call in the recovery flow takes the wrong branch. The pool-level AccountRecoverySetting a tenant configured has no observable effect, so account-recovery routing cannot be tested against the emulator.
Fix: the cognito-idp ForgotPassword handler's recovery-mechanism selection — it must walk RecoveryMechanisms in Priority order and skip any mechanism whose attribute is missing/unverified for the resolved user before emitting CodeDeliveryDetails; the same selection feeds AttributeName, DeliveryMedium and Destination.
Traces: cognito-idp-acr-traj-forgot-password-falls-to-email#task#11.

Cause 2 of 3 — the fabricated CodeDeliveryDetails.Destination served for a non-existent user is a constant placeholder instead of being derived from the request (operation: ForgotPassword).
AWS: recorded behavior only (the Smithy model documents Destination merely as "The email address or phone number destination where Amazon Cognito sent the code" and states no derivation rule). The recordings are ForgotPassword calls for usernames that do not exist in the pool, where Cognito suppresses user-existence disclosure and still answers HTTP 200 with a CodeDeliveryDetails whose Destination is a masked address in the form <first-char>***@<first-domain-char>***. The recorded masked destination varies with the request's username (e.g. 's***@y***' for one ghost username), i.e. the masking pattern is the fixed part and the characters inside it are values taken from the request/state.
Emulator: answers HTTP 200 with the same masking shape but always the same characters, 'a***@b***', for every ghost username — a hard-coded placeholder rather than a value derived from the request.
Why: the whole point of the fabricated reply is that it is indistinguishable from a real one. A constant 'a***@b***' makes the non-existent-user path trivially distinguishable from the real path (a user-enumeration oracle), and any client or test that compares the masked destination it displays against the address it submitted sees a value unrelated to the request. Hypothesis: AWS derives the fabricated destination from the submitted Username (masked) rather than from pool state; what is required regardless is that the characters inside the mask come from the request/state, not from a literal.
Fix: the cognito-idp ForgotPassword handler's code-delivery-destination fabrication for the user-not-found path, plus the shared destination-masking helper it calls.
Traces: cip-autherr-forgot-password-ghost-enabled-fabricated#task#4, cip-autherr-forgot-password-ghost-legacy-not-found#task#4.

Cause 3 of 3 — UpdateUserAttributes accepts a malformed phone_number instead of refusing it (operation: UpdateUserAttributes).
AWS: Smithy: UpdateUserAttributes models InvalidParameterException (errorKind client, httpError 400) among its errors. The recorded response is HTTP 400 with body {"__type": "InvalidParameterException", "message": "Invalid phone number format."} and header x-amzn-errortype: "InvalidParameterException:". That message is entirely fixed text — it has no variable parts, so it must be reproduced verbatim.
Emulator: answers HTTP 200 with an empty JSON body ({}), emits no __type, no message and no x-amzn-errortype header, and stores the malformed value: the phone_number format gate is simply absent from the self-service attribute-update path.
Why: a client that relies on AWS refusing a badly formatted phone number never sees the refusal, so its error handling is never exercised and the pool is left holding an unusable phone_number — which then silently breaks every downstream SMS path (SMS MFA, SMS account recovery) that assumes a well-formed E.164 number. Note the missing x-amzn-errortype header here is the same general gap already tracked by the open issue "cognito-idp: every error response omits the x-amzn-errortype header" (20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md); that issue does not cover this step, because here the refusal is not produced at all. The nearby open issue "cognito-idp: nine validation/enforcement gates missing" (20261103T075017Z) likewise does not carry this step's replay key.
Fix: the cognito-idp UpdateUserAttributes handler's request validation — the phone_number format check must run before the attribute is written and raise InvalidParameterException (HTTP 400) with AWS's wording.
Traces: cognito-idp-attribute-management-edge-cases-and-invalid-input#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (193 fixtures, 1742 steps, score 85.0%, emulator_up=true, bootstrap_signal=false). The three clusters below carry no stub_path and no owned_by, i.e. no filed issue holds their replay keys.

Cause 1 — fixture cognito-idp-acr-traj-forgot-password-falls-to-email, step task#11, ForgotPassword, status 200 -> 200:
  $.CodeDeliveryDetails.AttributeName: expected 'email', got 'phone_number'
  $.CodeDeliveryDetails.DeliveryMedium: expected 'EMAIL', got 'SMS'
  $.CodeDeliveryDetails.Destination: expected 's***@e***', got '+*******0100'
  (affected_count 1)

Cause 2 — fixture cip-autherr-forgot-password-ghost-enabled-fabricated, step task#4, ForgotPassword, status 200 -> 200:
  $.CodeDeliveryDetails.Destination: expected 's***@y***', got 'a***@b***'
  (affected_count 2; also cip-autherr-forgot-password-ghost-legacy-not-found#task#4)

Cause 3 — fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step task#10, UpdateUserAttributes:
  status: expected 400, got 200
  $: empty_body (expected {'__type': 'InvalidParameterException', 'message': 'Invalid phone number format.'}, got {})
  $headers.x-amzn-errortype: expected 'InvalidParameterException:', got None
  (affected_count 1)

## Steps to Reproduce
1. git clone the project repo and build/boot the emulator from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 1: replay fixture cognito-idp-acr-traj-forgot-password-falls-to-email up to step task#11 — a pool whose AccountRecoverySetting ranks a phone mechanism ahead of email, for a user that cannot satisfy the phone mechanism; call ForgotPassword and inspect CodeDeliveryDetails.{AttributeName,DeliveryMedium,Destination}. Expect email/EMAIL/masked-email; the emulator returns phone_number/SMS/masked-phone.
4. Cause 2: replay fixture cip-autherr-forgot-password-ghost-enabled-fabricated step task#4 (and cip-autherr-forgot-password-ghost-legacy-not-found step task#4) — call ForgotPassword with a username that does not exist in the pool and inspect CodeDeliveryDetails.Destination. The emulator returns the constant 'a***@b***' for every such username instead of a masked destination derived from the request.
5. Cause 3: replay fixture cognito-idp-attribute-management-edge-cases-and-invalid-input step task#10 — call UpdateUserAttributes with a malformed phone_number value. Expect HTTP 400, __type InvalidParameterException, message 'Invalid phone number format.' and header x-amzn-errortype 'InvalidParameterException:'; the emulator answers HTTP 200 with an empty body and persists the value.
