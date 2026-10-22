Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild__secretsmanager.codebuild-secret-reference-denied-without-getsecretvalue-permission codebuild__secretsmanager.codebuild-secrets-manager-environment-reference-resolves-inside-the-build
Issue-Key: codebuild__secretsmanager.codebuild-secret-reference-denied-without-getsecretvalue-permission|run#23|AssertionError|assert ok_build["buildStatus"] == "SUCCEEDED", (
Issue-Key: codebuild__secretsmanager.codebuild-secrets-manager-environment-reference-resolves-inside-the-build|run#15|AssertionError|assert build["buildStatus"] == "SUCCEEDED", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeBuild/Secrets Manager: builds FAULT at PROVISIONING so a SECRETS_MANAGER environment reference is never resolved inside the build

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
One root cause behind both failing checks of the `codebuild__secretsmanager` cross-service suite (neither marked ALREADY FILED or OWNED).

Cause 1 of 1: builds never execute, so a SECRETS_MANAGER-typed environment variable is never resolved and the permission-denied path is unreachable.
AWS: a build walks the Smithy BuildPhaseType sequence and ends SUCCEEDED on exit 0 (Smithy StatusType); an environmentVariable of type SECRETS_MANAGER is resolved from the referenced secret before the build phases run, and a build whose service role lacks secretsmanager:GetSecretValue fails with that resolution error instead of succeeding.
Emulator: every build ends buildStatus FAULT with phases [SUBMITTED SUCCEEDED, QUEUED SUCCEEDED, PROVISIONING FAULT, COMPLETED] and the context message 'this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed' — the same outcome for the permitted control build and the denied one, so the two cases are indistinguishable.
Why: a client cannot verify that secret injection works, and cannot distinguish a missing permission from an infrastructure fault; AWS reserves FAULT for the latter.
Fix: the CodeBuild build state machine behind StartBuild and its environment-variable resolution step (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_build.rs plus the phase state batch_get_builds.rs reads back). Same underlying defect as the codebuild-service issue filed in this sweep.
Traces: codebuild__secretsmanager.codebuild-secret-reference-denied-without-getsecretvalue-permission, codebuild__secretsmanager.codebuild-secrets-manager-environment-reference-resolves-inside-the-build.

## Evidence
AssertionError: the permitted control build was 'FAULT'; phases=[('SUBMITTED','SUCCEEDED'),('QUEUED','SUCCEEDED'),('PROVISIONING','FAULT'),('COMPLETED',None)]; contexts='this emulator does not run CodeBuild build containers, so the build could not be provisioned and no buildspec was executed'. Same for the resolve case: "build was 'FAULT'".

## Steps to Reproduce
Boot the emulator from this checkout and replay the codebuild__secretsmanager suite, or: CreateSecret, create a project with an environmentVariable of type SECRETS_MANAGER pointing at it and a buildspec that echoes the variable, StartBuild and poll BatchGetBuilds to terminal — observe FAULT at PROVISIONING with no phase that could have resolved the secret.
