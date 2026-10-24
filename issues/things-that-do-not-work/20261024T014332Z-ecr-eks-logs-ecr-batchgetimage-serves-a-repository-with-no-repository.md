Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected
Issue-Key: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected|_error#19|AssertionError|raise AssertionError(f"expected the call to be denied, but it succeeded: {result!r}")
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ECR BatchGetImage serves a repository with no repository policy to a foreign principal instead of denying it

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetImage

## Description
One cause.

Cause 1 of 1: a cross-account pull against a repository that has no repository policy is served instead of denied.

AWS: Smithy: BatchGetImage is a sigv4 operation returning images/failures; access to a repository's images is governed by the repository policy plus the caller's identity policy. A principal outside the repository's own grants — here a role with no identity-policy grant and a repository carrying no repository policy at all — is refused: AWS answers HTTP 400 with an access-denied error (recorded example code `AccessDeniedException`) and returns no manifest. Adding a repository policy that allows that principal ecr:BatchGetImage is what makes the same call succeed.

Emulator: the same BatchGetImage returns HTTP 200 with `images[0].imageManifest` fully populated and `failures: []`, so the pull succeeds although nothing granted it. The check's negative leg therefore fails with `expected the call to be denied, but it succeeded`.

Why: image contents are readable by any caller the emulator can authenticate, so no client can validate that a private repository is actually private, and code paths that depend on a denial (fallback registries, cross-account bootstrap errors, EKS node pull failures surfaced as ImagePullBackOff) never trigger. A least-privilege regression in the client's own policies is invisible when tested here.

Fix: the request-authorization layer for ECR data-plane reads — repository-policy and identity-policy evaluation ahead of the BatchGetImage handler (the same gate is missing on the control plane; see the ECR issue filed in this sweep for CreateRepository, and the related open issue 20261022T234922Z-ecr-ecs-iam-ecr-setrepositorypolicy-accepts-a-policy-whose-principal-names.md).

Traces: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected.

## Evidence
Expected: an access-denied ClientError (recorded code `AccessDeniedException`, HTTP 400) from BatchGetImage for a principal with no grant on a repository with no repository policy.
Actual: AssertionError: expected the call to be denied, but it succeeded: {'images': [{'registryId': '000000000000', 'repositoryName': 'sim2real-xacct-repo-dc17ba59f220', 'imageId': {'imageDigest': 'sha256:918a0e02...', 'imageTag': 'sim2real-test'}, 'imageManifest': '{...}', 'imageManifestMediaType': 'application/vnd.docker.distribution.manifest.v2+json'}], 'failures': [], 'ResponseMetadata': {'HTTPStatusCode': 200}}.

## Steps to Reproduce
1. Create an ECR repository and push an image to it; do NOT set a repository policy.
2. Create an IAM role with no identity-based permissions on ECR and obtain credentials for it (sts assume-role).
3. Call ecr.batch_get_image(repositoryName=<repo>, imageIds=[{'imageTag': 'sim2real-test'}]) with those credentials.
4. Observe HTTP 200 with the full manifest instead of an access-denied error.
