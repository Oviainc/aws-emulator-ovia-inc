Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.get-application-not-found codedeploy.create-get-delete-application codedeploy.get-deployment-group-not-found codedeploy.create-deployment-nonexistent-deployment-group-rejected codedeploy.get-deployment-not-found codedeploy.create-application-duplicate codedeploy.deployment-group-duplicate-name-rejected codedeploy.create-deployment-missing-application-name-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.get-application-not-found|run#10|AssertionError|assert message == f"No application found for name: {ghost}", message
Issue-Key: codedeploy.create-get-delete-application|_assert_not_found#10|AssertionError|assert err.get("Message") == expected_message, f'expected the message {expected_message!r}, got {err.get("Message")!r}'
Issue-Key: codedeploy.get-deployment-group-not-found|run#10|AssertionError|assert message == f"No Deployment Group found for name: {ghost}", message
Issue-Key: codedeploy.create-deployment-nonexistent-deployment-group-rejected|run#11|AssertionError|assert message == expected_message, f"CreateDeployment message {message!r}, expected {expected_message!r}"
Issue-Key: codedeploy.get-deployment-not-found|run#17|AssertionError|assert message == f"The deployment {fake_id} could not be found", message
Issue-Key: codedeploy.create-application-duplicate|run#7|AssertionError|assert message == f"{name} already exists.", message
Issue-Key: codedeploy.deployment-group-duplicate-name-rejected|run#12|AssertionError|assert message == (
Issue-Key: codedeploy.create-deployment-missing-application-name-rejected|run#5|AssertionError|assert message == "Application name is missing", f"got {message!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeDeploy client-error messages do not follow AWS's sentence templates for not-found, already-exists and required-member errors (6 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
Six CodeDeploy error paths return the correct modeled error code and HTTP 400 but build the `Message` with the emulator's own sentence instead of the template AWS uses. Each template is a known rule: it is a fixed sentence interpolating the request value(s), measured against real AWS by the checks, so the TEMPLATE (not one recording's name) is the requirement. Six distinct templates, six root causes.

Cause 1 of 6: ApplicationDoesNotExistException message.
AWS: `No application found for name: <applicationName>` — the requested name after a colon-space. Smithy: GetApplication/GetDeploymentGroup model ApplicationDoesNotExistException (HTTP 400).
Emulator: `No application found for name <applicationName>` — the colon separator is missing.
Why: clients and test harnesses that match AWS's sentence (the only place the requested name is delimited) mis-parse it.
Fix: the message construction for ApplicationDoesNotExistException in the CodeDeploy error/state layer, shared by GetApplication and friends.
Traces: codedeploy.get-application-not-found, codedeploy.create-get-delete-application.

Cause 2 of 6: DeploymentGroupDoesNotExistException message.
AWS: `No Deployment Group found for name: <deploymentGroupName>` — capitalised "Deployment Group", colon-space, and the application name is NOT part of the sentence.
Emulator: `No deployment group found for name <deploymentGroupName> in application <applicationName>`.
Why: same mis-parse, and the emulator leaks the application name into a sentence AWS scopes to the group only.
Fix: the message construction for DeploymentGroupDoesNotExistException (raised by GetDeploymentGroup and by CreateDeployment).
Traces: codedeploy.get-deployment-group-not-found, codedeploy.create-deployment-nonexistent-deployment-group-rejected.

Cause 3 of 6: DeploymentDoesNotExistException message.
AWS: `The deployment <deploymentId> could not be found`.
Emulator: `No deployment found for ID <deploymentId>`.
Why: as above; the sentence shape is the only machine-readable part of the error beyond the code.
Fix: the message construction for DeploymentDoesNotExistException in GetDeployment's handler/error layer.
Traces: codedeploy.get-deployment-not-found.

Cause 4 of 6: ApplicationAlreadyExistsException message.
AWS: `<applicationName> already exists.` (name first, trailing period).
Emulator: `An application named <applicationName> already exists with this account`.
Why: a client that strips AWS's template to recover the conflicting name gets the whole sentence instead.
Fix: the message construction for ApplicationAlreadyExistsException in CreateApplication.
Traces: codedeploy.create-application-duplicate.

Cause 5 of 6: DeploymentGroupAlreadyExistsException message.
AWS: `An Deployment Group with the name <deploymentGroupName> already exists for this application.` (AWS's wording, including its "An", and a trailing period).
Emulator: `A deployment group named <deploymentGroupName> already exists for application <applicationName>`.
Why: same as above.
Fix: the message construction for DeploymentGroupAlreadyExistsException in CreateDeploymentGroup.
Traces: codedeploy.deployment-group-duplicate-name-rejected.

Cause 6 of 6: ApplicationNameRequiredException carries a generic constraint message.
AWS: `Application name is missing` — a fixed sentence for this modeled error. Smithy: CreateDeployment models ApplicationNameRequiredException (HTTP 400); `applicationName` is ApplicationName with @length min 1.
Emulator: returns the right code but the generic modeled-constraint sentence `Value '' at 'applicationName' failed to satisfy constraint: Member must have length greater than or equal to 1`, i.e. the length-constraint channel's text is leaking into a service-specific modeled error.
Why: the response says "malformed member" where AWS says "required input missing", and the two are produced by different layers; a client surfacing the message to an operator shows a shape error for a missing field.
Fix: CreateDeployment's validation — raise the modeled ApplicationNameRequiredException with its own sentence rather than deferring to the constraint validator's text.
Traces: codedeploy.create-deployment-missing-application-name-rejected.

## Evidence
Replayed behavioral suite, codedeploy service. In every case the HTTP status (400) and the error Code match AWS; only the Message sentence diverges. Verbatim failing assertions:

- codedeploy.get-application-not-found (line 38): `assert message == f"No application found for name: {ghost}"` -> "No application found for name sim2real-behav-ghost-app-..."
- codedeploy.create-get-delete-application (line 58): "expected the message 'No application found for name: sim2real-behav-app-...', got 'No application found for name sim2real-behav-app-...'"
- codedeploy.get-deployment-group-not-found (line 124): `assert message == f"No Deployment Group found for name: {ghost}"` -> "No deployment group found for name sim2real-behav-dg-ghost-... in application sim2real-behav-app-..."
- codedeploy.create-deployment-nonexistent-deployment-group-rejected: "CreateDeployment message 'No deployment group found for name sim2real-cd-nogroup-... in application sim2real-cd-app-...', expected 'No Deployment Group found for name: sim2real-cd-nogroup-...'"
- codedeploy.get-deployment-not-found (line 68): `assert message == f"The deployment {fake_id} could not be found"` -> "No deployment found for ID d-B15C2FD24"
- codedeploy.create-application-duplicate (line 35): `assert message == f"{name} already exists."` -> "An application named sim2real-behav-app-... already exists with this account"
- codedeploy.deployment-group-duplicate-name-rejected (line 57): expected "An Deployment Group with the name <dg> already exists for this application." -> "A deployment group named sim2real-cdgroup-... already exists for application sim2real-cdapp-..."
- codedeploy.create-deployment-missing-application-name-rejected (line 36): code assertion ApplicationNameRequiredException PASSED; `assert message == "Application name is missing"` -> "Value '' at 'applicationName' failed to satisfy constraint: Member must have length greater than or equal to 1"

The resource names above are environment-specific; the requirement is the sentence template with the request value interpolated.

## Steps to Reproduce
1. Boot the emulator and point a botocore codedeploy client at it.
2. GetApplication(applicationName=<never-created>) -> Message must be "No application found for name: <name>".
3. CreateApplication, then GetDeploymentGroup(applicationName=app, deploymentGroupName=<never-created>) -> Message must be "No Deployment Group found for name: <dg>"; CreateDeployment against the same missing group must use the same sentence.
4. GetDeployment(deploymentId="d-B15C2FD24") -> Message must be "The deployment d-B15C2FD24 could not be found".
5. CreateApplication twice with the same name -> Message must be "<name> already exists.".
6. CreateDeploymentGroup twice with the same name -> Message must be "An Deployment Group with the name <dg> already exists for this application.".
7. With a parameter_validation=False client, CreateDeployment(applicationName="", deploymentGroupName="whatever-group") -> Code ApplicationNameRequiredException with Message "Application name is missing".
