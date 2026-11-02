Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.distribution-caller-reference-change-rejected cloudfront.distribution-tenant-missing-required-tenant-path-rejected cloudfront.get-field-level-encryption-profile-type-confusion-rejected cloudfront.origin-access-control-unknown-signing-behavior-rejected cloudfront.update-origin-request-policy-inconsistent-quantities-fails cloudfront.update-origin-request-policy-inconsistent-quantities-rejected cloudfront.op-contract-get-field-level-encryption-config
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.distribution-caller-reference-change-rejected|run#13|AssertionError|assert message == "The caller reference cannot be modified.", (
Issue-Key: cloudfront.distribution-tenant-missing-required-tenant-path-rejected|run#10|AssertionError|assert message == expected_message, (
Issue-Key: cloudfront.get-field-level-encryption-profile-type-confusion-rejected|run#12|AssertionError|assert message == "Id format is invalid", (
Issue-Key: cloudfront.origin-access-control-unknown-signing-behavior-rejected|run#6|AssertionError|assert message == expected_message, f"unexpected MalformedXML message: {message!r}"
Issue-Key: cloudfront.update-origin-request-policy-inconsistent-quantities-fails|run#11|AssertionError|assert message == "The specified quantity of QueryStrings (2) does not match the actual quantity supplied (1).", (
Issue-Key: cloudfront.update-origin-request-policy-inconsistent-quantities-rejected|run#10|AssertionError|assert message == "The specified quantity of Headers (2) does not match the actual quantity supplied (1).", (
Issue-Key: cloudfront.op-contract-get-field-level-encryption-config|run#26|AssertionError|assert fwd_qa is True, f"expected ForwardWhenQueryArgProfileIsUnknown True, got {fwd_qa!r}"
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudfront: six message/representation divergences — IllegalUpdate, tenant-parameter, FLE not-found and enum-validation wording, InconsistentQuantities member path, and a dropped FLE config flag

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOriginRequestPolicy

## Description
Six net-new root causes where the emulator reaches the right outcome but the error CODE/MESSAGE or the serialized value is not AWS's. Related open issues that do NOT carry these replay keys: 20261029T212223Z-cloudfront-refusal-codes-and-messages-diverge-from-aws.md, 20261029T212223Z-cloudfront-duplicate-caller-reference-409-messages-do-not.md, 20261031T221122Z-cloudfront-refuses-well-formed-but-absent-distribution-tenant.md.

Cause 1 of 6: UpdateDistribution's IllegalUpdate for a changed CallerReference uses emulator wording.
AWS: changing DistributionConfig.CallerReference on an existing distribution is refused 400 IllegalUpdate (Smithy: UpdateDistribution error IllegalUpdate, httpError 400) with the message "The caller reference cannot be modified." — fixed text, no variable part.
Emulator: same code, message 'You cannot update the value of CallerReference.'
Why: clients and operators that match on the refusal text (and docs/runbooks quoting it) do not recognise the refusal.
Fix: the IllegalUpdate message for the immutable-CallerReference path in the cloudfront UpdateDistribution handler.
Traces: cloudfront.distribution-caller-reference-change-rejected.

Cause 2 of 6: CreateDistributionTenant's missing-parameter refusal uses emulator wording.
AWS: a tenant request that supplies no value for a parameter the multi-tenant distribution requires is refused with the message "A parameter referenced in the multi-tenant distribution configuration is missing. Specify the parameter and try again." — fixed text; it does NOT name the parameter.
Emulator: 'An argument is invalid: the parameter TenantPath is required by the distribution and the request supplies no value for it' — emulator-authored prose that splices in the member name.
Why: the message is neither AWS's text nor a stable shape to match on; the "An argument is invalid:" prefix is the error shape's documentation, not a runtime message.
Fix: the missing-parameter refusal message in the cloudfront CreateDistributionTenant handler's validation.
Traces: cloudfront.distribution-tenant-missing-required-tenant-path-rejected.

Cause 3 of 6: GetFieldLevelEncryptionProfile's cross-type refusal message is a not-found sentence instead of AWS's format refusal.
AWS: an Id that carries another resource type's shape (here a real PublicKey id) is refused on FORMAT, before lookup: 404 NoSuchFieldLevelEncryptionProfile (Smithy: GetFieldLevelEncryptionProfile error NoSuchFieldLevelEncryptionProfile, httpError 404) with the message "Id format is invalid" — fixed text.
Emulator: the same code with 'The specified field level encryption profile does not exist.' (the error shape's documentation sentence).
Why: a client cannot tell a malformed id from an absent profile, which is the whole point of the distinction AWS draws.
Fix: id-format validation ordering and message in the cloudfront GetFieldLevelEncryptionProfile handler.
Traces: cloudfront.get-field-level-encryption-profile-type-confusion-rejected.

Cause 4 of 6: CreateOriginAccessControl's enum-validation message lists the value set in the wrong order.
AWS: SigningBehavior outside the enum is refused MalformedXML with "1 validation error detected: Value '<supplied value>' at 'originAccessControlConfig.signingBehavior' failed to satisfy constraint: Member must satisfy enum value set: [always-amz-auth, never, always, no-override]" — fixed text with the supplied value and the member path as variables, and that exact value-set order.
Emulator: identical except the value set is rendered '[never, always, no-override, always-amz-auth]' (the model's declaration order; Smithy OriginAccessControlSigningBehaviors values: never, always, no-override, always-amz-auth).
Why: byte-for-byte message assertions and golden-file comparisons fail; the same defect class was already fixed once for ListConnectionFunctions (20261027T155218Z-cloudfront-listconnectionfunctions-enum-validation-message-lists-the-functionstage.md), so the ordering is not derived from the model.
Fix: the enum value-set ordering used to render validation messages in the cloudfront CreateOriginAccessControl handler's validation.
Traces: cloudfront.origin-access-control-unknown-signing-behavior-rejected.

Cause 5 of 6: UpdateOriginRequestPolicy's InconsistentQuantities message prefixes the container name onto the member.
AWS: a Quantity that disagrees with the number of Items is refused 400 InconsistentQuantities (Smithy: UpdateOriginRequestPolicy error InconsistentQuantities, httpError 400) with "The specified quantity of <member> (<declared quantity>) does not match the actual quantity supplied (<actual count>)." where <member> is the bare list member name — "QueryStrings", "Headers" — and the two numbers come from the request.
Emulator: the member is rendered as the dotted container path: 'The specified quantity of QueryStringsConfig.QueryStrings (2) does not match the actual quantity supplied (1).' and 'The specified quantity of HeadersConfig.Headers (2) does not match the actual quantity supplied (1).'
Why: the message no longer matches AWS for any member, so clients keying on it (and operators reading it) see an unfamiliar member name.
Fix: the InconsistentQuantities message rendering (member naming) in the cloudfront UpdateOriginRequestPolicy handler's validation.
Traces: cloudfront.update-origin-request-policy-inconsistent-quantities-fails, cloudfront.update-origin-request-policy-inconsistent-quantities-rejected.

Cause 6 of 6: GetFieldLevelEncryptionConfig does not round-trip ForwardWhenQueryArgProfileIsUnknown.
AWS: QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown is a required boolean (Smithy: QueryArgProfileConfig member ForwardWhenQueryArgProfileIsUnknown, required, boolean) and GetFieldLevelEncryptionConfig returns the value the config was created with.
Emulator: a config created with ForwardWhenQueryArgProfileIsUnknown=true reads back False.
Why: a client reading its own configuration sees field-level-encryption forwarding disabled when it is enabled — a silent behavioural inversion on the request path. The resolved issue 20261029T212223Z-cloudfront-does-not-preserve-request-supplied-list-order.md names the same class but does not cover this replay key.
Fix: FLE config persistence/serialization of QueryArgProfileConfig in the cloudfront GetFieldLevelEncryptionConfig handler and the FLE config state.
Traces: cloudfront.op-contract-get-field-level-encryption-config.

## Evidence
Cause 1: "AssertionError: UpdateDistribution IllegalUpdate message: 'You cannot update the value of CallerReference.'" — expected "The caller reference cannot be modified."
Cause 2: "AssertionError: expected message 'A parameter referenced in the multi-tenant distribution configuration is missing. Specify the parameter and try again.', got 'An argument is invalid: the parameter TenantPath is required by the distribution and the request supplies no value for it'".
Cause 3: "AssertionError: expected AWS's exact message, got 'The specified field level encryption profile does not exist.'" — expected "Id format is invalid" under code NoSuchFieldLevelEncryptionProfile.
Cause 4: "AssertionError: unexpected MalformedXML message: \"1 validation error detected: Value 'sometimes' at 'originAccessControlConfig.signingBehavior' failed to satisfy constraint: Member must satisfy enum value set: [never, always, no-override, always-amz-auth]\"" — expected the set rendered [always-amz-auth, never, always, no-override].
Cause 5: "AssertionError: InconsistentQuantities message: 'The specified quantity of QueryStringsConfig.QueryStrings (2) does not match the actual quantity supplied (1).'" and "'The specified quantity of HeadersConfig.Headers (2) does not match the actual quantity supplied (1).'" — expected "QueryStrings" / "Headers".
Cause 6: "AssertionError: expected ForwardWhenQueryArgProfileIsUnknown True, got False".

## Steps to Reproduce
1. Cause 1: create a distribution, then UpdateDistribution with a config whose CallerReference differs -> expect 400 IllegalUpdate "The caller reference cannot be modified."
2. Cause 2: create a tenant-only distribution declaring a required parameter, then CreateDistributionTenant without a value for it -> expect AWS's missing-parameter message.
3. Cause 3: CreatePublicKey, then GetFieldLevelEncryptionProfile with that PublicKey id -> expect 404 NoSuchFieldLevelEncryptionProfile with message "Id format is invalid".
4. Cause 4: CreateOriginAccessControl with SigningBehavior="sometimes" -> expect MalformedXML whose enum value set reads [always-amz-auth, never, always, no-override].
5. Cause 5: UpdateOriginRequestPolicy with QueryStringsConfig.QueryStrings Quantity=2 and one item (and separately HeadersConfig.Headers Quantity=2 with one item) -> expect the message to name "QueryStrings" / "Headers".
6. Cause 6: CreateFieldLevelEncryptionConfig with QueryArgProfileConfig.ForwardWhenQueryArgProfileIsUnknown=true, then GetFieldLevelEncryptionConfig -> expect true.
