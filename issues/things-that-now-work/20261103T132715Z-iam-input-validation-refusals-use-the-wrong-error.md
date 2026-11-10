Issue-Source: trace-sweep-divergence
Issue-Trace: iam-agentcore-traj-team-role-deny-is-read-only
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-agentcore-traj-team-role-deny-is-read-only iam-bap-user-reference-mistakes iam-ecsm-instance-profile-arn-vs-name-role-policy-chain iam-sshp-traj-instance-profile-chain-name-vs-arn iam-execute-api-role-lookup-mistakes iam-execute-api-simulation-request-mistakes iam-gpn-traj-attach-glue-service-role-to-the-worker
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRolePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetUser|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetInstanceProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|ListRolePolicies|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetRolePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|SimulatePrincipalPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|ListAttachedRolePolicies|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: f9571cbe65aa0d276fa5ca35992cbed3efbd51b6

# IAM input-validation refusals use the wrong error code and generic constraint prose

> _Note: this issue was identified against the code base at commit `f9571cbe65aa0d276fa5ca35992cbed3efbd51b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRolePolicy

## Description
When an IAM request violates a modeled name constraint or omits a required member, the emulator answers with a generic Smithy constraint-violation sentence (and sometimes the wrong error code) instead of AWS's wording for that member. The HTTP status (400) is right in every case; the error code and/or message is wrong. 7 causes.

Cause 1 of 7: a roleName that is not a valid IAM friendly name is refused with the generic constraint template instead of AWS's roleName sentence.
AWS: Smithy GetRolePolicy.RoleName targets roleNameType with @length min 1 max 64 and @pattern ^[\w+=,.@-]+$. The recording shows 400 with Error.Code ValidationError and the fixed message "The specified value for roleName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-" — the only variable part is the member name (roleName), and the offending value is NOT echoed.
Emulator: 400 with the generic Smithy text "2 validation errors detected: Value at 'roleName' failed to satisfy constraint: Member must have length less than or equal to 64; Value at 'roleName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$".
Why: clients and operators match on this sentence to distinguish a malformed name from a missing role; AWS's single-sentence form also tells the user which characters are allowed. The emulator's message additionally enumerates two violations where AWS reports one condition.
Fix: the IAM name validation layer (the error message template for the friendly-name pattern).
Traces: iam-agentcore-traj-team-role-deny-is-read-only#task#4.

Cause 2 of 7: the same template is used for userName.
AWS: Smithy GetUser.UserName targets existingUserNameType (@pattern ^[\w+=,.@-]+$). Recording: 400, message "The specified value for userName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-" — fixed text, variable part is the member name userName; the supplied value is not echoed.
Emulator: "1 validation error detected: Value 'arn:aws:iam::000000000000:user/.../...' at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$" — wrong wording and it echoes the caller's value back.
Why: as Cause 1, plus the emulator reflects caller input into the error body where AWS does not.
Fix: same component.
Traces: iam-bap-user-reference-mistakes#task#2.

Cause 3 of 7: for instanceProfileName AWS does use the "validation error detected" template, but without the offending value and with the unanchored pattern.
AWS: Smithy GetInstanceProfile.InstanceProfileName targets instanceProfileNameType with @pattern ^[\w+=,.@-]+$. Recording: 400, message "1 validation error detected: Value at 'instanceProfileName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w+=,.@-]+" — fixed text, variable parts are the member name and the pattern as AWS renders it (no ^...$ anchors), and no value is echoed.
Emulator: "1 validation error detected: Value 'arn:aws:iam::000000000000:instance-profile/...' at 'instanceProfileName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$" — echoes the value and renders the anchored pattern.
Why: the message differs from AWS's wording in two ways a string match notices; it also leaks the request value.
Fix: the shared constraint-violation message renderer (drop the value, render the pattern unanchored).
Traces: iam-ecsm-instance-profile-arn-vs-name-role-policy-chain#task#5.

Cause 4 of 7: the roleName sentence is also wrong on ListRolePolicies.
AWS: Smithy ListRolePolicies.RoleName targets roleNameType (length 1-64, pattern ^[\w+=,.@-]+$); recording: 400 with "The specified value for roleName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-".
Emulator: the "2 validation errors detected: ..." generic form.
Why: as Cause 1.
Fix: same component.
Traces: iam-execute-api-role-lookup-mistakes#task#4.

Cause 5 of 7: the roleName sentence is also wrong on ListAttachedRolePolicies.
AWS: same constraint on ListAttachedRolePolicies.RoleName; recording: 400 with the same "The specified value for roleName is invalid..." sentence.
Emulator: the "2 validation errors detected: ..." generic form.
Why: as Cause 1.
Fix: same component.
Traces: iam-gpn-traj-attach-glue-service-role-to-the-worker#task#3.

Cause 6 of 7: a missing required member is classified MalformedInput instead of ValidationError, and the message echoes the null.
AWS: Smithy GetRolePolicy.PolicyName is @required. Recording: 400 with Error.Code ValidationError and message "1 validation error detected: Value at 'policyName' failed to satisfy constraint: Member must not be null" — fixed text, variable part is the member name policyName.
Emulator: Error.Code MalformedInput and message "1 validation error detected: Value null at 'policyName' failed to satisfy constraint: Member must not be null" (an extra "null" token after "Value").
Why: the error code is what SDK retry/classification logic branches on; MalformedInput is not the code AWS returns for an absent required member, so a client that special-cases it mishandles its own bug.
Fix: the request-deserialization/validation layer's error classification and missing-member message template.
Traces: iam-execute-api-role-lookup-mistakes#task#5.

Cause 7 of 7: the same MalformedInput-for-ValidationError misclassification on SimulatePrincipalPolicy.
AWS: recording shows 400 with Error.Code ValidationError for a request whose required input fails framework validation (Smithy SimulatePrincipalPolicy requires PolicySourceArn and ActionNames; its modeled errors are InvalidInputException/InvalidInput, NoSuchEntityException/NoSuchEntity and PolicyEvaluationException, so ValidationError is the awsQuery framework validation code, not a modeled shape).
Emulator: Error.Code MalformedInput.
Why: as Cause 6.
Fix: same component.
Traces: iam-execute-api-simulation-request-mistakes#task#3.

The open issues 20261029T184359Z-iam-error-codes-diverge-from-the-modeled-awsquery.md and 20261103T022922Z-iam-getrole-refuses-a-path-prefixed-rolename-with.md describe the same family of faults on other operations but carry no replay key for any of the 7 causes above.

## Evidence
Replay of trace set simparity-wire-aws-v1; all steps returned the expected status 400 with a diverging code/message.

Cause 1 — iam-agentcore-traj-team-role-deny-is-read-only#task#4, GetRolePolicy: $.ErrorResponse.Error.Message expected 'The specified value for roleName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-', got "2 validation errors detected: Value at 'roleName' failed to satisfy constraint: Member must have length less than or equal to 64; Value at 'roleName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$".

Cause 2 — iam-bap-user-reference-mistakes#task#2, GetUser: expected 'The specified value for userName is invalid. It must contain only alphanumeric characters and/or the following: +=,.@_-', got "1 validation error detected: Value 'arn:aws:iam::000000000000:user/sim2real-probe-bap-ids-178584d1/sim2real-probe-bap-staging178584d1' at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$".

Cause 3 — iam-ecsm-instance-profile-arn-vs-name-role-policy-chain#task#5, GetInstanceProfile (also iam-sshp-traj-instance-profile-chain-name-vs-arn#task#5): expected "1 validation error detected: Value at 'instanceProfileName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w+=,.@-]+", got "1 validation error detected: Value 'arn:aws:iam::000000000000:instance-profile/sim2real-probe-ecsm-prof-97b77475' at 'instanceProfileName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w+=,.@-]+$".

Cause 4 — iam-execute-api-role-lookup-mistakes#task#4, ListRolePolicies: same expected/actual pair as Cause 1.

Cause 5 — iam-gpn-traj-attach-glue-service-role-to-the-worker#task#3, ListAttachedRolePolicies (also iam-sshp-traj-instance-profile-chain-name-vs-arn#task#8): same expected/actual pair as Cause 1.

Cause 6 — iam-execute-api-role-lookup-mistakes#task#5, GetRolePolicy: $.ErrorResponse.Error.Code expected 'ValidationError', got 'MalformedInput'; Message expected "1 validation error detected: Value at 'policyName' failed to satisfy constraint: Member must not be null", got "1 validation error detected: Value null at 'policyName' failed to satisfy constraint: Member must not be null".

Cause 7 — iam-execute-api-simulation-request-mistakes#task#3, SimulatePrincipalPolicy: $.ErrorResponse.Error.Code expected 'ValidationError', got 'MalformedInput'.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay iam-agentcore-traj-team-role-deny-is-read-only step task#4 and compare the ErrorResponse body.
3. Replay the steps in trace_ids: iam-bap-user-reference-mistakes#task#2, iam-ecsm-instance-profile-arn-vs-name-role-policy-chain#task#5, iam-execute-api-role-lookup-mistakes#task#4 and #task#5, iam-execute-api-simulation-request-mistakes#task#3, iam-gpn-traj-attach-glue-service-role-to-the-worker#task#3.
4. In each response diff $.ErrorResponse.Error.Code and $.ErrorResponse.Error.Message against the recording.
