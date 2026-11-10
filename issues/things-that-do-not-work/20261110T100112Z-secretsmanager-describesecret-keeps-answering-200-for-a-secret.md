Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|$.Message
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|$.__type
Issue-Related: 20261108T215449Z-secretsmanager-2-net-new-causes-a-stack-created.md
Issue-Related: 20261109T205028Z-secretsmanager-an-rds-managed-master-user-secret-outlives.md
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# secretsmanager DescribeSecret keeps answering 200 for a secret that was deleted, instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One root cause: a deleted secret stays readable through DescribeSecret.
AWS: once a secret has been deleted, DescribeSecret for it is refused with HTTP 400 and the error ResourceNotFoundException whose message is the fixed text "Secrets Manager can't find the specified secret." (no variable part). Smithy: DescribeSecret errors include ResourceNotFoundException (errorKind client, httpError 400); DeletedDate is documented as the date the deletion of the secret was requested.
Emulator: answers HTTP 200 with the full DescribeSecret output for the removed secret — ARN, Name, Description, RotationEnabled, VersionIdsToStages, CreatedDate and a populated DeletedDate — and keeps doing so for the whole polling window (37 attempts over the 180s cap), so the refusal never arrives.
Why: a client cannot tell a live secret from a removed one. Teardown verification, existence probes and 'is this secret gone yet' polling all succeed where AWS refuses, and a caller that treats a 200 as 'the secret exists' will keep using an identifier Secrets Manager no longer honours.
Fix: the SecretsManager DescribeSecret handler's lifecycle check against the stored secret's deleted state (and the state the CloudFormation AWS::SecretsManager::Secret delete path leaves behind).
Traces: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy#teardown#9.

Related open issue, which does NOT carry this replay key: 20261109T223435Z-cloudformation-secretsmanager-4-net-new-causes-describesecret-still.md describes the same emulator behavior as its behavioral Cause 1 (service cloudformation, grounded in behavioral check ids). This wire step is filed here so the fix is gated on a fixture a fixer can re-replay.

## Evidence
Step cloudformation-refarch-aur-secret-stack-route-to-deletion-policy#teardown#9: status: expected 400, got 200. $.__type: missing (expected 'ResourceNotFoundException', got None). $.Message: missing (expected "Secrets Manager can't find the specified secret.", got None). $: poll capped at 180s / 'error code None not in [ResourceNotFoundException] after 37 attempt(s)' — the served body was {'ARN': 'arn:aws:secretsmanager:...:secret:<name>-<suffix>', 'Name': '<name>', 'Description': '...', 'RotationEnabled': False, 'LastChangedDate': ..., 'DeletedDate': ..., 'VersionIdsToStages': {...}, 'CreatedDate': ...}.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture cloudformation-refarch-aur-secret-stack-route-to-deletion-policy, step teardown#9: deploy the stack that creates the Aurora master-credentials secret, delete the stack (deletion policy removes the secret), then poll DescribeSecret with the secret's ARN.
3. Observe HTTP 200 with a DeletedDate-bearing description on every attempt, instead of HTTP 400 ResourceNotFoundException with the message "Secrets Manager can't find the specified secret."
