Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-identity-identity-pool-roles-workflow
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-identity-identity-pool-roles-workflow cogid-creds-role-mapping-empty-rules-list cogid-creds-role-mapping-match-type-out-of-enum cogid-creds-role-mapping-multi-rule-missing-ambiguous-resolution cogid-creds-role-mapping-rules-without-rules-configuration cogid-login-delete-identities-batch-bounds cognito-identity-validation-error-boundaries cognito-identity-principal-tag-attribute-mapping cognito-identity-principal-tag-attribute-mapping-configuration cognito-identity-identity-pool-tagging-workflow cognito-identity-resource-tagging-round-trip cognito-identity-tag-operations-on-missing-or-malformed-resource cognito-identity-developer-authenticated-identity-merge-and-unlink
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->4xx|$.__type
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->4xx|$.message
Issue-Key: cognito-identity|SetIdentityPoolRoles|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|DeleteIdentities|4xx->4xx|$.message
Issue-Key: cognito-identity|DeleteIdentities|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|4xx->4xx|$.message
Issue-Key: cognito-identity|SetPrincipalTagAttributeMap|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-identity|TagResource|4xx->4xx|$.message
Issue-Key: cognito-identity|UntagResource|4xx->4xx|$.message
Issue-Key: cognito-identity|GetOpenIdTokenForDeveloperIdentity|4xx->4xx|$.message
Issue-Related: 20261029T230931Z-cognito-identity-error-responses-omit-the-x-amzn.md
Issue-Related: 20261029T230931Z-cognito-identity-pool-scoped-not-found-refusals-answer.md
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-identity: constraint-violation refusals are re-worded and mis-classified

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetIdentityPoolRoles

## Description
This issue bundles 7 root causes in which the emulator refuses the request at the right place but words the refusal its own way (and in one case uses the wrong error code). The AWS message is a requirement: AWS's constraint-violation text is the SDK framework's fixed template "<N> validation error(s) detected: Value '<the rejected value, rendered>' at '<the input path>' failed to satisfy constraint: <the constraint sentence>" — the fixed text is the template, and the variable parts are the rejected value, the member path and the limit, all taken from the request and the model. The emulator substitutes a SUMMARY of the value ('0 items', '51 entries', a count) where AWS renders the value itself, and in several places replaces the template with free prose. Fix: the request-validation layer shared by these operations (value rendering + error class selection), and for causes 5-7 the named handler's message text.

Cause 1 of 7: SetIdentityPoolRoles with a Roles map key outside the modeled pattern. AWS: `__type` ValidationException, HTTP 400, message "1 validation error detected: Value '{<key>=<role-arn>}' at 'roles' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: (un)?authenticated]" — fixed text as shown, with the offending map rendered from the request. Emulator: `__type` InvalidParameterException and prose "Roles key '<key>' is not valid: the key must be one of authenticated, unauthenticated." Why: the error CODE is what clients branch on; ValidationException and InvalidParameterException are different classes, so retry/handling logic diverges, and no client matching AWS's text matches. Fix: validation layer (constraint failure => ValidationException with the framework template) in the SetIdentityPoolRoles input validation. Traces: cognito-identity-identity-pool-roles-workflow#task#8.

Cause 2 of 7: SetIdentityPoolRoles with an empty RulesConfiguration.Rules list. AWS: 400, message "1 validation error detected: Value '[]' at 'roleMappings.<provider>.member.rulesConfiguration.rules' failed to satisfy constraint: Member must have length greater than or equal to 1" — the rejected value is rendered as the empty list '[]'. Emulator: same template but renders the value as '0 items'. Why: the message is a requirement; a client or test matching AWS's rendering fails. Fix: value rendering in the validation layer (render the list, not its cardinality). Traces: cogid-creds-role-mapping-empty-rules-list#task#4 (4 steps).

Cause 3 of 7: DeleteIdentities with more IdentityIdsToDelete than the modeled maximum. AWS: 400 with the template rendering the full list of submitted identity ids inside Value '[...]' at 'identityIdsToDelete'. Emulator: renders a summary instead of the list. Why/Fix: as cause 2 (same rendering defect, different member). Traces: cogid-login-delete-identities-batch-bounds#task#3 (3 steps).

Cause 4 of 7: SetPrincipalTagAttributeMap with more than 50 principal tags. Smithy: PrincipalTags is a map with @length(min 0, max 50). AWS: 400, message "1 validation error detected: Value '{<every key=value pair of the submitted map>}' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50". Emulator: "1 validation error detected: Value '51 entries' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50" — the limit and path are right, the rendered value is a count. Why/Fix: as cause 2. Traces: cognito-identity-principal-tag-attribute-mapping#task#6 (2 steps).

Cause 5 of 7: TagResource with a tag key in the reserved 'aws:' namespace. AWS: 400, `x-amzn-errortype: InvalidParameterException:`, message fixed text 'Caller is an end user and not allowed to mutate system tags'. Emulator: prose "Tags key '<key>' is not valid: the 'aws:' prefix is reserved for Amazon Web Services use." and no errortype header. Why: the message is a requirement and AWS's text has no variable part here. Fix: TagResource handler's reserved-prefix refusal message. Traces: cognito-identity-identity-pool-tagging-workflow#task#7.

Cause 6 of 7: UntagResource with a ResourceArn that is not an ARN. AWS: 400, message beginning with the fixed text 'ARN in SecureServiceAuthStrategy failed validation:'. Emulator: "ResourceArn '<value>' is not the ARN of an Amazon Cognito identity pool." Why/Fix: as cause 5, in the UntagResource/ARN-parsing refusal path. Traces: cognito-identity-resource-tagging-round-trip#task#7 (2 steps).

Cause 7 of 7: GetOpenIdTokenForDeveloperIdentity against a pool that has no DeveloperProviderName configured. AWS: 400, `x-amzn-errortype: InvalidParameterException:`, message fixed text 'This identity pool does not support the specified developer provider.' Emulator: "Invalid value for parameter Logins: it must carry an entry for the developer provider '<provider>' of identity pool '<pool-id>'." plus no errortype header. Why: the refusal reason a client reads is different from AWS's. Fix: the GetOpenIdTokenForDeveloperIdentity handler's developer-provider check message. Traces: cognito-identity-developer-authenticated-identity-merge-and-unlink#task#10.

Related open issues (no replay keys, not covering these clusters): '20261023T185902Z-cognito-identity-input-validation-and-error-classification-diverge.md' and '20261029T173827Z-cognito-identity-8-net-new-validation-message-and.md'.

## Evidence
Replay of simparity-wire-aws-v1, expected vs actual:
- SetIdentityPoolRoles: $.__type expected 'ValidationException', got 'InvalidParameterException'; $.message expected "1 validation error detected: Value '{invalid-key=<role-arn>}' at 'roles' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: (un)?authenticated]", got "Roles key 'invalid-key' is not valid: the key must be one of authenticated, unauthenticated." (cognito-identity-identity-pool-roles-workflow#task#8).
- SetIdentityPoolRoles: $.message expected Value '[]' at 'roleMappings.<provider>.member.rulesConfiguration.rules', got Value '0 items' at the same path (cogid-creds-role-mapping-empty-rules-list#task#4, 4 steps).
- DeleteIdentities: $.message expected the full rendered id list at 'identityIdsToDelete', got a summary (cogid-login-delete-identities-batch-bounds#task#3, 3 steps).
- SetPrincipalTagAttributeMap: $.message expected Value '{tag0=sub, ...}' at 'principalTags' ... length less than or equal to 50, got Value '51 entries' at 'principalTags' ... (cognito-identity-principal-tag-attribute-mapping#task#6, 2 steps).
- TagResource: $.message expected 'Caller is an end user and not allowed to mutate system tags', got "Tags key 'aws:reserved' is not valid: the 'aws:' prefix is reserved for Amazon Web Services use."; x-amzn-errortype expected 'InvalidParameterException:', got None (cognito-identity-identity-pool-tagging-workflow#task#7).
- UntagResource: $.message expected 'ARN in SecureServiceAuthStrategy failed validation:', got "ResourceArn 'not-a-valid-identity-pool-arn' is not the ARN of an Amazon Cognito identity pool." (cognito-identity-resource-tagging-round-trip#task#7, 2 steps).
- GetOpenIdTokenForDeveloperIdentity: $.message expected 'This identity pool does not support the specified developer provider.', got "Invalid value for parameter Logins: it must carry an entry for the developer provider '<provider>' of identity pool '<pool-id>'."; x-amzn-errortype expected 'InvalidParameterException:', got None (cognito-identity-developer-authenticated-identity-merge-and-unlink#task#10).
Smithy: PrincipalTags @length(0,50); IdentityPool roles map keys constrained to (un)?authenticated; InvalidParameterException is a common error of every operation.

## Steps to Reproduce
1. Replay cognito-identity-identity-pool-roles-workflow step task#8: SetIdentityPoolRoles with Roles key 'invalid-key' -> emulator answers InvalidParameterException with its own prose; AWS answers ValidationException with the '1 validation error detected: ...' template.
2. Replay cogid-creds-role-mapping-empty-rules-list#task#4 (empty rules list), cogid-login-delete-identities-batch-bounds#task#3 (over-long id list) and cognito-identity-principal-tag-attribute-mapping#task#6 (51 principal tags) and compare the rendered Value '...' in the message.
3. Replay cognito-identity-identity-pool-tagging-workflow#task#7 (tag key 'aws:reserved'), cognito-identity-resource-tagging-round-trip#task#7 (malformed ResourceArn) and cognito-identity-developer-authenticated-identity-merge-and-unlink#task#10 (pool without a developer provider) and compare the message text and the x-amzn-errortype header.
