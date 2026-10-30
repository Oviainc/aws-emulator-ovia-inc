Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-managed-login-branding-error-paths
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-managed-login-branding-error-paths cognito-idp-user-pool-domain-errors cognito-idp-user-pool-replica-errors cognito-idp-user-pool-replica-notfound-errors
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateManagedLoginBranding|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateUserPoolDomain|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->4xx|$.message
Issue-Commit: c16f2fd57a71cf90620ee867de88f472739a2612

# cognito-idp: branding and user-pool-not-found refusal messages use emulator prose instead of AWS wording (3 causes)

> _Note: this issue was identified against the code base at commit `c16f2fd57a71cf90620ee867de88f472739a2612`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateManagedLoginBranding

## Description
Three cognito-idp refusals return the correct HTTP status and the correct modeled error code, but the `message` field is emulator-authored prose rather than the sentence AWS emits for that error class. Clients and SDK integration tests that assert on refusal text (and humans reading logs) see wording that exists nowhere in AWS. The three causes share one root: the message templates used when building these exceptions were written from scratch instead of reproducing AWS's fixed text with the request's values interpolated.

Cause 1 of 3: CreateManagedLoginBranding — mutually-exclusive-argument refusal message.
Fixture cognito-idp-managed-login-branding-error-paths step task#6.
AWS: CreateManagedLoginBranding takes UseCognitoProvidedValues (BooleanType, @default false), Settings (document) and Assets (AssetListType, @length 0..40); supplying UseCognitoProvidedValues=true together with Settings/Assets is refused with InvalidParameterException (Smithy: CreateManagedLoginBranding errors include InvalidParameterException, errorKind client, httpError 400). The model does not state the message text, so this is recorded behavior only: the recorded response is HTTP 400 with message `useCognitoProvidedValues or settings should be specified (but not both)`. That whole sentence is fixed text — it contains no request- or state-derived value, and it refers to the members in lowerCamelCase (`useCognitoProvidedValues`, `settings`), not the wire-cased member names.
Emulator: answers HTTP 400 with message `Settings and Assets must be omitted when UseCognitoProvidedValues is true.` — a different, invented sentence (different wording, different member casing, trailing period, and it names Assets which AWS's sentence does not).
Why: a client that branches on or asserts the InvalidParameterException message for this mutually-exclusive-argument case never matches, and a user following AWS's wording to locate the offending field is pointed at a sentence AWS never sends.
Fix: the message template in CreateManagedLoginBranding's input validation (the mutually-exclusive UseCognitoProvidedValues/Settings check), not the status or error class, which are already correct.

Cause 2 of 3: CreateUserPoolDomain — unknown-user-pool refusal message.
Fixture cognito-idp-user-pool-domain-errors step task#5.
AWS: CreateUserPoolDomain requires UserPoolId (UserPoolIdType, @length 1..55, @pattern ^[\w-]+_[0-9a-zA-Z]+$) and refuses an id that names no pool with ResourceNotFoundException (Smithy: CreateUserPoolDomain errors include ResourceNotFoundException, errorKind client). The model does not carry the message text, so recorded behavior only: the recorded response's message is `UserPool with ID: <UserPoolId>does not exist.` — fixed text `UserPool with ID: ` and `does not exist.` with exactly one variable part, the UserPoolId copied verbatim from the request, concatenated with no separating space before `does` (AWS emits no space there). In the recording UserPoolId happens to be us-west-2_ZZZZZZZZZZ; that value is the request's, not part of the template.
Emulator: answers the same status/error code but with message `User pool <UserPoolId> does not exist.` — `User pool` instead of `UserPool with ID:` and a space inserted before `does`.
Why: the message is the only part of this refusal that identifies which pool was rejected, and a client matching AWS's sentence (or a snapshot test over it) fails against the emulator even though the pool really was absent.
Fix: the not-found message template used by CreateUserPoolDomain's handler/validation when resolving UserPoolId, shared with cause 3.

Cause 3 of 3: CreateUserPoolReplica — unknown-user-pool refusal message.
Fixture cognito-idp-user-pool-replica-errors step task#2 (also cognito-idp-user-pool-replica-notfound-errors step task#1).
AWS: CreateUserPoolReplica requires UserPoolId (same UserPoolIdType constraints) and RegionName, and refuses an unknown pool with ResourceNotFoundException (Smithy: CreateUserPoolReplica errors include ResourceNotFoundException, errorKind client). Recorded behavior only for the text: the same AWS template as cause 2 — fixed `UserPool with ID: ` + the request's UserPoolId + `does not exist.`, with no space before `does`.
Emulator: answers `User pool <UserPoolId> does not exist.`, the same emulator-invented template as cause 2.
Why: identical client impact to cause 2, on a second operation — so the divergence is systematic across the pool-resolution path rather than one handler's typo.
Fix: the shared user-pool-not-found message template (the pool-lookup helper that constructs ResourceNotFoundException) so every operation resolving a UserPoolId emits AWS's sentence; CreateUserPoolDomain and CreateUserPoolReplica are the two instances the corpus exercises.

Related open issues that do NOT cover these steps: 20261029T184956Z-cognito-idp-ten-not-found-refusal-messages-diverge.md, 20261029T182116Z-cognito-idp-not-found-and-duplicate-resource-error.md and 20261029T182116Z-cognito-idp-input-validation-refusals-carry-the-wrong.md describe the same family of wording defects but carry no replay key for these three clusters, which replay reports as unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 — cognito-idp-managed-login-branding-error-paths#task#6, CreateManagedLoginBranding, verdict mismatch, expected_status 400 / actual_status 400:
  $.message: value_mismatch (expected 'useCognitoProvidedValues or settings should be specified (but not both)', got 'Settings and Assets must be omitted when UseCognitoProvidedValues is true.')
  divergence_key: service=cognito-idp|CreateManagedLoginBranding|status=400->400|keys=$.message
  affected_count 1.

Cause 2 — cognito-idp-user-pool-domain-errors#task#5, CreateUserPoolDomain, verdict mismatch, expected_status 400 / actual_status 400:
  $.message: value_mismatch (expected 'UserPool with ID: us-west-2_ZZZZZZZZZZdoes not exist.', got 'User pool us-west-2_ZZZZZZZZZZ does not exist.')
  divergence_key: service=cognito-idp|CreateUserPoolDomain|status=400->400|keys=$.message
  affected_count 1. (us-west-2_ZZZZZZZZZZ is the request's UserPoolId, not a required literal.)

Cause 3 — cognito-idp-user-pool-replica-errors#task#2, CreateUserPoolReplica, verdict mismatch, expected_status 400 / actual_status 400:
  $.message: value_mismatch (expected 'UserPool with ID: us-west-2_ZZZZZZZZZZdoes not exist.', got 'User pool us-west-2_ZZZZZZZZZZ does not exist.')
  divergence_key: service=cognito-idp|CreateUserPoolReplica|status=400->400|keys=$.message
  affected_count 2; other member: cognito-idp-user-pool-replica-notfound-errors#task#1.

In all three the HTTP status and the serialized error code already agree with the recording; only the message text diverges.

Model grounding (Smithy, com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice):
  CreateManagedLoginBranding: input UseCognitoProvidedValues BooleanType @default false, Settings Document, Assets AssetListType @length 0..40; errors include InvalidParameterException (client, httpError 400).
  CreateUserPoolDomain: input UserPoolId UserPoolIdType @required @length 1..55 @pattern ^[\w-]+_[0-9a-zA-Z]+$; errors include ResourceNotFoundException (client).
  CreateUserPoolReplica: input UserPoolId UserPoolIdType @required, RegionName @required @length 5..32; errors include ResourceNotFoundException (client).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay fixture cognito-idp-managed-login-branding-error-paths, step task#6 — CreateManagedLoginBranding against an existing pool/client with UseCognitoProvidedValues=true AND Settings (and/or Assets) supplied. Expect HTTP 400 InvalidParameterException with message 'useCognitoProvidedValues or settings should be specified (but not both)'; observe message 'Settings and Assets must be omitted when UseCognitoProvidedValues is true.'
4. Cause 2: replay fixture cognito-idp-user-pool-domain-errors, step task#5 — CreateUserPoolDomain with a well-formed but nonexistent UserPoolId. Expect HTTP 400 ResourceNotFoundException with message 'UserPool with ID: ' + that UserPoolId + 'does not exist.' (no space before 'does'); observe 'User pool ' + UserPoolId + ' does not exist.'
5. Cause 3: replay fixture cognito-idp-user-pool-replica-errors, step task#2 (and cognito-idp-user-pool-replica-notfound-errors, step task#1) — CreateUserPoolReplica with a nonexistent UserPoolId and a valid RegionName. Same expected vs observed message templates as step 4.
6. After fixing, re-replay all three fixture/step pairs plus cognito-idp-user-pool-replica-notfound-errors#task#1 and confirm $.message matches.
