Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes codebuild-cbnv-batch-get-malformed-requests codebuild-cbnv-build-history-route
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|BatchGetProjects|4xx->2xx|$.__type
Issue-Key: codebuild|BatchGetProjects|4xx->2xx|$.message
Issue-Key: codebuild|BatchGetProjects|4xx->2xx|status
Issue-Key: codebuild|BatchGetProjects|4xx->2xx|$.Message
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projectsNotFound[*]
Issue-Key: codebuild|BatchGetProjects|2xx->2xx|$.projects[*]
Issue-Key: codebuild|ListBuildsForProject|4xx->4xx|$.__type
Issue-Key: codebuild|ListBuildsForProject|2xx->4xx|$.ids
Issue-Key: codebuild|ListBuildsForProject|2xx->4xx|status
Issue-Related: 20261029T175929Z-codebuild-absent-resource-and-invalid-input-error-messages.md
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CodeBuild does not resolve or validate ARN-addressed project references (5 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetProjects

## Description
FIVE net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006) sharing one fault: the emulator treats a project identifier as an opaque NAME. It never parses an ARN-shaped identifier, so it neither resolves a valid project ARN nor refuses an ARN whose region or account is wrong. ALL FIVE must be closed before this issue is resolved.

Cause 1 of 5: BatchGetProjects accepts a project ARN whose region is not the caller's region.
AWS: Smithy BatchGetProjects(names @required ProjectNames @length 1-100, member NonEmptyString) -> {projects, projectsNotFound}; modeled error InvalidInputException (client, HTTP 400). Because the member is NonEmptyString and not the ProjectName pattern, an entry may be a project ARN; AWS parses it and refuses a cross-region ARN with HTTP 400, __type 'InvalidInputException' and the message "Invalid project ARN: region does not match caller's region" (that message is entirely fixed text — it names neither the ARN nor the region).
Emulator: HTTP 200 with a success body — no __type and no message at all. The cross-region ARN is simply looked up as a name, misses, and the request succeeds.
Why: a client that mistypes or templates the wrong region into a project ARN gets a 200 instead of a 400, so the misconfiguration is invisible; any test asserting the refusal passes against the emulator and fails against AWS.
Fix: the project-identifier resolver shared by the BatchGetProjects handler's input validation — parse an ARN-shaped name and compare its region to the request's region before lookup, raising InvalidInputException.
Traces: codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes#task#4 (same cluster also covers that fixture's task#5 and codebuild-cbnv-batch-get-malformed-requests#task#6).

Cause 2 of 5: BatchGetProjects accepts a project ARN belonging to another account instead of denying it.
AWS: recorded behavior only for the error class — BatchGetProjects models only InvalidInputException, but the recording shows a cross-account project ARN refused with HTTP 400, __type 'AccessDeniedException' and a capital-M 'Message' member whose fixed text is 'User: <caller principal ARN> is not authorized to perform: codebuild:BatchGetProjects on resource: <the project ARN from the request> because no resource-based policy allows the codebuild:BatchGetProjects action'. The variable parts are the caller's principal ARN and the ARN supplied in the request; the rest, including the action name repeated twice, is fixed.
Emulator: HTTP 200 with a success body and neither __type nor Message.
Why: cross-account authorization is unobservable — a client cannot develop or test the deny path, and code that distinguishes AccessDeniedException from a not-found result takes the wrong branch.
Fix: the same project-identifier resolver plus the authorization check in the BatchGetProjects handler — an ARN whose account differs from the caller's must produce AccessDeniedException with the 'Message' member (not 'message').
Traces: codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes#task#6.

Cause 3 of 5: BatchGetProjects reports an existing project as not found when it is addressed by its own ARN.
AWS: a names entry that is the project's full ARN in the caller's own region and account resolves exactly as the bare name does: the project structure is returned in `projects` and `projectsNotFound` stays empty.
Emulator: HTTP 200 with `projects` empty and the submitted ARN echoed in `projectsNotFound[0]`.
Why: the ARN is the identifier every other API (and CloudFormation/IAM output) hands back, so a client that pipes an ARN into BatchGetProjects concludes the project it just created does not exist.
Fix: the project lookup behind the BatchGetProjects handler — when a name parses as a codebuild project ARN for this region/account, resolve on its resource name.
Traces: codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes#task#7.

Cause 4 of 5: ListBuildsForProject reports a cross-region project ARN as not found instead of refusing the ARN.
AWS: Smithy ListBuildsForProject(projectName @required NonEmptyString @length min 1) -> {ids, nextToken}; errors InvalidInputException and ResourceNotFoundException (both client, HTTP 400). A projectName that is an ARN whose region is not the caller's region is an INPUT error: HTTP 400, __type 'InvalidInputException', message "Invalid project ARN: region does not match caller's region" (fixed text).
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and message 'Project cannot be found: arn:aws:codebuild:<caller region>:<caller account>:project/<the entire submitted value>' — i.e. the supplied ARN is pasted verbatim into an ARN template, producing a nested, nonsensical ARN in the message and the wrong error code.
Why: the error code drives client handling — a caller retries/creates on ResourceNotFoundException but must fix its input on InvalidInputException — and the message leaks a malformed ARN that corresponds to no resource.
Fix: the projectName resolver and error mapping in the ListBuildsForProject handler — parse the ARN, validate region before lookup, and never interpolate an already-ARN value into an ARN template.
Traces: codebuild-cbnv-build-history-route#task#7.

Cause 5 of 5: ListBuildsForProject refuses a project reference AWS resolves, instead of returning an empty build list.
AWS: for an existing project that has no builds, ListBuildsForProject answers HTTP 200 with `ids` present and empty ([]).
Emulator: HTTP 400 (ResourceNotFoundException-class refusal) with no `ids` member.
Hypothesis: the step addresses the project by its full same-region/same-account ARN — the adjacent step in the same fixture uses a cross-region ARN, and the emulator's own message in Cause 4 shows identifiers are never parsed as ARNs — so the lookup misses a project that exists.
Why: history/pagination code that lists builds for a freshly created project dies on a 400 where AWS hands back an empty page, so no caller can distinguish 'no builds yet' from 'no such project'.
Fix: the ListBuildsForProject handler's project resolution (same resolver as Cause 4) and its empty-result path, which must serialize ids as an empty list rather than refusing.
Traces: codebuild-cbnv-build-history-route#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (165 steps, 110 passed, 55 mismatched, score 66.7%). None of these five clusters is owned by an open issue or blocked by a stub.

Cause 1 - codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes#task#4 BatchGetProjects: status expected 400, got 200; $.__type missing (expected 'InvalidInputException'); $.message missing (expected "Invalid project ARN: region does not match caller's region"). Cluster also covers task#5 of the same fixture and codebuild-cbnv-batch-get-malformed-requests#task#6.
Cause 2 - codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes#task#6 BatchGetProjects: status expected 400, got 200; $.__type missing (expected 'AccessDeniedException'); $.Message missing (expected 'User: arn:aws:iam::<account>:user/<caller> is not authorized to perform: codebuild:BatchGetProjects on resource: arn:aws:codebuild:<region>:<other account>:project/<project name> because no resource-based policy allows the codebuild:BatchGetProjects action').
Cause 3 - codebuild-cbnv-batch-get-by-arn-region-and-account-mistakes#task#7 BatchGetProjects, 200/200: $.projects[0] missing (expected the full Project structure for the project created earlier in the fixture, addressed by its ARN 'arn:aws:codebuild:<region>:<account>:project/<name>'); $.projectsNotFound[0] extra, got that same ARN.
Cause 4 - codebuild-cbnv-build-history-route#task#7 ListBuildsForProject, 400/400: $.__type expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message expected "Invalid project ARN: region does not match caller's region", got 'Project cannot be found: arn:aws:codebuild:us-west-2:000000000000:project/arn:aws:codebuild:eu-west-3:000000000000:project/<name>' (note the submitted eu-west-3 ARN nested inside a locally built us-west-2 ARN).
Cause 5 - codebuild-cbnv-build-history-route#task#4 ListBuildsForProject: status expected 200, got 400; $.ids missing (expected []).

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json - BatchGetProjects(names @required ProjectNames @length 1-100, member NonEmptyString @length min 1 -> projects/projectsNotFound; error InvalidInputException 400); ListBuildsForProject(projectName @required NonEmptyString @length min 1 -> ids/nextToken; errors InvalidInputException 400, ResourceNotFoundException 400). ProjectName's pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$ applies to Project.name only, NOT to the identifier members of these two operations, which is why an ARN is a legal value there. AccessDeniedException is not in the model's BatchGetProjects error list - that error class is recorded behavior only.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the codebuild divergences, or reproduce each cause directly:
3. Cause 1: CreateProject(name=<p>), then BatchGetProjects(names=['arn:aws:codebuild:<other region>:<caller account>:project/<p>']) - expect HTTP 400 InvalidInputException "Invalid project ARN: region does not match caller's region"; observe HTTP 200.
4. Cause 2: BatchGetProjects(names=['arn:aws:codebuild:<caller region>:111111111111:project/<p>']) - expect HTTP 400 AccessDeniedException with the 'Message' member naming the caller principal, the resource ARN and the codebuild:BatchGetProjects action; observe HTTP 200.
5. Cause 3: BatchGetProjects(names=['arn:aws:codebuild:<caller region>:<caller account>:project/<p>']) - expect the project in `projects`; observe it echoed in `projectsNotFound`.
6. Cause 4: ListBuildsForProject(projectName='arn:aws:codebuild:eu-west-3:<caller account>:project/<p>') - expect HTTP 400 InvalidInputException with the invalid-ARN-region message; observe ResourceNotFoundException with the submitted ARN nested inside a locally built ARN.
7. Cause 5: ListBuildsForProject(projectName='arn:aws:codebuild:<caller region>:<caller account>:project/<p>') for a project with no builds - expect HTTP 200 with ids: []; observe HTTP 400.
