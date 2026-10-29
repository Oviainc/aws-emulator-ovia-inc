Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cdc-stream-error-handling
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cdc-stream-error-handling dsql-cluster-subresource-guards
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|CreateStream|4xx->2xx|$.fieldList
Issue-Key: dsql|CreateStream|4xx->2xx|$.message
Issue-Key: dsql|CreateStream|4xx->2xx|$.reason
Issue-Key: dsql|CreateStream|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: dsql|CreateStream|4xx->2xx|status
Issue-Key: dsql|TagResource|4xx->2xx|$
Issue-Key: dsql|TagResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: dsql|TagResource|4xx->2xx|status
Issue-Key: dsql|UpdateCluster|4xx->2xx|$.message
Issue-Key: dsql|UpdateCluster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: dsql|UpdateCluster|4xx->2xx|status
Issue-Key: dsql|UpdateCluster|2xx->4xx|$.arn
Issue-Key: dsql|UpdateCluster|2xx->4xx|$.creationTime
Issue-Key: dsql|UpdateCluster|2xx->4xx|$.identifier
Issue-Key: dsql|UpdateCluster|2xx->4xx|$.status
Issue-Key: dsql|UpdateCluster|2xx->4xx|status
Issue-Key: dsql|GetCluster|2xx->2xx|$.encryptionDetails.encryptionType
Issue-Commit: 16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22

# dsql mutations accept input AWS refuses, and the accepted input corrupts cluster state

> _Note: this issue was identified against the code base at commit `16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStream

## Description
Three mutations answer 200 for requests Aurora DSQL refuses with 400, because the emulator never checks the referenced IAM role, the tag character set, or the referenced KMS key. Two further divergences in the same fixture are the state damage that the accepted KMS key then causes.

Cause 1 of 5: CreateStream whose targetDefinition names an IAM role that does not exist (or whose trust policy does not allow Aurora DSQL).
AWS: 400 ValidationException with reason 'fieldValidationFailed' and a fieldList entry whose name is the wire member 'role-arn' and whose message repeats the top-level message: "Aurora DSQL can't access the specified IAM role <roleArn> because the role doesn't exist or doesn't allow Aurora DSQL access. Create the role or modify its trust policy to grant Aurora DSQL access." The sentences are fixed text; the only variable part is the role ARN echoed from targetDefinition. Smithy: CreateStream.targetDefinition is required; ValidationException requires message and reason (ValidationExceptionReason includes fieldValidationFailed), fieldList entries require name and message.
Emulator: 200 — the stream is created, no ValidationException body, no x-amzn-errortype header.
Why: a caller that relies on this refusal to catch a missing or mis-trusted delivery role gets a stream the real service would never have created, and the failure surfaces much later (or never, since the emulator serves no CDC delivery).
Fix: CreateStream's validation, which must resolve targetDefinition's role ARN and emit the modeled fieldValidationFailed ValidationException before minting the stream.
Traces: dsql-cdc-stream-error-handling#task#4.

Cause 2 of 5: TagResource with a tag key or value containing characters outside DSQL's tag character set.
AWS: 400 with the fixed message "Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @" (the whole sentence is fixed text; nothing from the request is interpolated) and an x-amzn-errortype header. Smithy: TagKey and TagValue both carry pattern ^[a-zA-Z0-9_.:/=+\\-@ ]*$.
Emulator: 200 with an empty body — the tag is stored.
Why: a client validating its tagging logic against the emulator ships keys/values AWS rejects, and the emulator's state now holds tags that cannot exist in the real service.
Fix: TagResource's input validation (the TagMap character-set check), shared with the tag maps on the create paths.
Traces: dsql-cluster-subresource-guards#task#6.

Cause 3 of 5: UpdateCluster with a kmsEncryptionKey naming a KMS alias that does not exist in the account/Region.
AWS: 400 with the message "AWS KMS key <keyIdentifier> not found. Verify the key ID or ARN and ensure the key exists in your AWS account and Region." — the two sentences are fixed text, and the variable part is the key identifier exactly as supplied in the request (an alias/<name>, key id or ARN). Smithy: UpdateCluster.kmsEncryptionKey targets KmsEncryptionKey (@length 1..2048, pattern ^[a-zA-Z0-9:/_-]+$); the existence check is the service's, not the pattern's.
Emulator: 200 — the update is applied and the bogus key is recorded as the cluster's encryption key.
Why: a caller that expects a bad key to be rejected at request time instead sees an encryption configuration silently take effect, and every later read of the cluster reports encryption against a key that does not exist.
Fix: UpdateCluster's validation, which must resolve kmsEncryptionKey against KMS before mutating encryptionDetails.
Traces: dsql-cluster-subresource-guards#task#8.

Cause 4 of 5: the next UpdateCluster in the same sequence is refused with 409 where AWS returns 200.
AWS: 200 with the UpdateClusterOutput envelope — identifier, arn, status and creationTime are all required members.
Emulator: 409 ConflictException with no body members at all ($.arn, $.identifier, $.creationTime, $.status all missing).
Why: the follow-up configuration change is unreachable, so a caller's update sequence dead-ends on a conflict AWS never reports.
Hypothesis: Cause 3's wrongly accepted key change left the cluster in a state the mutation gate treats as in-flight, so the subsequent UpdateCluster conflicts; once the bad key is refused at request time this step should pass unchanged.
Fix: UpdateCluster's handler/state transition — the same component as Cause 3; re-check after fixing it.
Traces: dsql-cluster-subresource-guards#task#9.

Cause 5 of 5: GetCluster reports encryptionDetails.encryptionType as CUSTOMER_MANAGED_KMS_KEY where AWS reports AWS_OWNED_KMS_KEY.
AWS: encryptionDetails.encryptionType reflects only encryption configuration the service actually accepted; a cluster whose customer-key change was refused stays on the AWS-owned key. Smithy: EncryptionDetails requires encryptionType and encryptionStatus.
Emulator: the cluster reports a customer-managed key, because the key change from Cause 3 was recorded.
Why: a client reading encryptionDetails to decide whether a cluster is customer-key protected is told it is, against a key that does not exist.
Hypothesis: downstream of Cause 3; verify after that validation lands.
Fix: UpdateCluster's state mutation of encryptionDetails (do not record a key the validation should have rejected).
Traces: dsql-cluster-subresource-guards#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- dsql-cdc-stream-error-handling#task#4 CreateStream: expected status 400, got 200; expected $.reason 'fieldValidationFailed', $.message "Aurora DSQL can't access the specified IAM role arn:aws:iam::000000000000:role/<role> because the role doesn't exist or doesn't allow Aurora DSQL access. Create the role or modify its trust policy to grant Aurora DSQL access." and $.fieldList [{name: 'role-arn', message: <same text>}] — all three missing, no x-amzn-errortype.
- dsql-cluster-subresource-guards#task#6 TagResource: expected status 400 with body {'message': 'Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @'}; got 200 with {} and no x-amzn-errortype.
- dsql-cluster-subresource-guards#task#8 UpdateCluster: expected status 400 with $.message 'AWS KMS key alias/<name> not found. Verify the key ID or ARN and ensure the key exists in your AWS account and Region.'; got 200, message missing.
- dsql-cluster-subresource-guards#task#9 UpdateCluster: expected status 200 with arn/identifier/creationTime/status; got 409 with none of them.
- dsql-cluster-subresource-guards#task#10 GetCluster (200 both sides): $.encryptionDetails.encryptionType expected 'AWS_OWNED_KMS_KEY', got 'CUSTOMER_MANAGED_KMS_KEY'.

## Steps to Reproduce
1. Replay dsql-cdc-stream-error-handling step task#4: CreateStream with targetDefinition naming a role ARN that does not exist -> emulator returns 200 and creates the stream instead of 400 ValidationException(reason=fieldValidationFailed, fieldList[name=role-arn]).
2. Replay dsql-cluster-subresource-guards step task#6: TagResource with a tag key/value containing a character outside ^[a-zA-Z0-9_.:/=+\-@ ]*$ -> 200 instead of 400.
3. Replay dsql-cluster-subresource-guards step task#8: UpdateCluster with kmsEncryptionKey = alias/<nonexistent alias> -> 200 instead of 400.
4. Continue the same fixture: step task#9 UpdateCluster -> 409 instead of 200, and step task#10 GetCluster -> encryptionDetails.encryptionType CUSTOMER_MANAGED_KMS_KEY instead of AWS_OWNED_KMS_KEY.
