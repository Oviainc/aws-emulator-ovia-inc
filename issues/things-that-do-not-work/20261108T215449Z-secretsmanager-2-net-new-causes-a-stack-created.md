Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.Tags
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|$
Issue-Related: 20261029T224105Z-secretsmanager-secrets-manager-accepts-createsecret-putsecretvalue-and-updatesecret-with.md
Issue-Commit: 6211fb78e0841381478152dba6893fe342be7489

# secretsmanager: 2 net-new causes — a stack-created secret carries no aws:cloudformation tags, and deleting the stack leaves the secret describable

> _Note: this issue was identified against the code base at commit `6211fb78e0841381478152dba6893fe342be7489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
Bundle of 2 net-new root causes of service `secretsmanager`. Both clusters report stub_path='' and owned_by=[], so no open issue carries their replay keys. A fixer MUST close both before submitting.

Cause 1 of 2: a secret provisioned by a CloudFormation stack carries none of the reserved stack-ownership tags.
AWS: Smithy: DescribeSecret input SecretId (required), output includes Tags (TagListType, a list of Key/Value pairs); the model says nothing about reserved keys, so recorded behavior only for the provenance. AWS CloudFormation stamps every resource it provisions — here an AWS::SecretsManager::Secret — with three reserved tags: aws:cloudformation:stack-id = the owning stack's ARN, aws:cloudformation:stack-name = the stack's name, aws:cloudformation:logical-id = the resource's logical id in the template. DescribeSecret answers HTTP 200 and returns them in Tags. The three KEYS are the fixed requirement; their values are taken from the owning stack and the template (the recording's stack ARN/uuid, generated stack name and logical id 'AuroraSecret' are per-run values).
Emulator: DescribeSecret answers 200 but the Tags member is absent entirely (got None).
Why: those tags are the secret-to-stack reverse lookup. A client holding only a secret id cannot discover the stack that owns it, cannot distinguish a stack-managed secret from a hand-made one, and tag-scoped policy or cost attribution over secrets comes back with no stack attribution.
Fix: the AWS::SecretsManager::Secret provisioning path in the CloudFormation resource provider must merge the three reserved tags into the secret state that the DescribeSecret serializer emits as Tags.
Traces: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy#task#7.

Cause 2 of 2: after the owning stack is deleted, the secret is still described instead of being gone.
AWS: Smithy: DescribeSecret models ResourceNotFoundException (client, httpError 400). Recorded behavior: once the stack whose DeletionPolicy removes the secret has been deleted, DescribeSecret on that secret answers HTTP 400 with the JSON error envelope {'__type': 'ResourceNotFoundException', 'Message': "Secrets Manager can't find the specified secret."} — the whole message is fixed text with no variable parts. CloudFormation removes the secret immediately on stack delete (a force delete with no recovery window), so the secret is not merely scheduled for deletion.
Emulator: answers HTTP 200 and returns the secret for the full 180 s poll (37 poll attempts, error code None), with DeletedDate set and VersionIdsToStages still populated — i.e. it put the secret into a recovery-window 'scheduled deletion' state and keeps serving it instead of removing it.
Why: a client polling for the teardown to finish never sees it; stack-delete waiters hang, and a re-deploy of the same template collides with a secret name that the client has been told is still live. A caller cannot tell a deleted secret from a live one.
Fix: the CloudFormation delete path for AWS::SecretsManager::Secret must force-delete without a recovery window, and the DescribeSecret lookup must raise ResourceNotFoundException for a secret that has been removed.
Traces: cloudformation-refarch-aur-secret-stack-route-to-deletion-policy#teardown#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. Two divergence clusters, both verdict=mismatch, stub_path='', owned_by=[], fixture cloudformation-refarch-aur-secret-stack-route-to-deletion-policy:
1. divergence_key 'service=secretsmanager|DescribeSecret|status=200->200|keys=$.Tags', step task#7, affected_count 1. Detail: $.Tags missing — expected [{'Key': 'aws:cloudformation:stack-id', 'Value': the owning stack's ARN}, {'Key': 'aws:cloudformation:stack-name', 'Value': the stack's name}, {'Key': 'aws:cloudformation:logical-id', 'Value': the resource's logical id}], got None.
2. divergence_key 'service=secretsmanager|DescribeSecret|status=400->200|keys=$,$.Message,$.__type,status', step teardown#9, affected_count 1. Detail: status expected 400, got 200; $.__type missing (expected 'ResourceNotFoundException', got None); $.Message missing (expected "Secrets Manager can't find the specified secret.", got None); replay reports 'poll capped at 180s' and 'poll: error code None not in [ResourceNotFoundException] after 37 attempt(s)', the served body carrying ARN/Name/Description plus DeletedDate and VersionIdsToStages.
Secret ARNs with their random suffix, the generated probe names and the epoch timestamps in these records are per-run values; the requirements are the presence of the three reserved tag keys, and the 400 / ResourceNotFoundException / fixed message for a removed secret.

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Cause 1: replay fixture cloudformation-refarch-aur-secret-stack-route-to-deletion-policy step task#7 — deploy the stack declaring an AWS::SecretsManager::Secret (logical id AuroraSecret), then call DescribeSecret on the created secret. Expect Tags to contain aws:cloudformation:stack-id, aws:cloudformation:stack-name and aws:cloudformation:logical-id; observe no Tags member.
3. Cause 2: continue to step teardown#9 — delete the stack, then poll DescribeSecret on that secret id. Expect HTTP 400 with __type 'ResourceNotFoundException' and Message "Secrets Manager can't find the specified secret."; observe HTTP 200 returning the secret (DeletedDate set) for the whole 180 s poll window.
