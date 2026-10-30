Issue-Source: trace-sweep-divergence
Issue-Trace: iam-list-groups-op-contract
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-list-groups-op-contract iam-list-instance-profiles-contract iam-list-roles-path-prefix-scoping-and-malformed-error iam-cred-account-alias-format-validation iam-sts-preferences-token-version-toggle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListGroups|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListInstanceProfiles|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListInstanceProfiles|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListRoles|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|CreateAccountAlias|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|CreateAccountAlias|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|SetSecurityTokenServicePreferences|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|SetSecurityTokenServicePreferences|4xx->4xx|$.ErrorResponse.RequestId
Issue-Related: 20261029T184359Z-iam-error-responses-omit-the-responsemetadata-requestid-element.md
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM ValidationError messages emit constraint text instead of AWS prose

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGroups

## Description
Three input-validation paths return the right status (400) and Code (ValidationError) but the generated Smithy constraint text where AWS emits its own prose, or the right prose with the wrong ordering.

Cause 1 of 3 - the pathPrefix rejection on the list operations. AWS: a pathPrefix that does not begin with '/' is refused 400 ValidationError with the fixed message 'The specified value for pathPrefix is invalid. It must begin with the / character and contain only alphanumeric characters and/or / characters.' - no request value is interpolated. Emulator: it emits the awsQuery constraint-violation form instead, echoing the submitted value and the regex. The same single defect shows on ListGroups, ListInstanceProfiles and ListRoles - one shared pathPrefix validator. Why: a client matching or displaying the documented message gets an unmatched string, and the submitted value is echoed back where AWS does not echo it. Fix: the shared pathPrefix validator must raise the AWS prose message.
Traces: iam-list-groups-op-contract#task#3; iam-list-instance-profiles-contract#task#3; iam-list-roles-path-prefix-scoping-and-malformed-error#task#3.

Cause 2 of 3 - the accountAlias length rejection. AWS: an alias outside 3..63 characters is refused 400 ValidationError with the single fixed message 'The specified value for accountAlias is invalid. It must be a minimum length of 3 characters and maximum length of 63 characters, contain only digits, lowercase letters, and hyphens (-), but cannot begin or end with a hyphen.' - one message for the whole rule, length and charset alike (Smithy: accountAliasType length 3..63, pattern ^[a-z0-9]([a-z0-9]|-(?!-)){1,61}[a-z0-9]$). Emulator: it emits only the violated length constraint, "1 validation error detected: Value at 'accountAlias' failed to satisfy constraint: Member must have length greater than or equal to 3". Fix: the CreateAccountAlias input validator must emit the single AWS message for any accountAlias violation.
Traces: iam-cred-account-alias-format-validation#task#2.

Cause 3 of 3 - the globalEndpointTokenVersion enum rejection lists the values in the wrong order. AWS: 400 ValidationError with "1 validation error detected: Value at 'globalEndpointTokenVersion' failed to satisfy constraint: Member must satisfy enum value set: [v2Token, v1Token]" - the set is rendered in the model's declaration order, v2Token first. Emulator: it renders '[v1Token, v2Token]'. Why: the enum set in the message is the documented, matchable part of the text; a client or test matching it fails. Fix: the enum-constraint message renderer must preserve the model's member order rather than sorting.
Traces: iam-sts-preferences-token-version-toggle#task#2.

## Evidence
Cause 1: expected 400 ValidationError 'The specified value for pathPrefix is invalid. It must begin with the / character and contain only alphanumeric characters and/or / characters.'; actual 400 ValidationError "1 validation error detected: Value 'badpath' at 'pathPrefix' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\u002F[\\u0021-\\u007F]*$" - reported identically on ListGroups, ListInstanceProfiles and ListRoles.
Cause 2: expected 'The specified value for accountAlias is invalid. It must be a minimum length of 3 characters and maximum length of 63 characters, contain only digits, lowercase letters, and hyphens (-), but cannot begin or end with a hyphen.'; actual "1 validation error detected: Value at 'accountAlias' failed to satisfy constraint: Member must have length greater than or equal to 3".
Cause 3: expected "... Member must satisfy enum value set: [v2Token, v1Token]"; actual "... Member must satisfy enum value set: [v1Token, v2Token]".

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-list-groups-op-contract#task#3 (ListGroups PathPrefix='badpath') and diff the Message against the recording; repeat for iam-list-instance-profiles-contract#task#3 and iam-list-roles-path-prefix-scoping-and-malformed-error#task#3.
3. Replay iam-cred-account-alias-format-validation#task#2 (2-character alias).
4. Replay iam-sts-preferences-token-version-toggle#task#2 (SetSecurityTokenServicePreferences with an invalid token version).
