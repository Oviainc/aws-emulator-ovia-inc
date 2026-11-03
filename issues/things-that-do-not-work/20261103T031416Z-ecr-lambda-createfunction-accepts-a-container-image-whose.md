Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__lambda.container-image-repository-policy-deny-rejected
Issue-Key: ecr__lambda.container-image-repository-policy-deny-rejected|run#11|AssertionError|raise AssertionError("expected CreateFunction to be refused, but it succeeded")
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# ecr__lambda: CreateFunction accepts a container image whose ECR repository policy denies Lambda, instead of refusing with AccessDeniedException

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: CreateFunction with `PackageType=Image` succeeds even when the image's ECR repository policy denies `lambda.amazonaws.com` the pull actions, so no permission check is performed against the image source.
AWS: Lambda pulls the image at create time using the function's service principal. When the repository policy does not allow `ecr:BatchGetImage` / `ecr:GetDownloadUrlForLayer` to `lambda.amazonaws.com`, CreateFunction is refused with HTTP 403 `AccessDeniedException`, message `Lambda does not have permission to access the ECR image. Check the ECR permissions.` — that whole sentence is fixed text, with no variable parts. The refused function does not exist afterwards: GetFunction answers `ResourceNotFoundException` with `Function not found: arn:<partition>:lambda:<region>:<account>:function:<name>` (fixed text `Function not found: ` plus the ARN built from the request and account). Smithy: lambda CreateFunction models the client-error family for this (400/403-class refusals) and the emulator's own model carries AccessDeniedException for image access.
Emulator: CreateFunction SUCCEEDS; replay detail `AssertionError: expected CreateFunction to be refused, but it succeeded`. The function is created and reports Active despite an image Lambda could never pull.
Why: a client whose ECR repository policy is wrong gets a function that appears healthy and only fails (if at all) at invoke time, with no diagnostic pointing at the repository policy. Worse, the emulator is over-permissive on a cross-service authorization boundary, so a scenario that is supposed to prove the deny is enforced proves nothing.
Fix: the lambda CreateFunction handler's image-source resolution — evaluate the ECR repository policy for the Lambda service principal before accepting the function, and emit AccessDeniedException with AWS's message while creating nothing.
Traces: ecr__lambda.container-image-repository-policy-deny-rejected

## Evidence
Expected: `CreateFunction(PackageType='Image', Code={'ImageUri': <repo-with-deny-policy>@<digest>})` -> `AccessDeniedException`, message `Lambda does not have permission to access the ECR image. Check the ECR permissions.`; and a following `GetFunction` -> `ResourceNotFoundException`, message `Function not found: arn:<partition>:lambda:<region>:<account>:function:<name>`.
Actual: CreateFunction returned success and the function exists (replay: `AssertionError: expected CreateFunction to be refused, but it succeeded`).

## Steps to Reproduce
1. Replay the behavioral suite and re-run ecr__lambda.container-image-repository-policy-deny-rejected.
2. Create an ECR repository, push an image, then PutRepositoryPolicy with a statement that Denies ecr:BatchGetImage/ecr:GetDownloadUrlForLayer to Principal `lambda.amazonaws.com`.
3. Call CreateFunction with PackageType=Image and that image URI; observe success instead of AccessDeniedException.
