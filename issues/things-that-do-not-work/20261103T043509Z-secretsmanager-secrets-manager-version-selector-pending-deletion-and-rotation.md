Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-ecsm-pending-deletion-describe-and-restore-route
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-ecsm-pending-deletion-describe-and-restore-route secretsmanager-ecsm-get-secret-value-version-selector-refusals secretsmanager-never-rotated-secret-omits-every-rotation-field secretsmanager-refarch-aur-master-secret-lookup-and-rotation-probe
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|DeleteSecret|2xx->4xx|$.ARN
Issue-Key: secretsmanager|DeleteSecret|2xx->4xx|$.DeletionDate
Issue-Key: secretsmanager|DeleteSecret|2xx->4xx|$.Name
Issue-Key: secretsmanager|DeleteSecret|2xx->4xx|status
Issue-Key: secretsmanager|CreateSecret|4xx->4xx|$.Message
Issue-Key: secretsmanager|GetSecretValue|4xx->4xx|$.Message
Issue-Key: secretsmanager|GetSecretValue|4xx->4xx|$.__type
Issue-Key: secretsmanager|DescribeSecret|2xx->2xx|$.RotationEnabled
Issue-Key: secretsmanager|ListSecrets|4xx->4xx|$.message
Issue-Commit: 98adcf7e213c0698067aec3ca5f9090d9d4f5eb2

# Secrets Manager version-selector, pending-deletion and rotation-metadata responses diverge (6 causes)

> _Note: this issue was identified against the code base at commit `98adcf7e213c0698067aec3ca5f9090d9d4f5eb2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSecret

## Description
Six net-new root causes in the Secrets Manager emulator, found by replaying the wire fixture corpus (simparity-wire-aws-v1). They share one fault family: the response a secret's own lifecycle/version state should produce is built from the emulator's internal view rather than from AWS's — a refusal is given the wrong identity or wrong wording, a lifecycle call that AWS accepts is refused, and a field AWS omits is emitted. Cause count: 6.

=== Cause 1 of 6: DeleteSecret on a secret that is already scheduled for deletion is refused instead of accepted ===
AWS: recorded behavior only (the model states no such precondition). Smithy: DeleteSecret input is SecretId (required) + RecoveryWindowInDays + ForceDeleteWithoutRecovery; DeleteSecretResponse carries ARN, Name and DeletionDate; its modeled errors are InvalidParameterException, InvalidRequestException, ResourceNotFoundException, InternalServiceError. The recording shows that re-deleting a secret that is ALREADY in the scheduled-for-deletion state answers HTTP 200 with the full body — ARN (the secret's region/account-qualified `secret:<name>-<6 chars>` ARN), Name (the secret's name as created) and DeletionDate (the scheduled permanent-deletion instant). The delete is idempotent over the pending-deletion state; no error is raised.
Emulator: the same call answers HTTP 400 and carries none of the response members (ARN, Name and DeletionDate are all absent), i.e. a secret in the pending-deletion state is treated as un-deletable.
Why: teardown and reconcile code deletes unconditionally and then reads DeletionDate/ARN off the response. Against the emulator the second delete raises a client error, so cleanup paths abort and a client cannot re-stamp (or confirm) the recovery window of a secret it has already scheduled.
Fix: the DeleteSecret handler's state precondition — accept the call when the secret is already scheduled for deletion and serialize the full DeleteSecretResponse — in the secret-lifecycle state transition.
Traces: secretsmanager-ecsm-pending-deletion-describe-and-restore-route#task#8.

=== Cause 2 of 6: the CreateSecret pending-deletion refusal message interpolates the secret name into AWS's fixed text ===
AWS: Smithy CreateSecret -> InvalidRequestException (client, HTTP 400), documented for the case 'The secret is scheduled for deletion.' For a create whose Name collides with a secret currently scheduled for deletion, the recorded message is the FIXED text, with no variable part at all: "You can't create this secret because a secret with this name is already scheduled for deletion." ('this name' is literal — the name is deliberately not echoed).
Emulator: same status and error class, but the message substitutes the requested name into that sentence: "You can't create this secret because a secret with the name <name> is already scheduled for deletion." (replay: name sim2real-probe-ecsm-del-97b77475 — that value is one run's input filling the emulator's template, not part of the requirement).
Why: clients and SDK-level error matchers key on this message text to distinguish 'name is tombstoned, call RestoreSecret' from a plain ResourceExistsException collision; a reworded sentence fails an exact match and makes the recovery branch unreachable.
Fix: the CreateSecret handler's name-collision validation — emit AWS's fixed sentence verbatim, with no name interpolation.
Traces: secretsmanager-ecsm-pending-deletion-describe-and-restore-route#task#7.

=== Cause 3 of 6: GetSecretValue's unknown-VersionId refusal is worded as a 'secret version' miss rather than a 'secret value for VersionId' miss ===
AWS: Smithy GetSecretValue takes VersionId (length 32..64) and lists ResourceNotFoundException ('Secrets Manager can't find the resource that you asked for', client, HTTP 400). For a VersionId that is not a version of the named secret, the recorded message is the fixed text "Secrets Manager can't find the specified secret value for VersionId: " followed by one variable part — the VersionId exactly as the request supplied it.
Emulator: same status and class, different fixed text: "Secrets Manager can't find the specified secret version: <VersionId>". (The uuid in the replay is the request's own value, not a requirement.)
Why: the message is the only thing distinguishing 'the secret exists, this version does not' from 'the secret does not exist', since both are ResourceNotFoundException/400; version-pinned readers log and branch on that text.
Fix: the GetSecretValue handler's version-selector resolution and its ResourceNotFoundException message construction.
Traces: secretsmanager-ecsm-get-secret-value-version-selector-refusals#task#4.

=== Cause 4 of 6: GetSecretValue with a VersionId and a VersionStage that do not belong together is refused with the wrong error class ===
AWS: Smithy GetSecretValue accepts VersionId and VersionStage together and models InvalidRequestException ('A parameter value is not valid for the current state of the resource', client, HTTP 400) alongside ResourceNotFoundException. When BOTH selectors name existing things but the stage is not attached to that version, the refusal is InvalidRequestException with the fixed, variable-free message "You provided a VersionStage that is not associated to the provided VersionId."
Emulator: answers ResourceNotFoundException with the version-miss message "Secrets Manager can't find the specified secret version: <VersionId>" — the wrong __type AND the wrong message, so the inconsistent-selector case is reported as a missing version.
Why: a caller that pins both selectors as a safety check treats ResourceNotFoundException as 'version rotated away, re-resolve' and InvalidRequestException as 'my selectors disagree, fix the request'. Collapsing the second into the first sends clients into a retry/re-resolve loop over a request that can never succeed.
Fix: the GetSecretValue handler's version-selector resolution — when VersionId and VersionStage are both present and resolve to different versions, raise InvalidRequestException with AWS's sentence instead of falling through to the not-found path.
Traces: secretsmanager-ecsm-get-secret-value-version-selector-refusals#task#6.

=== Cause 5 of 6: DescribeSecret emits RotationEnabled=false for a secret that has never been configured for rotation ===
AWS: Smithy DescribeSecretResponse.RotationEnabled is a boolean with NO default (default: null), so it is absent from the JSON body unless rotation state exists for the secret. The recording confirms it: for a secret created without rotation (and across repeated describes of it), the response carries no RotationEnabled key at all, exactly as it carries no RotationLambdaARN/RotationRules/LastRotatedDate/NextRotationDate.
Emulator: the key is always present, serialized as false, for a never-rotated secret — on every DescribeSecret of it (the cluster covers four describes in the fixture).
Why: the presence of RotationEnabled is the documented signal 'rotation has been configured at some point on this secret'; clients use `"RotationEnabled" in resp` to tell never-configured from explicitly-disabled. Materializing false erases that distinction, and reconcilers that diff DescribeSecret output see a phantom field on every pass.
Fix: the DescribeSecret response serializer / rotation-metadata state — omit RotationEnabled (do not default it) when the secret has no rotation configuration.
Traces: secretsmanager-never-rotated-secret-omits-every-rotation-field#task#2 (also task#4, task#6, task#8 of the same fixture).

=== Cause 6 of 6: the ListSecrets filter-key validation message lists the enum value set in model-declaration order instead of AWS's order ===
AWS: Smithy ListSecrets.Filters -> FiltersListType, whose Key targets the enum FilterNameStringType; an out-of-set key is refused with the standard framework validation message, HTTP 400. Its fixed template is "<N> validation error(s) detected: Value '<value>' at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [<set>]", where the variable parts are the offending value, the request path and the count — and <set> is AWS's own ordering of the value set: [all, owning-service, name, tag-key, description, tag-value, primary-region].
Emulator: same status and template, but renders <set> in the model's declaration order: [description, name, tag-key, tag-value, primary-region, owning-service, all]. (Replay step: Value 'Name' at 'filters.1.member.key' — those are the request's own values; only the bracketed set differs.)
Why: the validation sentence is a requirement like any error message; tests, docs fixtures and error-parsing clients compare it literally, so an emulator-ordered set fails a byte-exact comparison even though the request was rejected for the right reason.
Fix: the ListSecrets filter validation — emit the enum value set in AWS's fixed order rather than iterating the generated enum.
Traces: secretsmanager-refarch-aur-master-secret-lookup-and-rotation-probe#task#3.

Related open issues (do NOT cover these causes): 20261029T224105Z-secretsmanager-secrets-manager-refusal-identity-and-wording-diverge-on.md already owns the StopReplicationToReplica and RotateSecret refusal divergences in this corpus, and 20261029T224105Z-secretsmanager-secrets-manager-getresourcepolicy-echoes-the-attached-policy-document.md owns the GetResourcePolicy re-serialization divergence; both were skipped here. 20261024T190718Z-secretsmanager-secrets-manager-replication-delete-lifecycle-access-metadata-and.md covers RotationEnabled AFTER a delete (behavioral corpus), not the never-configured omission in cause 5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator: 216 steps, 200 passed, 16 mismatched, score 92.6%, bootstrap_signal=false (emulator serving).

Cause 1 — secretsmanager-ecsm-pending-deletion-describe-and-restore-route#task#8, DeleteSecret: status expected 200, got 400; $.ARN missing (expected the secret's qualified ARN, got None); $.Name missing (expected the created name, got None); $.DeletionDate missing (expected a timestamp, got None).
Cause 2 — same fixture, task#7, CreateSecret, 400 -> 400, $.Message value_mismatch: expected "You can't create this secret because a secret with this name is already scheduled for deletion.", got "You can't create this secret because a secret with the name sim2real-probe-ecsm-del-97b77475 is already scheduled for deletion."
Cause 3 — secretsmanager-ecsm-get-secret-value-version-selector-refusals#task#4, GetSecretValue, 400 -> 400, $.Message value_mismatch: expected "Secrets Manager can't find the specified secret value for VersionId: <versionId>", got "Secrets Manager can't find the specified secret version: <versionId>" (replay versionId db8dd515-db54-4a20-bc8a-4e561dae27c2).
Cause 4 — same fixture, task#6, GetSecretValue, 400 -> 400, $.__type value_mismatch: expected 'InvalidRequestException', got 'ResourceNotFoundException'; $.Message value_mismatch: expected 'You provided a VersionStage that is not associated to the provided VersionId.', got "Secrets Manager can't find the specified secret version: <versionId>".
Cause 5 — secretsmanager-never-rotated-secret-omits-every-rotation-field#task#2 (and task#4, task#6, task#8), DescribeSecret, 200 -> 200, $.RotationEnabled unexpected_present: expected absent, got False.
Cause 6 — secretsmanager-refarch-aur-master-secret-lookup-and-rotation-probe#task#3, ListSecrets, 400 -> 400, $.message value_mismatch: expected "... Member must satisfy enum value set: [all, owning-service, name, tag-key, description, tag-value, primary-region]", got "... Member must satisfy enum value set: [description, name, tag-key, tag-value, primary-region, owning-service, all]" (the emulator's order equals the model's FilterNameStringType declaration order).

## Steps to Reproduce
Replay the fixtures named per cause (trace set simparity-wire-aws-v1), or by hand against the emulator's Secrets Manager endpoint:

Causes 1-2 (fixture secretsmanager-ecsm-pending-deletion-describe-and-restore-route): CreateSecret(Name=<n>, SecretString=...); DeleteSecret(SecretId=<n>, RecoveryWindowInDays=<w>); DescribeSecret(<n>); CreateSecret(Name=<n>) again and compare $.Message with AWS's fixed sentence (cause 2); then DeleteSecret(SecretId=<n>) again — expect 200 with ARN/Name/DeletionDate, emulator answers 400 with none of them (cause 1).
Causes 3-4 (fixture secretsmanager-ecsm-get-secret-value-version-selector-refusals): CreateSecret then PutSecretValue to get two versions; GetSecretValue(SecretId=<n>, VersionId=<uuid not a version of the secret>) and compare $.Message (cause 3); GetSecretValue(SecretId=<n>, VersionId=<version A>, VersionStage=<stage attached to version B>) and compare $.__type and $.Message (cause 4).
Cause 5 (fixture secretsmanager-never-rotated-secret-omits-every-rotation-field): CreateSecret with no rotation configuration, then DescribeSecret and assert 'RotationEnabled' is not a key of the response body (repeat after PutSecretValue/tag updates).
Cause 6 (fixture secretsmanager-refarch-aur-master-secret-lookup-and-rotation-probe): ListSecrets(Filters=[{Key:'name',Values:[...]},{Key:'Name',Values:[...]}]) — a key outside FilterNameStringType in position 1 — and compare the bracketed enum value set in $.message with AWS's order.
