Issue-Source: trace-sweep-divergence
Issue-Trace: iam-account-alias-duplicate-create-error
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-account-alias-duplicate-create-error iam-account-alias-duplicate-error iam-account-alias-lifecycle-and-guard-clauses iam-cred-account-alias-format-validation iam-account-properties-get-put-lifecycle-and-invalid-property-error iam-account-properties-role-manager-toggle iam-account-properties-get-baseline
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|CreateAccountAlias|4xx->2xx|$.ErrorResponse
Issue-Key: iam|CreateAccountAlias|4xx->2xx|status
Issue-Key: iam|PutAccountProperties|4xx->2xx|$.ErrorResponse
Issue-Key: iam|PutAccountProperties|4xx->2xx|status
Issue-Key: iam|GetAccountProperties|2xx->2xx|$.GetAccountPropertiesResponse.GetAccountPropertiesResult.Properties.entry
Issue-Key: iam|GetAccountProperties|2xx->2xx|$.GetAccountPropertiesResponse.GetAccountPropertiesResult.Properties
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Account alias and account property writes skip validation and duplicate detection

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccountAlias

## Description
Cause 1 of 4 - CreateAccountAlias does not detect an existing alias. AWS: an account has at most one alias; creating one when the same alias already exists is refused HTTP 409 with Code EntityAlreadyExists and Message 'The account alias <accountAlias> already exists.' - fixed text with the request's AccountAlias substituted (Smithy: CreateAccountAlias errors include EntityAlreadyExistsException, httpError 409, awsQueryError code EntityAlreadyExists). Emulator: HTTP 200 with an empty CreateAccountAliasResponse. Why: an idempotency-sensitive client that treats EntityAlreadyExists as 'already provisioned' instead reads success and never learns the alias was not re-created. Fix: the CreateAccountAlias handler must compare against the stored alias and raise EntityAlreadyExists.
Traces: iam-account-alias-duplicate-create-error#task#2.

Cause 2 of 4 - CreateAccountAlias does not enforce the alias format. AWS: the alias must be 3-63 characters of lowercase letters, digits and hyphens, not beginning or ending with a hyphen (Smithy: accountAliasType length 3..63, pattern ^[a-z0-9]([a-z0-9]|-(?!-)){1,61}[a-z0-9]$). A violating alias is refused HTTP 400 with Code ValidationError and the fixed message 'The specified value for accountAlias is invalid. It must be a minimum length of 3 characters and maximum length of 63 characters, contain only digits, lowercase letters, and hyphens (-), but cannot begin or end with a hyphen.' - no request value is interpolated. Emulator: a violating alias is accepted with HTTP 200, and where it does reject (too-short), it emits the generic constraint text instead of the AWS prose. Why: a client gets an alias AWS would refuse, then fails only when the same call is made against real IAM. Fix: alias validation in the CreateAccountAlias handler, raising ValidationError with the message above.
Traces: iam-cred-account-alias-format-validation#task#1.

Cause 3 of 4 - PutAccountProperties accepts an unknown namespace. AWS: property keys are namespaced and an unrecognised namespace is refused HTTP 400 with Code InvalidInput and Message 'Unsupported namespace: <namespace>' - fixed text with the leading namespace segment of the submitted key substituted. Emulator: HTTP 200, and the bogus property is persisted, so a later GetAccountProperties returns an extra entry (the key the request invented) that AWS never stores. Why: the account-properties map diverges permanently after one bad write, and a client never learns its key was rejected. Fix: namespace validation in the PutAccountProperties handler, before the property is written to account state.
Traces: iam-account-properties-get-put-lifecycle-and-invalid-property-error#task#3 (200 instead of 400) and #teardown#5 (GetAccountProperties returns the extra entry - cascade of the accepted write).

Cause 4 of 4 - GetAccountProperties has no default property set. AWS: a fresh account returns GetAccountPropertiesResult/Properties with the account's default entries, which the recordings show as entry{key 'RoleManager/Enabled', value 'false'}. Emulator: Properties is serialised empty (''). Why: a client reading a default-valued property before setting it gets nothing instead of the documented default, so feature-gate checks misfire. Fix: seed the account-properties state with its defaults.
Traces: iam-account-properties-get-baseline#task#1.

## Evidence
Cause 1: expected (recorded AWS) HTTP 409, ErrorResponse/Error{Type Sender, Code EntityAlreadyExists, Message 'The account alias <accountAlias> already exists.'}; actual HTTP 200 with no ErrorResponse at all.
Cause 2: expected HTTP 400 Code ValidationError Message 'The specified value for accountAlias is invalid. It must be a minimum length of 3 characters and maximum length of 63 characters, contain only digits, lowercase letters, and hyphens (-), but cannot begin or end with a hyphen.'; actual HTTP 200 for the invalid alias, and for a too-short alias the message is "1 validation error detected: Value at 'accountAlias' failed to satisfy constraint: Member must have length greater than or equal to 3".
Cause 3: expected HTTP 400 Code InvalidInput Message 'Unsupported namespace: <namespace>'; actual HTTP 200. Follow-on GetAccountProperties expected Properties/entry{key 'RoleManager/Enabled', value 'false'} only; actual two entries including {key 'NotARealProperty/Enabled', value 'true'}.
Cause 4: expected GetAccountPropertiesResult/Properties/entry{key 'RoleManager/Enabled', value 'false'}; actual Properties = '' (type_mismatch).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-account-alias-duplicate-create-error: task#1 CreateAccountAlias, task#2 the same alias again -> 200 instead of 409 EntityAlreadyExists.
3. Replay iam-cred-account-alias-format-validation#task#1 (alias violating the format) -> 200 instead of 400 ValidationError.
4. Replay iam-account-properties-get-put-lifecycle-and-invalid-property-error#task#3 (PutAccountProperties with key 'NotARealProperty/Enabled') -> 200 instead of 400 InvalidInput, then #teardown#5 shows the property persisted.
5. Replay iam-account-properties-get-baseline#task#1 on a fresh account -> empty Properties instead of the RoleManager/Enabled default.
