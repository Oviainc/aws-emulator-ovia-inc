Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.activate-nonexistent-public-type-fails cloudformation.type-registry-not-found-errors cloudformation.deregister-in-use-type-version-rejected cloudformation.type-registration-full-lifecycle cloudformation.type-registry-full-lifecycle cloudformation.type-registration-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.activate-nonexistent-public-type-fails|run#5|AssertionError|assert msg == "TypeName is invalid", msg
Issue-Key: cloudformation|ActivateType|CFNRegistryException
Issue-Key: cloudformation.deregister-in-use-type-version-rejected|run#16|AssertionError|assert got == expected, f"deregistering the default version: expected {expected!r}, got {got!r}"
Issue-Key: cloudformation.type-registration-full-lifecycle|_error_code_and_message#29|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: cloudformation.type-registry-full-lifecycle|run#39|AssertionError|assert candidate is not None, (
Issue-Key: cloudformation.type-registration-lifecycle|_wait_type_not_found#30|Timeout|time.sleep(interval_s)
Issue-Commit: a6bd8162e254cd53a71309df843cd006b7af8737

# cloudformation: 5 net-new root causes in the extension type registry — ActivateType's public-ARN refusal is mis-worded, DeregisterType's default-version refusal is mis-worded, whole-type DeregisterType is not guarded while two versions are active, ListTypes exposes no public third-party extensions, and the registration lifecycle never settles

> _Note: this issue was identified against the code base at commit `a6bd8162e254cd53a71309df843cd006b7af8737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateType

## Description
Five net-new behavioral root causes of service `cloudformation`, all in the extension (type) registry, each grounded in failing behavioral checks of this replay. A fixer must close all five before submitting.

Cause 1 of 5: ActivateType's refusal for a public type ARN that was never published carries the wrong message.
AWS: Smithy ActivateType -> errors CFNRegistryException (client, httpError 400) and TypeNotFoundException (client, httpError 404); input member PublicTypeArn targets ThirdPartyTypeArn with @length max 1024 and @pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$. The wording is recorded behavior only: for a syntactically shaped but never-published public type ARN AWS answers HTTP 400 CFNRegistryException with the message `TypeName is invalid` — that is the whole message, fixed text with no variable parts taken from the request.
Emulator: the same error code, CFNRegistryException, but the message `You cannot specify public version number in the arn`. The emulator is diagnosing the ARN's version segment instead of reporting that the named type does not resolve to a published public extension.
Why: the message is the only thing distinguishing "this public extension does not exist" from "your ARN is shaped wrongly". A client (or an operator reading a `cdk`/CLI failure) that activates a third-party extension by ARN is told to remove a version it legitimately supplied, and a client matching on the message to branch between a retry with a different ARN and a hard failure takes the wrong branch.
Fix: the refusal construction in the cloudformation ActivateType handler — the public-type resolution miss must produce CFNRegistryException `TypeName is invalid`, and the ARN-shape diagnosis must not pre-empt it.
Traces: cloudformation.activate-nonexistent-public-type-fails, cloudformation.type-registry-not-found-errors.

Cause 2 of 5: the refusal for deregistering a type's DEFAULT version carries the wrong message.
AWS: Smithy DeregisterType -> errors CFNRegistryException (client, httpError 400) and TypeNotFoundException (client, httpError 404); input members Arn / Type / TypeName / VersionId. Message wording is recorded behavior: HTTP 400 CFNRegistryException whose fixed text is `Version '` + `' is the default version and cannot be deregistered. Deregister the resource type '` + `' instead.` with exactly two variable parts — the VersionId of the version named in the request and the TypeName of its type.
Emulator: the same code with a different sentence keyed on the ARN — `Cannot deregister <type-version ARN>: you can't deregister the default version of an extension if there are other active versions of that extension`.
Why: AWS's message is the only place the remedy is stated (deregister the TYPE, not the version); a client is left without the version id and without the instruction, and message-matching automation never recognises the condition.
Fix: the default-version guard's error construction in the cloudformation DeregisterType handler. Note: the open issue 20261105T132157Z-cloudformation-5-net-new-root-causes-in-the-cf7de2.md describes this same wording defect under its Cause 2 for a neighbouring check (cloudformation.deregister-inuse-type-rejected); it does NOT carry this check's replay key, so this cause is listed here and must be re-verified against this check too.
Traces: cloudformation.deregister-in-use-type-version-rejected.

Cause 3 of 5: DeregisterType of a WHOLE type succeeds while more than one version of it is still active, instead of being refused.
AWS: Smithy DeregisterType -> CFNRegistryException (client, httpError 400). Recorded behavior: with two active versions registered, DeregisterType(Arn=<the type ARN, no VersionId>) is refused with HTTP 400 CFNRegistryException; the whole type can only be deregistered once every non-default version has been deregistered first.
Emulator: the call returns success (HTTP 200, empty output), so the premature whole-type deregistration is accepted.
Why: the guard is what keeps a multi-version extension from being destroyed in one call; a client that relies on the refusal to detect "other versions still exist" instead silently loses the whole type, and the subsequent version-level bookkeeping it performs operates on a registry AWS would still hold.
Fix: the active-version-count precondition in the cloudformation DeregisterType handler (whole-type path) and the CFNRegistryException it must raise.
Traces: cloudformation.type-registration-full-lifecycle.

Cause 4 of 5: ListTypes surfaces no public third-party RESOURCE extensions, so nothing in the registry is activatable.
AWS: Smithy ListTypes -> input Visibility (enum), Type (RegistryType enum), Filters (TypeFilters), output TypeSummaries; error CFNRegistryException (400). Recorded behavior: in any account/region ListTypes(Visibility='PUBLIC', Type='RESOURCE') with a third-party category filter returns a non-empty TypeSummaries set — the public extension registry is populated independently of the caller's account — and each entry is then activatable through ActivateType and deactivatable through DeactivateType.
Emulator: TypeSummaries contains no public third-party RESOURCE extension at all, so the discovery step finds no candidate and the whole activate/deactivate lifecycle is unreachable.
Why: a client that discovers an extension before activating it can never proceed; every public-registry workflow (activate a vendor resource type, pin a major version, deactivate it) is dead, and the failure looks like "no such extension" rather than "registry not populated".
Fix: the public-extension catalogue backing the cloudformation ListTypes handler (the PUBLIC/third-party visibility branch) and the ActivateType/DeactivateType paths that read it. The open issue 20261028T094216Z-cloudformation-the-public-extension-registry-exposes-no-activatable.md describes a neighbouring gap in the same surface but does not carry this check's replay key.
Traces: cloudformation.type-registry-full-lifecycle.

Cause 5 of 5: the asynchronous RegisterType registration lifecycle never reaches a terminal state, so the registration/activation/default-version/deregistration sequence never completes.
AWS: recorded behavior only — RegisterType returns a RegistrationToken and the registration progresses asynchronously to a terminal ProgressStatus (COMPLETE, or FAILED when the registry's package fetch is denied), after which DescribeType / ListTypeVersions / SetTypeDefaultVersion / DeregisterType all answer consistently for that type.
Emulator: the check polling that lifecycle makes no progress and exhausts its entire budget (replay reports `Check timed out after 60.0s`), i.e. the registration never settles to a state the rest of the registry surface agrees with.
Why: a client that registers an extension and polls for readiness waits forever; nothing downstream of registration (versioning, default-version selection, deactivation, deregistration) can be exercised.
Fix: the registration worker behind the cloudformation RegisterType handler and the state it must publish to DescribeTypeRegistration / DescribeType, including the FAILED terminal state when the schema-handler package cannot be fetched.
Traces: cloudformation.type-registration-lifecycle.

## Evidence
Cause 1 — cloudformation.activate-nonexistent-public-type-fails [mismatch]:
  AssertionError: You cannot specify public version number in the arn
  (assertion: `assert msg == "TypeName is invalid"`; the code CFNRegistryException matched, only the message diverged)
  Request: ActivateType(PublicTypeArn='arn:aws:cloudformation:<region>::type/resource/<Type-With-Hyphens>/00000001')
  Expected: HTTP 400 CFNRegistryException, message exactly 'TypeName is invalid' (no variable parts).
  Observed: HTTP 400 CFNRegistryException, message 'You cannot specify public version number in the arn'.
Same divergence, second check — cloudformation.type-registry-not-found-errors [mismatch]:
  AssertionError: expected ('CFNRegistryException', 'TypeName is invalid'), got ('CFNRegistryException', 'You cannot specify public version number in the arn')

Cause 2 — cloudformation.deregister-in-use-type-version-rejected [mismatch]:
  AssertionError: deregistering the default version: expected ('CFNRegistryException', "Version '00000001' is the default version and cannot be deregistered. Deregister the resource type '<Type>::MODULE' instead."), got ('CFNRegistryException', "Cannot deregister <type-version ARN>: you can't deregister the default version of an extension if there are other active versions of that extension")
  Fixed text of the required message: "Version '…' is the default version and cannot be deregistered. Deregister the resource type '…' instead." Variable parts: the VersionId named in the request and the TypeName of its type. The ARN and account/region in the observed message are per-run values, not part of any template.

Cause 3 — cloudformation.type-registration-full-lifecycle [mismatch]:
  AssertionError: expected the call to be rejected, but it succeeded
  (at `premature = _error_code_and_message(lambda: cfn.deregister_type(Arn=type_arn))`, i.e. whole-type DeregisterType while versions 1 and 2 are both active)
  Expected: HTTP 400 CFNRegistryException. Observed: success.

Cause 4 — cloudformation.type-registry-full-lifecycle [mismatch]:
  AssertionError: no public third-party RESOURCE extensions discoverable via ListTypes
  Expected: ListTypes over the public/third-party RESOURCE registry returns at least one activatable candidate. Observed: none.

Cause 5 — cloudformation.type-registration-lifecycle [mismatch]:
  Error: Check timed out after 60.0s — the RegisterType registration never reaches a terminal state the registry surface agrees with, so the lifecycle assertions are never reached.

Smithy consulted: ActivateType (input PublicTypeArn @length 0..1024 @pattern ^arn:aws…:cloudformation:…::type/.+/[0-9a-zA-Z]{12,40}/.+$, PublisherId, TypeName @pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$; errors CFNRegistryException 400, TypeNotFoundException 404), DeregisterType (input Arn/Type/TypeName/VersionId; errors CFNRegistryException 400, TypeNotFoundException 404), ListTypes (input Visibility/Type/Filters/MaxResults/NextToken, output TypeSummaries; error CFNRegistryException 400). The model constrains shapes and error classes; the message texts above are recorded behavior.

## Steps to Reproduce
1. Cause 1: call ActivateType(PublicTypeArn='arn:aws:cloudformation:<region>::type/resource/<SomeVendor-Thing-Thing>/00000001') for a public extension that was never published — expect HTTP 400 CFNRegistryException with message exactly 'TypeName is invalid'; observe CFNRegistryException 'You cannot specify public version number in the arn'. Re-check the same refusal through the registry's not-found sweep (cloudformation.type-registry-not-found-errors), whose positive control DescribeType(Type='RESOURCE', TypeName='AWS::S3::Bucket') must keep resolving.
2. Cause 2: register two versions of a private MODULE so version 00000001 remains the default, then DeregisterType(Arn=<the default version's ARN>) and read Error.Message — expect "Version '00000001' is the default version and cannot be deregistered. Deregister the resource type '<TypeName>' instead."
3. Cause 3: with two active versions of a private RESOURCE type registered, call DeregisterType(Arn=<the TYPE ARN, no VersionId>) — expect HTTP 400 CFNRegistryException; observe success. Then deregister the non-default version and repeat — that call must succeed.
4. Cause 4: call ListTypes(Visibility='PUBLIC', Type='RESOURCE') (third-party category) — expect a non-empty TypeSummaries list, then ActivateType one of its entries and DeactivateType it, confirming it becomes DEPRECATED and unlisted.
5. Cause 5: upload a MODULE schema-handler package to S3, RegisterType against it, then poll DescribeTypeRegistration/DescribeType for a terminal state — expect the registration to settle (COMPLETE, or FAILED when the registry's fetch is denied) and the whole register→list versions→activate→set default→deactivate→deregister sequence to proceed.
6. Re-replay: cloudformation.activate-nonexistent-public-type-fails, cloudformation.type-registry-not-found-errors, cloudformation.deregister-in-use-type-version-rejected, cloudformation.type-registration-full-lifecycle, cloudformation.type-registry-full-lifecycle, cloudformation.type-registration-lifecycle.
