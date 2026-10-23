Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-managed-rule-set-vendor-operations-on-fresh-account
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-managed-rule-set-vendor-operations-on-fresh-account wafv2-api-key-lifecycle-for-captcha-integration
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.Field
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.Parameter
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.Reason
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.__type
Issue-Key: wafv2|PutManagedRuleSetVersions|4xx->4xx|$.message
Issue-Key: wafv2|CreateAPIKey|4xx->4xx|$.message
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# WAFv2 parameter validation returns ValidationException instead of WAFInvalidParameterException

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutManagedRuleSetVersions

## Description
This bundle has 2 causes. Both are the same emulator defect: wafv2 input validation is performed by a generic Smithy-constraint validator that emits an unmodeled ValidationException-shaped body, instead of WAFv2's own WAFInvalidParameterException with its Field/Parameter/Reason members, and it validates members in a different order than AWS does so a different member is reported as the offender.

Cause 1 of 2: PutManagedRuleSetVersions with a non-existent rule-group ARN and a malformed LockToken. AWS: 400 with __type WAFInvalidParameterException and the structured members the model defines -- Smithy WAFInvalidParameterException members: message, Field (enum ParameterExceptionField), Parameter (length min 1), Reason (ErrorReason); PutManagedRuleSetVersions models WAFInvalidParameterException at httpError 400. The recording shows AWS reports the resource ARN as the offending parameter: Field 'RESOURCE_ARN', Parameter the request's ARN, Reason "The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes.", and message is composed from those three as "Error reason: <Reason>, field: <Field>, parameter: <Parameter>". Emulator: 400 but __type 'ValidationException' -- an error shape WAFv2 does not model at all -- with Field, Parameter and Reason entirely absent, and the message reporting a different member (recorded example: "1 validation error detected: Value 'stale-token' at 'lockToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$"). Why: an SDK client deserializes errors by __type, so it cannot map ValidationException to any modeled WAFv2 error and surfaces it as an unknown/unmodeled service error rather than WafV2Exception::WafInvalidParameterException; code that reads Field/Parameter/Reason to decide which input to correct gets nulls. Hypothesis for the member mismatch: the emulator runs generic member-constraint validation before any semantic/ARN validation, so the LockToken pattern fires first and AWS's RESOURCE_ARN finding is never reached. Fix: the wafv2 validation layer and its error serializer -- raise WAFInvalidParameterException populated with Field/Parameter/Reason, and order semantic resource-reference validation as AWS does. Traces: wafv2-managed-rule-set-vendor-operations-on-fresh-account#task#3.

Cause 2 of 2: CreateAPIKey with an invalid token domain. AWS: 400 WAFInvalidParameterException (Smithy: CreateAPIKey.TokenDomains -> APIKeyTokenDomains, required, length min 1; errors include WAFInvalidParameterException). The recorded message reports the failure against the whole list member path and enumerates every constraint of the member target at once -- recorded example: "1 validation error detected: Value '[not a valid domain]' at 'tokenDomains' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 253, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[\\w\\.\\-/]+$]". The rule is request-derived and therefore a requirement: the path is the list member ('tokenDomains'), the value is rendered as the list, and all constraints of the member target are listed in one bracketed set. Emulator: 400 with the same status but a different-shaped message built per offending element and per single constraint -- recorded example: "1 validation error detected: Value 'not a valid domain' at 'tokenDomains.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w\\.\\-/]+$". Why: clients and tests that parse the validation message to find the offending field see an index-qualified path ('tokenDomains.1.member') that AWS never emits, and only one of the member's constraints, so the reported reason differs from AWS for the same rejected input. Fix: the shared constraint-validation message formatter used by the wafv2 handlers (aggregate list-member constraints onto the list member's path). Traces: wafv2-api-key-lifecycle-for-captcha-integration#task#4.

Both causes are in the same validation/error-serialization seam, which is why they are bundled; closing only cause 1 leaves CreateAPIKey's message shape still divergent.

## Evidence
Replay of simparity-wire-aws-v1, 2 divergence clusters:

1. PutManagedRuleSetVersions, expected 400 / actual 400 but wrong error shape (wafv2-managed-rule-set-vendor-operations-on-fresh-account#task#3):
   $.__type: expected 'WAFInvalidParameterException', got 'ValidationException'
   $.Field: expected 'RESOURCE_ARN', got None (absent)
   $.Parameter: expected 'arn:aws:wafv2:us-east-1:000000000000:regional/rulegroup/does-not-exist/00000000-0000-0000-0000-000000000000', got None
   $.Reason: expected "The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes.", got None
   $.message: expected "Error reason: <Reason>, field: RESOURCE_ARN, parameter: <arn>", got "1 validation error detected: Value 'stale-token' at 'lockToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$"

2. CreateAPIKey, 400 -> 400, $.message only (wafv2-api-key-lifecycle-for-captcha-integration#task#4):
   expected "1 validation error detected: Value '[not a valid domain]' at 'tokenDomains' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 253, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^[\\w\\.\\-/]+$]"
   got "1 validation error detected: Value 'not a valid domain' at 'tokenDomains.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w\\.\\-/]+$"

Model grounding: Smithy WAFInvalidParameterException {message, Field: ParameterExceptionField, Parameter (length min 1), Reason: ErrorReason}, errorKind client, httpError 400; it is bound to both PutManagedRuleSetVersions and CreateAPIKey. 'ValidationException' is not a shape in the wafv2 model.

## Steps to Reproduce
1. Replay fixture wafv2-managed-rule-set-vendor-operations-on-fresh-account step task#3: PutManagedRuleSetVersions naming a rule group ARN that does not exist, with LockToken 'stale-token'. Observe __type 'ValidationException' and absent Field/Parameter/Reason instead of WAFInvalidParameterException with Field 'RESOURCE_ARN'.
2. Replay fixture wafv2-api-key-lifecycle-for-captcha-integration step task#4: CreateAPIKey with a TokenDomains entry containing spaces. Observe the message path 'tokenDomains.1.member' with a single constraint instead of 'tokenDomains' with the aggregated constraint list.
