Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.update-package-configuration-auto-versioning-applied
Issue-Key: iot.update-package-configuration-auto-versioning-applied|run#30|AssertionError|assert dup["Message"] == expected, f"expected message {expected!r}, got {dup['Message']!r}"
Issue-Commit: c7b2bd15b2412f35bab302a04f7640cf9c159be7

# CreatePackageVersion duplicate-version ConflictException message is emulator-authored ("Version X of package Y already exists") instead of AWS's "Conditional check failure: Received duplicate request for package <packageName> version <versionName>"

> _Note: this issue was identified against the code base at commit `c7b2bd15b2412f35bab302a04f7640cf9c159be7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackageVersion

## Description
Cause 1 of 1: the ConflictException raised when CreatePackageVersion is called again for a (packageName, versionName) pair that already exists carries a message the emulator invents, so clients that read or match on the refusal text see wording AWS never emits. Failing check: iot.update-package-configuration-auto-versioning-applied.

AWS: Smithy (services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json) models CreatePackageVersion with packageName/versionName as required @httpLabel members on PUT /packages/{packageName}/versions/{versionName} and declares ConflictException (HTTP 409, "The request conflicts with the current state of the resource"). The model does not state the message text, so this is recorded behavior: against real IoT, re-creating an existing published package version is refused 409 ConflictException with the message 'Conditional check failure: Received duplicate request for package <packageName> version <versionName>'. Fixed text of that template: 'Conditional check failure: Received duplicate request for package ' + ' version '; the variable parts are the packageName and versionName taken verbatim from the request (the recording's 'sim2real-iot-pkg-f5489b666f72' and '1.0.0' are just one run's values filling that template, not part of it).

Emulator: the status (409) and the error code (ConflictException) are right, but the message is 'Version <versionName> of package <packageName> already exists' — a different sentence, with the two values in the opposite order and none of AWS's fixed text. For the replayed check the emulator answered 'Version 1.0.0 of package sim2real-iot-pkg-f5489b666f72 already exists' where AWS answers 'Conditional check failure: Received duplicate request for package sim2real-iot-pkg-f5489b666f72 version 1.0.0'.

Why: a client (or a test, log scraper, or operator runbook) that distinguishes a duplicate-version conflict from the other ConflictException causes of this operation by its message never matches AWS's sentence, and a user-facing surface that echoes the service message shows text AWS does not produce. The error message is part of the operation's contract, so the divergence is a defect even though the code and status match.

Fix: the duplicate-version refusal built in the CreatePackageVersion handler — the ConflictException construction on the already-present-version branch of handle() in the operation's handler file. The message must be rendered from AWS's fixed template with packageName and versionName interpolated from the request in AWS's order.

Traces: iot.update-package-configuration-auto-versioning-applied asserts dup['Code'] == 'ConflictException' (passes) and then dup['Message'] == the 'Conditional check failure: Received duplicate request for package ... version ...' sentence (fails). Handler source: services/service-com-amazonaws-iot-awsiotservice/src/ops/create_package_version.rs lines 205-228 (the ConflictException at lines 218-227 formats "Version {} of package {} already exists").

## Evidence
Failing check iot.update-package-configuration-auto-versioning-applied:

  File "check_iot_update_package_configuration_auto_versioning_applied.py", line 124, in run
    assert dup["Message"] == expected, f"expected message {expected!r}, got {dup['Message']!r}"
  AssertionError: expected message 'Conditional check failure: Received duplicate request for package sim2real-iot-pkg-f5489b666f72 version 1.0.0', got 'Version 1.0.0 of package sim2real-iot-pkg-f5489b666f72 already exists'

Expected (AWS, 409 ConflictException):
  Message = 'Conditional check failure: Received duplicate request for package <packageName> version <versionName>'
Actual (emulator, 409 ConflictException):
  Message = 'Version <versionName> of package <packageName> already exists'

Emulator source producing it — services/service-com-amazonaws-iot-awsiotservice/src/ops/create_package_version.rs:218-227:
  return Err(CreatePackageVersionErrorModeled::ConflictException(ConflictException {
      message: Some(format!("Version {} of package {} already exists", existing.version_name, existing.package_name)),
      resource_id: Some(existing.version_name.clone()),
  }).into());

## Steps to Reproduce
1. Boot the emulator and point an iot client at it.
2. iot.create_package(packageName=P)
3. iot.create_package_version(packageName=P, versionName="1.0.0")
4. iot.update_package_version(packageName=P, versionName="1.0.0", action="PUBLISH")
5. Call iot.create_package_version(packageName=P, versionName="1.0.0") again with NO clientToken.
6. Observe the ClientError: Code is ConflictException with HTTP 409 (correct), but Error.Message is 'Version 1.0.0 of package P already exists' instead of AWS's 'Conditional check failure: Received duplicate request for package P version 1.0.0'.

Or re-replay the behavioral check: iot.update-package-configuration-auto-versioning-applied.
