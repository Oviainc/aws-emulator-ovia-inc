Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessapplicationrepository-application-version-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessapplicationrepository-application-version-lifecycle
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|CreateApplicationVersion|4xx->4xx|$.message
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# serverlessrepo CreateApplicationVersion rejects a non-semantic version with its own message wording

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationVersion

## Description
This bundle carries 1 CAUSE and it must be closed before submitting.

Cause 1 of 1: CreateApplicationVersion refuses a SemanticVersion path label that is not a semantic version with the correct HTTP status and error code but with an emulator-authored message instead of the one SAR words for that validation failure (fixture serverlessapplicationrepository-application-version-lifecycle, step task#4).

AWS: Smithy: CreateApplicationVersion PUT /applications/{ApplicationId}/versions/{SemanticVersion}, success code 201; input member SemanticVersion is an @httpLabel @required __string documented as "The semantic version of the new version" (output member SemanticVersion: "The semantic version of the application: https://semver.org/"). The model carries no @pattern for it, so the refusal itself is recorded behavior only: the operation models BadRequestException (httpError 400, ErrorCode member doc "400", Message member doc "One of the parameters in the request is invalid"), and the recording shows SAR answering 400 with the invalid-semantic-version message "Version name is a not valid semantic version. See https://semver.org/ for syntax". That message is entirely FIXED text for this error class — it names neither the rejected value, nor the member, nor the application — so there are no variable parts to substitute from the request or from state; the whole sentence, including the "is a not valid" wording and the trailing "See https://semver.org/ for syntax", is the requirement.

Emulator: answers the right status (400) and error code, but with its own sentence "SemanticVersion must be a semantic version (https://semver.org/): <rejected value>" — it names the input member, drops the "See ... for syntax" clause, and appends the rejected version string, which AWS's message never includes. (In the replayed step the appended value was the submitted label; that value is request data, not part of the template, and the divergence is the wording itself.)

Why: the message is the only part of a BadRequestException that distinguishes one invalid parameter from another — ErrorCode is the constant "400" for every BadRequestException this service raises. A client, SAM CLI wrapper, or test that matches on the documented sentence (or merely surfaces it to a user) does not recognise the emulator's refusal as the semantic-version failure, so error-classification and message-assertion logic that works against SAR fails against the emulator even though the version was correctly rejected.

Fix: the input validation of the CreateApplicationVersion handler — services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/src/ops/create_application_version.rs, the `is_semantic_version` guard that builds the `bad_request(...)` message — must emit the fixed sentence AWS words for this class rather than a member-named, value-appended one.

Traces: serverlessapplicationrepository-application-version-lifecycle#task#4.

Related but NOT covering: the open issue 20261022T011815Z-serverlessrepo-version-publish-and-template-generation-fixtures-404.md bundles CreateApplicationVersion 4xx message/errorCode keys, but its causes are the 404s that follow a rejected CreateApplication (and its cause 7 is the missing SAM Transform validation on fixture serverlessrepo-application-version-publish#task#8); it carries no replay binding for this step and the replay reports this cluster unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting): 1 unowned mismatch cluster for this cause.

Cause 1 - serverlessapplicationrepository-application-version-lifecycle#task#4, CreateApplicationVersion: expected status 400, got 400 (status and error class agree). Diverging key `$.message`: value_mismatch - expected 'Version name is a not valid semantic version. See https://semver.org/ for syntax', got 'SemanticVersion must be a semantic version (https://semver.org/): 1.0'. affected_count 1 (no other members of the cluster). Fixed text expected: the entire sentence 'Version name is a not valid semantic version. See https://semver.org/ for syntax'. Variable parts: none - the recorded message substitutes nothing from the request or from state. Values taken from the request in the emulator's message: the rejected semantic-version label trailing the colon (here the label submitted by the step), plus the input member name 'SemanticVersion'; neither appears in AWS's message.

Code: services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/src/ops/create_application_version.rs lines 192-197 - `if !crate::state::is_semantic_version(&input.semantic_version) { return Err(bad_request(format!("SemanticVersion must be a semantic version (https://semver.org/): {}", input.semantic_version))); }`; the 400 is built by `bad_request` at line 298 via `crate::state::bad_request`.

## Steps to Reproduce
1. Replay fixture serverlessapplicationrepository-application-version-lifecycle against the emulator.
2. Step task#4 issues PUT /applications/{applicationId}/versions/{semanticVersion} where the SemanticVersion path label is not a semantic version (the fixture uses a two-component version such as '1.0').
3. Observe HTTP 400 with the expected error class, but body `$.message` == 'SemanticVersion must be a semantic version (https://semver.org/): <submitted label>' instead of the recorded 'Version name is a not valid semantic version. See https://semver.org/ for syntax'.
4. Any valid semantic version at the same step is accepted (201), so only the refusal's wording diverges.
