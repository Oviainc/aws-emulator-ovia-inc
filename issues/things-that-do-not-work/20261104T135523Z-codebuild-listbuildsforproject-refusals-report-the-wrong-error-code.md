Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-cbnv-build-history-route
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-cbnv-build-history-route
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|ListBuildsForProject|4xx->4xx|$.message
Issue-Related: 20261102T084353Z-codebuild-does-not-resolve-or-validate-arn-addressed.md
Issue-Commit: 732d86be3641565fdf04b49c6a1d1231e9cf6723

# CodeBuild ListBuildsForProject refusals report the wrong error code and non-AWS message wording

> _Note: this issue was identified against the code base at commit `732d86be3641565fdf04b49c6a1d1231e9cf6723`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuildsForProject

## Description
TWO net-new root causes in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006), sharing one fault: when ListBuildsForProject refuses a request, the emulator does not produce CodeBuild's own refusal for that condition — it emits either a generic smithy constraint rendering or the wrong modeled error entirely. Status (400) already matches in both cases, so the __type and message ARE the whole observable difference. BOTH must be closed before this issue is resolved.

Related open issues that do NOT cover these two steps: 20261102T084353Z-codebuild-input-validation-refusals-use-generic-wording-and.md describes the same FAMILY of generic-constraint wording defects but only for BatchGetProjects, BatchGetBuilds and ListProjects, and carries no replay key for ListBuildsForProject's missing-projectName step; 20261102T084353Z-codebuild-does-not-resolve-or-validate-arn-addressed.md describes the ARN-resolution family and names this fixture's task#7, but its replay keys do not own the current divergence (the emulator's message for that step has since changed to a different wording, so the step is live and unowned). Neither issue's pointer covers the two causes below.

Cause 1 of 2: a ListBuildsForProject request with no projectName is refused with the generic constraint message instead of CodeBuild's own text.
AWS: Smithy ListBuildsForProject(projectName @required NonEmptyString @length min 1) -> {ids, nextToken}; modeled errors InvalidInputException (client, HTTP 400) and ResourceNotFoundException (client, HTTP 400). A request that omits projectName is refused with HTTP 400, __type 'InvalidInputException' and the message 'Project name or ARN is required' — that string is fixed text in full: it interpolates no member name, no value and no count.
Emulator: HTTP 400 whose message is "1 validation error detected: Value null at 'projectName' failed to satisfy constraint: Member must not be null" — the generic smithy constraint renderer, naming the member and the offending value, which AWS never emits for this operation.
Why: a client or test that matches CodeBuild's documented refusal text never recognizes the condition, and operators reading emulator logs see a message shape that no AWS response has, so behavior tuned against the emulator breaks on AWS.
Fix: the ListBuildsForProject handler's input validation — emit the service's own required-identifier message for a missing projectName instead of delegating to the generic constraint renderer.
Traces: codebuild-cbnv-build-history-route#task#5.

Cause 2 of 2: a projectName that is a project ARN from another region is reported as a missing resource instead of as an invalid ARN, and the submitted ARN is pasted inside a locally built ARN.
AWS: same operation/model as above. A projectName whose value is an ARN whose region is not the caller's region is an INPUT error, not a lookup miss: HTTP 400, __type 'InvalidInputException', message "Invalid project ARN: region does not match caller's region" — entirely fixed text, naming neither the ARN nor either region.
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and a message of the form 'The provided project arn:aws:codebuild:<caller region>:<caller account>:project/<the entire submitted value> does not exist.' — i.e. the already-ARN input is interpolated verbatim into an ARN template, yielding a nested ARN that addresses no resource, together with the wrong error code.
Why: the error code is what a client branches on — ResourceNotFoundException invites a create/retry, while InvalidInputException requires fixing the request — so a caller that templated the wrong region into a project ARN is steered into the wrong recovery path, and the emitted message leaks a malformed identifier.
Fix: the projectName resolver and error mapping in the ListBuildsForProject handler — parse an ARN-shaped identifier, compare its region to the request's region BEFORE lookup and raise InvalidInputException, and never interpolate a value that is already an ARN into an ARN template.
Traces: codebuild-cbnv-build-history-route#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (165 steps, 128 passed, 37 mismatched, 21 divergence clusters, score 77.6%). Neither of these two clusters is blocked by a stub or owned by an open issue (owned_by was empty for both).

Cause 1 — codebuild-cbnv-build-history-route#task#5, ListBuildsForProject, 400/400: $.message value_mismatch — expected 'Project name or ARN is required' (fixed text in full), got "1 validation error detected: Value null at 'projectName' failed to satisfy constraint: Member must not be null". __type and status already agree.

Cause 2 — codebuild-cbnv-build-history-route#task#7, ListBuildsForProject, 400/400: $.__type value_mismatch — expected 'InvalidInputException', got 'ResourceNotFoundException'; $.message value_mismatch — expected "Invalid project ARN: region does not match caller's region" (fixed text in full), got 'The provided project arn:aws:codebuild:<caller region>:<caller account>:project/<submitted cross-region project ARN> does not exist.' (the recording shows the submitted eu-west-3 ARN nested inside a locally built us-west-2 ARN; region, account and project name are per-run values, the nesting and the wrong error code are the defect).

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — ListBuildsForProject(projectName @required NonEmptyString @length min 1) -> ListBuildsForProjectOutput{ids BuildIds @length 1-100, nextToken}; errors InvalidInputException (client, HTTP 400) and ResourceNotFoundException (client, HTTP 400); paginated inputToken/outputToken nextToken, items ids. The model does not state the refusal texts, so both messages are taken from the recorded responses.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus) and read the two codebuild ListBuildsForProject divergences in fixture codebuild-cbnv-build-history-route (task#5 and task#7), or reproduce each cause directly against the emulator's data listener:
3. Cause 1: POST CodeBuild_20161006.ListBuildsForProject with body {} — expect HTTP 400, __type 'InvalidInputException', message 'Project name or ARN is required'; observe the generic "1 validation error detected: Value null at 'projectName' failed to satisfy constraint: Member must not be null".
4. Cause 2: CreateProject(name=<p>), then POST CodeBuild_20161006.ListBuildsForProject with {"projectName": "arn:aws:codebuild:<a region other than the caller's>:<caller account>:project/<p>"} — expect HTTP 400, __type 'InvalidInputException', message "Invalid project ARN: region does not match caller's region"; observe __type 'ResourceNotFoundException' and a 'The provided project <caller-region ARN containing the submitted ARN> does not exist.' message.
