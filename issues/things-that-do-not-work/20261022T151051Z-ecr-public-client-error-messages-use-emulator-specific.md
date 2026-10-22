Issue-Source: trace-sweep-divergence
Issue-Trace: api-image-layer-upload-and-push-workflow
Issue-Step: task#13
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-image-layer-upload-and-push-workflow api-repository-creation-catalog-lifecycle api-repository-deletion-and-non-empty-error-paths
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public|PutImage|4xx->4xx|$.message
Issue-Key: ecr-public|DescribeImages|4xx->4xx|$.message
Issue-Key: ecr-public|CreateRepository|4xx->4xx|$.message
Issue-Key: ecr-public|DeleteRepository|4xx->4xx|$.message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECR Public client-error messages use emulator-specific templates instead of the AWS wording

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutImage

## Description
Four ecr-public client errors return the correct HTTP status (400) and the correct modeled exception, but the $.message body is produced by a different template than the one AWS fills from the request. In each case the recorded message is a fixed sentence interpolating only values taken from the request (repository name, registry id, image digest, image tag), so the template IS the rule and is reproducible — the emulator rephrases it, drops request-derived fields it should name, or adds state-derived detail AWS never includes. All four must be closed before this issue is resolved.

Cause 1 of 4: PutImage / ImageDigestDoesNotMatchException message.
AWS: Smithy PutImage error ImageDigestDoesNotMatchException (client, httpError 400). Recorded behavior: 400 with message "The image with digest '<imageDigest from the request>' in repository '<repositoryName from the request>' and registry ID '<registryId>' does not match the image digest that Amazon ECR has calculated." — i.e. the request's digest, repository and registry id, and no mention of the computed digest.
Emulator: 400 with "The image digest '<requested digest>' does not match the digest of the supplied manifest ('<computed digest>')" — omits repository and registry id and leaks the computed digest.
Why: the message is the only place these errors carry which repository/registry the rejection applies to; a client that logs or surfaces it (or matches on the AWS sentence to distinguish a digest mismatch from a manifest-parse failure) loses that context, and tooling pinned to the AWS wording stops recognizing the condition.
Fix: the PutImage handler's digest-verification error construction — build the message from the request's imageDigest, repositoryName and resolved registryId using the AWS template.
Traces: api-image-layer-upload-and-push-workflow#task#13.

Cause 2 of 4: DescribeImages / ImageNotFoundException message.
AWS: Smithy DescribeImages error ImageNotFoundException (client, httpError 400). Recorded behavior: 400 with message "The image with imageId {imageDigest:'<requested digest>', imageTag:'<requested tag or null>'} does not exist within the repository with name '<repositoryName>' in the registry with id '<registryId>'" — the whole requested ImageIdentifier rendered in brace form, plus repository name and registry id.
Emulator: 400 with "The image with digest '<digest>' and tag 'null' does not exist within the repository with name '<repositoryName>'" — different identifier rendering and the registry id dropped.
Fix: the DescribeImages not-found error construction.
Traces: api-image-layer-upload-and-push-workflow#task#14.

Cause 3 of 4: CreateRepository / RepositoryAlreadyExistsException message.
AWS: Smithy CreateRepository error RepositoryAlreadyExistsException (client, httpError 400). Recorded behavior: 400 with message "The repository with name '<repositoryName>' already exists in the registry with id '<registryId>'".
Emulator: 400 with "Repository with name '<repositoryName>' already exists in the registry with id '<registryId>'" — same interpolated values, missing the leading "The ".
Fix: the CreateRepository conflict error string.
Traces: api-repository-creation-catalog-lifecycle#task#6.

Cause 4 of 4: DeleteRepository / RepositoryNotEmptyException message.
AWS: Smithy DeleteRepository error RepositoryNotEmptyException (client, httpError 400), raised when force is false and images remain. Recorded behavior: 400 with message "The repository with name '<repositoryName>' in registry with id '<registryId>' cannot be deleted because it still contains images" — a fixed tail with no count.
Emulator: 400 with "The repository with name '<repositoryName>' in the registry with id '<registryId>' cannot be deleted because it still contains 1 image(s)" — inserts "the" and appends a state-derived image count AWS does not report.
Why (causes 2-4): these messages are deterministic functions of the request, so clients and conformance suites treat them as stable; the emulator's variants differ structurally (dropped registry id, different identifier syntax, an extra count that changes with state), so assertions and log scrapers that hold against AWS fail against the emulator even though status and error code are right.
Fix (causes 2-4): the respective handlers' error-construction sites; emit the AWS template interpolating only request-derived values.
Traces: api-repository-deletion-and-non-empty-error-paths#task#11.

Note: statuses and error codes are already correct for all four — only the message template diverges, which is why this is filed as LOW.

## Evidence
All four clusters: expected_status 400, actual_status 400, verdict mismatch, divergence confined to $.message.
Cause 1 — PutImage, api-image-layer-upload-and-push-workflow#task#13: expected "The image with digest 'sha256:0000...0000' in repository 'sim2real-probe-ecr-public-push-repo-02c1a31d' and registry ID '000000000000' does not match the image digest that Amazon ECR has calculated."; got "The image digest 'sha256:0000...0000' does not match the digest of the supplied manifest ('sha256:a8b4056...50efe2')".
Cause 2 — DescribeImages, api-image-layer-upload-and-push-workflow#task#14: expected "The image with imageId {imageDigest:'sha256:1111...1111', imageTag:'null'} does not exist within the repository with name '<repo>' in the registry with id '000000000000'"; got "The image with digest 'sha256:1111...1111' and tag 'null' does not exist within the repository with name '<repo>'".
Cause 3 — CreateRepository, api-repository-creation-catalog-lifecycle#task#6: expected "The repository with name '<repo>' already exists in the registry with id '000000000000'"; got "Repository with name '<repo>' already exists in the registry with id '000000000000'".
Cause 4 — DeleteRepository, api-repository-deletion-and-non-empty-error-paths#task#11: expected "... in registry with id '000000000000' cannot be deleted because it still contains images"; got "... in the registry with id '000000000000' cannot be deleted because it still contains 1 image(s)".
The quoted repository names, digests and registry id are the recordings' example values; the requirement is the template over the request's own values. Replay context: simparity-wire-aws-v1, emulator_up true, state_reset_failures 0.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Cause 1: fixture api-image-layer-upload-and-push-workflow step task#13 — PutImage the probe manifest with imageDigest sha256:0000...0000 (a digest that is not the manifest's); compare the 400 body's message to the AWS template naming the requested digest, repository and registry ID.
3. Cause 2: same fixture step task#14 — DescribeImages with imageIds=[{imageDigest: sha256:1111...1111}] against the existing repository; compare the 400 ImageNotFoundException message to the AWS brace-form imageId template including the registry id.
4. Cause 3: fixture api-repository-creation-catalog-lifecycle step task#6 — CreateRepository with a name that already exists; compare the 400 RepositoryAlreadyExistsException message.
5. Cause 4: fixture api-repository-deletion-and-non-empty-error-paths step task#11 — DeleteRepository (force unset/false) on a repository that still holds an image; compare the 400 RepositoryNotEmptyException message, which must not carry an image count.
