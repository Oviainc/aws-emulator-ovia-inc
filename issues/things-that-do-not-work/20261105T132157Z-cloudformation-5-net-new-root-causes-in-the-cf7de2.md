Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deregister-aws-owned-type-rejected cloudformation.deregister-inuse-type-rejected cloudformation.register-type-invalid-schema-rejected cloudformation.type-configuration-roundtrip cloudformation.type-publishing-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.deregister-aws-owned-type-rejected|_expect_error#5|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.deregister-inuse-type-rejected|run#13|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: cloudformation.register-type-invalid-schema-rejected|run#12|AssertionError|assert bad_detail["ProgressStatus"] == "FAILED", (
Issue-Key: cloudformation.type-configuration-roundtrip|_error_code_and_message#19|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with: {resp}")
Issue-Key: cloudformation.type-publishing-lifecycle|run#21|AssertionError|assert not described_cfg["TypeConfigurations"], described_cfg
Issue-Commit: 269019b5b42949e66be9fce8117dfdac4ac9abf5

# cloudformation: 5 net-new root causes in the extension registry — DeregisterType leaves the type describable and mis-words its default-version refusal, RegisterType of a schema-only package reports COMPLETE, SetTypeConfiguration accepts MODULE types and BatchDescribeTypeConfigurations invents an empty configuration

> _Note: this issue was identified against the code base at commit `269019b5b42949e66be9fce8117dfdac4ac9abf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterType

## Description
Five net-new behavioral root causes of service `cloudformation`, all in the extension (type) registry, grounded in five failing behavioral checks. A fixer must close all five before submitting.

Cause 1 of 5: DeregisterType of a whole private type leaves the type readable through DescribeType instead of making it not-found.
AWS: Smithy DeregisterType -> errors CFNRegistryException (client, httpError 400) and TypeNotFoundException (client, httpError 404); after the type (not a single version) is deregistered it can no longer be used or read, and DescribeType for it is refused with TypeNotFoundException whose message is the fixed text `The type '` + the TypeName from the request + `' cannot be found.`
Emulator: DescribeType(Type="MODULE", TypeName=<deregistered type>) answers HTTP 200 with the full type detail (Arn, Schema, DefaultVersionId, Visibility PRIVATE) and merely flips `DeprecatedStatus` to `DEPRECATED`; no refusal at all.
Why: deregistration is the registry's delete. A client that deregisters a type and then confirms it is gone is told it still exists, and a type that CloudFormation must refuse to provision still reads as available.
Fix: the registry state transition in the cloudformation DeregisterType handler (whole-type deregistration must remove the type from the readable set) and the not-found path of DescribeType.
Traces: cloudformation.deregister-aws-owned-type-rejected.

Cause 2 of 5: the refusal for deregistering a type's default version carries the wrong message.
AWS: Smithy DeregisterType -> CFNRegistryException (client, httpError 400). The message is the fixed text `Version '` + the VersionId from the request + `' is the default version and cannot be deregistered. Deregister the resource type '` + the TypeName + `' instead.`; the version id and type name are the only variable parts.
Emulator: the same error code with a different sentence — `Cannot deregister <type-version ARN>: you can't deregister the default version of an extension if there are other active versions of that extension` — keying on the ARN rather than naming the version and the remedy.
Why: the message is the only place AWS states the remedy (deregister the type, not the version); a client or operator matching on it, or reading it, gets neither the version id nor the instruction.
Fix: the default-version guard's error construction in the cloudformation DeregisterType handler.
Traces: cloudformation.deregister-inuse-type-rejected.

Cause 3 of 5: RegisterType accepts a schema-only handler package and drives the registration to COMPLETE instead of failing it.
AWS: Smithy RegisterType -> CFNRegistryException (client, httpError 400); registration is asynchronous and its outcome is read through DescribeTypeRegistration, whose ProgressStatus reaches `FAILED` for a package that carries a schema but no handlers/binary, with a description of the validation failure.
Emulator: DescribeTypeRegistration reports `ProgressStatus: COMPLETE` and no description for a schema-only package, so an unusable extension registers successfully.
Why: the registration result is the only validation feedback for an extension package; a client that polls it is told an unusable type is ready and only discovers the problem when a stack using it fails.
Fix: the schema-handler-package validation in the cloudformation RegisterType registration worker, and the FAILED ProgressStatus/description it must record.
Traces: cloudformation.register-type-invalid-schema-rejected.

Cause 4 of 5: SetTypeConfiguration is accepted for a MODULE type, which has no configuration surface.
AWS: Smithy SetTypeConfiguration -> errors CFNRegistryException (client, httpError 400) and TypeNotFoundException (client, httpError 404). A MODULE type is refused with CFNRegistryException and the fixed message `TypeConfiguration is not supported for MODULE types` (no variable parts). The discriminator in the same flow still holds: an unregistered TypeName is refused with TypeNotFoundException `The type '<TypeName>' cannot be found.`
Emulator: answers HTTP 200 and returns a ConfigurationArn (`arn:aws:cloudformation:<region>:<account>:type-configuration/module/<type>/default`) for a registered MODULE, persisting a configuration AWS never creates.
Why: the refusal is how a client learns the category does not take configuration; instead it believes a secret-bearing configuration was stored against a module and will never be applied.
Fix: the type-category check in the cloudformation SetTypeConfiguration handler (reject RegistryType MODULE before writing state).
Traces: cloudformation.type-configuration-roundtrip.

Cause 5 of 5: BatchDescribeTypeConfigurations synthesizes an empty `{}` configuration for a type that has none, instead of reporting it as a not-found error entry.
AWS: Smithy BatchDescribeTypeConfigurations -> output members TypeConfigurations, Errors and UnprocessedTypeConfigurations; errors CFNRegistryException (400) and TypeConfigurationNotFoundException (404). For an identifier whose configuration was never set, `TypeConfigurations` is empty and exactly one entry appears under `Errors` with ErrorCode `404` and ErrorMessage `TypeConfiguration not found`.
Emulator: returns `TypeConfigurations: [{Configuration: '{}', TypeArn: ..., TypeName: ..., IsDefaultConfiguration: false}]` with `Errors: []` and `UnprocessedTypeConfigurations: []`.
Why: a client cannot distinguish "configured with nothing" from "never configured", so it skips the configuration step it needed to perform, and the batch API's error channel never fires.
Fix: the lookup in the cloudformation BatchDescribeTypeConfigurations handler — a missing configuration must become an Errors entry (404 / `TypeConfiguration not found`), not a fabricated TypeConfigurations row. The open issue 20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md owns two neighbouring batch-describe checks; it does not carry this one's replay key.
Traces: cloudformation.type-publishing-lifecycle.

## Evidence
Cause 1 — cloudformation.deregister-aws-owned-type-rejected [mismatch]:
  AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'Arn': 'arn:aws:cloudformation:us-west-2:000000000000:type/module/<Type>/00000001', 'Type': 'MODULE', 'TypeName': '<Type>', 'DefaultVersionId': '00000001', 'IsDefaultVersion': True, 'Schema': '...', 'DeprecatedStatus': 'DEPRECATED', 'Visibility': 'PRIVATE', ... 'HTTPStatusCode': 200}
  Expected: TypeNotFoundException (404) `The type '<TypeName>' cannot be found.`

Cause 2 — cloudformation.deregister-inuse-type-rejected [mismatch]:
  got      ('CFNRegistryException', "Cannot deregister arn:aws:cloudformation:us-west-2:000000000000:type/module/<Type>/00000002: you can't deregister the default version of an extension if there are other active versions of that extension")
  expected ('CFNRegistryException', "Version '00000002' is the default version and cannot be deregistered. Deregister the resource type '<Type>' instead.")
  (fixed text: "Version '…' is the default version and cannot be deregistered. Deregister the resource type '…' instead."; variable: the VersionId and TypeName from the request.)

Cause 3 — cloudformation.register-type-invalid-schema-rejected [mismatch]:
  AssertionError: expected registration to FAIL for a schema-only package, got COMPLETE: None (DescribeTypeRegistration ProgressStatus/Description).

Cause 4 — cloudformation.type-configuration-roundtrip [mismatch]:
  AssertionError: expected a ClientError, call succeeded with: {'ConfigurationArn': 'arn:aws:cloudformation:us-west-2:000000000000:type-configuration/module/<Type>/default', ... 'HTTPStatusCode': 200}
  Expected: CFNRegistryException (400) 'TypeConfiguration is not supported for MODULE types'.

Cause 5 — cloudformation.type-publishing-lifecycle [mismatch]:
  AssertionError: {'Errors': [], 'UnprocessedTypeConfigurations': [], 'TypeConfigurations': [{'Configuration': '{}', 'TypeArn': 'arn:aws:cloudformation:us-west-2:000000000000:type/module/<Type>', 'TypeName': '<Type>', 'IsDefaultConfiguration': False}], ... 'HTTPStatusCode': 200}
  Expected: TypeConfigurations empty, exactly one Errors entry with ErrorCode '404' and ErrorMessage 'TypeConfiguration not found'.

Smithy consulted: DeregisterType (errors CFNRegistryException 400, TypeNotFoundException 404; input Arn/Type/TypeName/VersionId), SetTypeConfiguration (errors CFNRegistryException 400, TypeNotFoundException 404; output ConfigurationArn), RegisterType (error CFNRegistryException 400; output RegistrationToken), BatchDescribeTypeConfigurations (output TypeConfigurations/Errors/UnprocessedTypeConfigurations; errors CFNRegistryException 400, TypeConfigurationNotFoundException 404). The message texts are recorded behavior — the model constrains the shapes, not the wording.

## Steps to Reproduce
1. Cause 1: RegisterType a private MODULE, wait for the registration to complete, DeregisterType(Type='MODULE', TypeName=<name>) with no VersionId, then DescribeType(Type='MODULE', TypeName=<name>) — expect TypeNotFoundException, observe a 200 with DeprecatedStatus DEPRECATED.
2. Cause 2: register two versions of a private MODULE so 00000002 is the default, then DeregisterType(Arn=<the default version's ARN>) and read the CFNRegistryException message.
3. Cause 3: RegisterType with a SchemaHandlerPackage zip that contains only the schema JSON (no handlers), then poll DescribeTypeRegistration — expect ProgressStatus FAILED.
4. Cause 4: on a registered MODULE type, call SetTypeConfiguration(TypeArn=<type arn>, Configuration='{"Parameters":{...}}', ConfigurationAlias='default') — expect CFNRegistryException 'TypeConfiguration is not supported for MODULE types'. For the discriminator, call it with an unregistered TypeName and Type='RESOURCE' — expect TypeNotFoundException "The type '<name>' cannot be found."
5. Cause 5: on a registered type whose configuration was never set, call BatchDescribeTypeConfigurations(TypeConfigurationIdentifiers=[{'TypeArn': <arn>}]) — expect TypeConfigurations empty and one Errors entry 404 / 'TypeConfiguration not found'.
6. Re-replay: cloudformation.deregister-aws-owned-type-rejected, cloudformation.deregister-inuse-type-rejected, cloudformation.register-type-invalid-schema-rejected, cloudformation.type-configuration-roundtrip, cloudformation.type-publishing-lifecycle.
