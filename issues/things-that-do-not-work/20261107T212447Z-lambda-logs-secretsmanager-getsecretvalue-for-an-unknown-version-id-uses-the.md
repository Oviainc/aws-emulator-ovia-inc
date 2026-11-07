Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation
Issue-Key: lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation|run#36|AssertionError|assert absent_message == f"Secrets Manager can't find the specified secret value for VersionId: {intruder}", (
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# GetSecretValue for an unknown version id uses the wrong not-found message

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Cause 1 of 1: GetSecretValue with a VersionId that does not exist is refused with a message about the *secret* being missing rather than AWS's message for a missing secret *version*.

AWS: recorded behavior only. Secrets Manager distinguishes the two not-found classes: a missing secret is refused with ResourceNotFoundException and the sentence "Secrets Manager can't find the specified secret.", while a VersionId that the secret does not have is refused with ResourceNotFoundException and a sentence that names the version, AWS's fixed text for that class with the VersionId taken from the request. The recording drives a failed FinishSecret so the candidate version stays AWSPENDING and then reads the stale version id.

Emulator: the version-level refusal is reported with the emulator's own sentence. Replay detail: `AssertionError: unexpected GetSecretValue not-found message: "Secrets Manager can't find the specified secret version: effdf619-e494-4c39-afc2-754617690cd9"` — the fixed part of that sentence is emulator wording; only the UUID is a request value.

Why: a client that distinguishes "this secret is gone" from "this version is gone" by message text (the error code is ResourceNotFoundException in both cases, so the message is the only discriminator) mis-handles a rotation that left a stale version behind: it will delete or recreate the secret instead of retrying against AWSCURRENT.

Fix: the GetSecretValue handler's version lookup error path — the ResourceNotFoundException message for an unknown VersionId must use AWS's wording for that class, with the VersionId substituted from the request.

Traces: lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation.

## Evidence
Expected: ResourceNotFoundException whose message is AWS's text for a missing secret *version*, naming the requested VersionId.
Actual: ResourceNotFoundException with message "Secrets Manager can't find the specified secret version: <versionId>" — emulator wording for the fixed part of the sentence.

## Steps to Reproduce
1. CreateSecret and start a rotation whose FinishSecret step fails, leaving the candidate version labelled AWSPENDING.
2. Call GetSecretValue with a VersionId the secret does not have.
3. Compare the ResourceNotFoundException message with AWS's message for an unknown secret version.
