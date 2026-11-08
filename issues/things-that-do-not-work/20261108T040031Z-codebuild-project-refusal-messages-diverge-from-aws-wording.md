Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-project-registry-and-absent-project-errors
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-project-registry-and-absent-project-errors
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|CreateProject|4xx->4xx|$.message
Issue-Key: codebuild|InvalidateProjectCache|4xx->4xx|$.message
Issue-Key: codebuild|UpdateProjectVisibility|4xx->4xx|$.message
Issue-Commit: 8b0f35809934c567f1bf7b94b8ce8845151fefb8

# CodeBuild project refusal messages diverge from AWS wording on CreateProject, InvalidateProjectCache and UpdateProjectVisibility (3 causes)

> _Note: this issue was identified against the code base at commit `8b0f35809934c567f1bf7b94b8ce8845151fefb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProject

## Description
THREE net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006). In each one the emulator answers the right HTTP status (400) and the right error code, but the WRONG message text, so a client that branches on, logs or displays AWS's wording misreads the refusal. An error message is part of the contract, and each operation's wording is its own defect, so ALL THREE must be closed before this issue is resolved.

In every cause the fixed text is what is quoted outside angle brackets; angle-bracketed parts are interpolated from the request (project name, project ARN) and must never be hard-coded from one recording. Recorded names/ARNs quoted under Evidence are per-run values filling the template, not requirements.

Related open issues that do NOT cover these wire steps (none of the three divergence clusters is reported as owned, and none of these issues carries a wire replay key for them): 20261029T204615Z-codebuild-error-message-wording-diverges-from-aws-on.md describes the same wording family but only from the BEHAVIORAL suite (its causes cover DeleteWebhook, UpdateProject, ListBuildBatchesForProject, GetReportGroupTrend, ListBuilds and CreateWebhook — not the three operations below) and carries only behavioral check ids; 20261107T192445Z-codebuild-codepipeline-ecr-4-net-new-causes-createproject.md describes the SAME CreateProject service-role message as its cause 1, but again only from a behavioral check id, so this wire fixture step is un-gated by it; 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md covers absent-resource calls answered 200, a different fault. Each cause below must be replayed against the fixture step named in its Traces line.

Cause 1 of 3: CreateProject with a serviceRole whose trust policy does not allow codebuild.amazonaws.com to assume it is refused with the right class but a rewritten remediation message.
AWS: Smithy CreateProject(name @required ProjectName @length 2-150 @pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$, source @required, artifacts @required, environment @required, serviceRole @required NonEmptyString @length min 1) -> {project}; modeled errors AccountLimitExceededException, InvalidInputException, ResourceAlreadyExistsException (all errorKind client, httpError 400). Recorded response: HTTP 400, __type InvalidInputException, message entirely FIXED text with no request-derived part: "CodeBuild is not authorized to perform: sts:AssumeRole on service role. Please verify that: 1) The provided service role exists, 2) The role name is case-sensitive and matches exactly, and 3) The role has the necessary trust policy configured."
Emulator: HTTP 400 with the correct __type InvalidInputException but its own two-clause rewrite: "CodeBuild is not authorized to perform: sts:AssumeRole on service role. Please verify that: 1) The provided service role exists, 2) The service role's trust policy allows codebuild.amazonaws.com to assume it." — AWS's case-sensitivity clause and its third numbered clause are dropped and clause 2 is reworded.
Why: a client or operator that matches on, or surfaces, AWS's remediation text gets emulator prose instead, and the case-sensitivity hint AWS gives for the most common CodeBuild onboarding failure is lost; exact-match assertions on the documented message never succeed.
Fix: the message construction in the CreateProject handler's service-role trust validation.
Traces: codebuild-project-registry-and-absent-project-errors#task#6.

Cause 2 of 3: InvalidateProjectCache on an absent project drops the project name AND the trailing period from the not-found message.
AWS: Smithy InvalidateProjectCache(projectName @required NonEmptyString @length min 1) -> {}; modeled errors InvalidInputException, ResourceNotFoundException (client, httpError 400). Recorded response: HTTP 400 with message 'Project does not exist: <projectName>.' — fixed text 'Project does not exist: ' and a trailing '.' around the project NAME taken verbatim from the request (not an ARN).
Emulator: HTTP 400 with the constant message 'Project does not exist' — the requested name and the trailing period are both missing.
Why: the caller is never told WHICH project was missing, which is the whole diagnostic value when a cache is invalidated across many projects in one loop; and the message is byte-different from AWS's, so message-matching clients and log assertions diverge.
Fix: the not-found message template in the InvalidateProjectCache handler's project lookup.
Traces: codebuild-project-registry-and-absent-project-errors#task#8.

Cause 3 of 3: UpdateProjectVisibility on an absent project uses the emulator's ARN-interpolating template where AWS emits a constant string.
AWS: Smithy UpdateProjectVisibility(projectArn @required NonEmptyString @length min 1, projectVisibility @required ProjectVisibilityType) -> {projectArn, publicProjectAlias, projectVisibility}; modeled errors InvalidInputException, ResourceNotFoundException (client, httpError 400). Recorded response: HTTP 400 with message exactly 'Project does not exist.' — wholly fixed text WITH a trailing period and no interpolated name or ARN.
Emulator: HTTP 400 with message 'Project cannot be found: <project ARN>' — different wording, no trailing period, and it leaks the requested ARN into a message AWS keeps constant.
Why: AWS's wording differs per operation (cause 2 interpolates the name, this one interpolates nothing), so one shared emulator template cannot be correct for both; a client comparing messages sees a string no AWS account emits, and because the emulator interpolates the ARN the message varies per project where AWS's does not.
Fix: the not-found message emitted by the UpdateProjectVisibility handler's project lookup — the constant string, not the shared ARN-interpolating not-found template.
Traces: codebuild-project-registry-and-absent-project-errors#task#9.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200): 165 steps, 133 passed, 29 mismatched, score 80.6%. All three causes below are mismatch clusters that the replay reports as NOT owned by any open issue and NOT blocked by a stub.

Cause 1 — fixture codebuild-project-registry-and-absent-project-errors, step task#6, operation CreateProject, verdict mismatch (expected status 400, actual 400): $.message value_mismatch — expected 'CodeBuild is not authorized to perform: sts:AssumeRole on service role. Please verify that: 1) The provided service role exists, 2) The role name is case-sensitive and matches exactly, and 3) The role has the necessary trust policy configured.', got "CodeBuild is not authorized to perform: sts:AssumeRole on service role. Please verify that: 1) The provided service role exists, 2) The service role's trust policy allows codebuild.amazonaws.com to assume it." The error code matched (InvalidInputException); the message is the only divergence.

Cause 2 — fixture codebuild-project-registry-and-absent-project-errors, step task#8, operation InvalidateProjectCache, verdict mismatch (expected status 400, actual 400): $.message value_mismatch — expected 'Project does not exist: sim2real-probe-codebuild-codebuild-absent-proj-74c4d814.', got 'Project does not exist'. The fixed text is 'Project does not exist: ' + the requested projectName + '.'; the quoted probe name is that run's request value, not part of the requirement.

Cause 3 — fixture codebuild-project-registry-and-absent-project-errors, step task#9, operation UpdateProjectVisibility, verdict mismatch (expected status 400, actual 400): $.message value_mismatch — expected 'Project does not exist.', got 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/sim2real-probe-codebuild-codebuild-absent-proj-74c4d814'. The expected message is wholly fixed text; the ARN in the actual message is a per-run value the emulator's template interpolated.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — CreateProject, InvalidateProjectCache and UpdateProjectVisibility signatures, required members and modeled errors (InvalidInputException / ResourceNotFoundException, errorKind client, httpError 400 defaulted).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) and read the codebuild mismatches for fixture codebuild-project-registry-and-absent-project-errors, steps task#6, task#8 and task#9. Or reproduce each cause directly against the emulator's codebuild endpoint:
3. Cause 1 (fixture codebuild-project-registry-and-absent-project-errors#task#6): iam CreateRole with a trust policy whose Principal.Service is NOT codebuild.amazonaws.com (e.g. ec2.amazonaws.com), then codebuild CreateProject(name=<valid name>, source={type:NO_SOURCE,buildspec:<minimal>}, artifacts={type:NO_ARTIFACTS}, environment={type:LINUX_CONTAINER,image:aws/codebuild/amazonlinux2-x86_64-standard:5.0,computeType:BUILD_GENERAL1_SMALL}, serviceRole=<that role ARN>) and read Error.Code/Error.Message — expect 400 InvalidInputException with AWS's three-clause remediation text including the case-sensitivity clause.
4. Cause 2 (…#task#8): codebuild InvalidateProjectCache(projectName=<a name that was never created, or one just deleted>) — expect 400 with message 'Project does not exist: <that projectName>.' (name interpolated, trailing period); observe the constant 'Project does not exist'.
5. Cause 3 (…#task#9): codebuild UpdateProjectVisibility(projectArn=arn:aws:codebuild:<region>:<account>:project/<absent project>, projectVisibility=PRIVATE) — expect 400 with message exactly 'Project does not exist.'; observe 'Project cannot be found: <the ARN from the request>'.
