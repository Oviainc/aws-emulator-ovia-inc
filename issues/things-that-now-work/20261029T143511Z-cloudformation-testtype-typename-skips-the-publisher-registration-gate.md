Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-not-found-across-apis-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-not-found-across-apis-rejected|run#17|AssertionError|assert test_error == expected_test, f"TestType returned {test_error!r}, expected {expected_test!r}"
Issue-Commit: 0479bbc94260c8c4bfc0ff3f287d60a9dc8953d9

# cloudformation: TestType(TypeName=...) skips the publisher-registration gate — a non-publisher caller gets 404 TypeNotFoundException instead of 400 CFNRegistryException 'You must register as a publisher with CloudFormation to call test-type.'

> _Note: this issue was identified against the code base at commit `0479bbc94260c8c4bfc0ff3f287d60a9dc8953d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestType

## Description
One net-new behavioral root cause of service `cloudformation`, grounded in one failing behavioral check (cloudformation.type-not-found-across-apis-rejected). A fixer must close it before submitting.

Cause 1 of 1: TestType has no publisher-registration pre-flight on the TypeName spelling of the call. It resolves the named extension in the registry first, so the refusal AWS owes an account that is not a registered CloudFormation extension publisher is never produced and the request falls through to the registry's not-found refusal. Failing check id: cloudformation.type-not-found-across-apis-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models `TestType` with input members Arn (target TypeArn, @length 0..1024, pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$), Type (ThirdPartyType enum), TypeName (@length 10..204, pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$), VersionId (@length 1..128, pattern ^[A-Za-z0-9-]+$), LogDeliveryBucket; output TypeVersionArn; idempotent; and exactly two modelled errors — CFNRegistryException (errorKind client, httpError 400, awsQueryError CFNRegistryException/400) and TypeNotFoundException (client, httpError 404, awsQueryError TypeNotFoundException/404). The model does not state the publisher rule itself, so for that rule: recorded behavior only — TestType requires the CALLING ACCOUNT to be a registered CloudFormation extension publisher, and an account that is not one is refused up front with HTTP 400, Error.Code `CFNRegistryException`, Error.Message exactly `You must register as a publisher with CloudFormation to call test-type.` That whole sentence is FIXED text: no segment of it is taken from the request (not TypeName, not Type, not VersionId) or from emulator state, so there is no variable part to template. The refusal is ordered BEFORE the registry lookup of the named extension: the recording passes a three-part type name (`SimParity::Fidelity::NoSuch<12 hex>` in this check — the hex suffix is a per-run value filling the template, not part of the requirement) that is registered nowhere and still gets the publisher refusal, never TypeNotFoundException.

Emulator: the same call is refused with a different class, status and message — HTTP 404, Error.Code `TypeNotFoundException`, Error.Message `The type 'SimParity::Fidelity::NoSuch92152fa2fd57' cannot be found.` (fixed text `The type '` + `' cannot be found.` with the request's TypeName as its one variable part). That is the registry's correct not-found refusal for a DescribeType-class lookup, but it is the wrong refusal class for TestType, and its presence proves the publisher pre-flight does not exist: the handler went straight to extension resolution. Everything earlier in the same check passes, which localises the defect to this gate alone: DeactivateType, DeregisterType, SetTypeConfiguration, SetTypeDefaultVersion and DescribeType all answer the expected ('TypeNotFoundException', "The type '<TypeName>' cannot be found."), the DescribeType(AWS::SNS::Topic) positive control answers TypeName=AWS::SNS::Topic/Visibility=PUBLIC, and ActivateType correctly answers ('CFNRegistryException', 'You cannot specify public version number in the arn'). TestType is implemented (it does registry work and does not answer NotImplemented), so this is a fidelity gap in the ordering/authorization layer, not a leftover scaffold stub.

Why: a client relying on AWS's behavior gets both the classification and the remediation wrong. Error-taxonomy code catches CFNRegistryException (a 400 registry/publisher-state refusal) to tell the operator to call RegisterPublisher first, and branches on 404 TypeNotFoundException by registering or re-naming the extension — so against the emulator it diagnoses a missing extension and performs the wrong remediation, while status-based retry/alarm layers see a 404 where AWS sends a 400. Because the gate is ABSENT rather than merely reworded, a caller whose type DOES resolve is let through entirely: the emulator will accept and report a TestType AWS would have refused outright, and every subsequent DescribeTypeRegistration/TestType status poll built on that acceptance diverges with no error anywhere in the sequence.

Fix: the TestType handler in the cloudformation crate's type-registry ops, together with the publisher state that RegisterPublisher writes. The handler must consult the calling account's publisher registration and refuse with the modelled CFNRegistryException carrying the fixed sentence above BEFORE any registry/extension resolution and before any state write, for BOTH input spellings (Arn and Type+TypeName). Validation ORDER is part of the contract here, not only the message text. Related open issue that does NOT cover this check: 20261029T131601Z-cloudformation-testtype-never-applies-the-publisher-registration-gate.md describes the same gate but is keyed to cloudformation.type-version-publish-and-test (the Arn spelling) and carries only that replay key; this check (the Type+TypeName spelling) is not in its Issue-Trace-Ids and was not reported as owned by it, so it is filed here and the two should be closed together. The fixed issue 20261025T133035Z-cloudformation-testtype-stub.md only implemented the operation; it is not reopened by this, since the call is correctly refused here, just with the wrong refusal class.

Traces: cloudformation.type-not-found-across-apis-rejected (the TestType assertion at check line 79).

## Evidence
Failing check cloudformation.type-not-found-across-apis-rejected [mismatch]:
  File "check_cloudformation_type_not_found_across_apis_rejected.py", line 79, in run
    assert test_error == expected_test, f"TestType returned {test_error!r}, expected {expected_test!r}"
  AssertionError: TestType returned ('TypeNotFoundException', "The type 'SimParity::Fidelity::NoSuch92152fa2fd57' cannot be found."), expected ('CFNRegistryException', 'You must register as a publisher with CloudFormation to call test-type.')

Request that diverges:
  cfn.test_type(TypeName='SimParity::Fidelity::NoSuch<12 hex>', Type='RESOURCE')
  (the 12-hex suffix is generated per run — a value that fills the template, never part of the requirement; the caller account has never called RegisterPublisher)

Message breakdown for the diverging refusal:
  expected — HTTP 400, Error.Code 'CFNRegistryException', Error.Message the entire fixed sentence 'You must register as a publisher with CloudFormation to call test-type.' with NO variable parts.
  observed — HTTP 404, Error.Code 'TypeNotFoundException', Error.Message "The type 'SimParity::Fidelity::NoSuch92152fa2fd57' cannot be found." — fixed text "The type '" + "' cannot be found." with the request's TypeName as the only variable part. Correct wording for a registry not-found, wrong refusal class and status for TestType.

Passing in the SAME check before this line, so the type-management surface is otherwise healthy and the finding is about the publisher gate alone:
  - cfn.describe_type(Type='RESOURCE', TypeName='AWS::SNS::Topic') -> 200, TypeName='AWS::SNS::Topic', Visibility='PUBLIC' (positive control).
  - DeactivateType, DeregisterType, SetTypeConfiguration, SetTypeDefaultVersion, DescribeType against the unregistered TypeName all -> ('TypeNotFoundException', "The type '<TypeName>' cannot be found.") as expected.
  - cfn.activate_type(Type='RESOURCE', PublicTypeArn='arn:aws:cloudformation:<region>::type/resource/<publisherId>/SimParity-Fidelity-NoSuch<hex>/00000001') -> ('CFNRegistryException', 'You cannot specify public version number in the arn') as expected.
  - The run never reaches the PublishType assertion that follows, so PublishType's own publisher gate is untested by this failure.

Smithy consulted (query=operation on TestType): input {Arn: TypeArn @length 0..1024 pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$; Type: ThirdPartyType enum; TypeName @length 10..204 pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$; VersionId @length 1..128 pattern ^[A-Za-z0-9-]+$; LogDeliveryBucket @length 3..63}, output {TypeVersionArn}, errors = CFNRegistryException (client/400) and TypeNotFoundException (client/404), idempotent=true. The TypeName the check sends satisfies the modelled pattern and length, so the refusal belongs to the registry/publisher layer and not to constraint validation.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation.type-not-found-across-apis-rejected.
2. Directly, from an account that has NOT called RegisterPublisher: cfn.test_type(Type='RESOURCE', TypeName='SimParity::Fidelity::NoSuchaabbccddeeff'). Expected: HTTP 400, Error.Code 'CFNRegistryException', Error.Message exactly 'You must register as a publisher with CloudFormation to call test-type.'. Observed: HTTP 404, 'TypeNotFoundException', "The type 'SimParity::Fidelity::NoSuchaabbccddeeff' cannot be found.".
3. Same expectation via the Arn spelling: cfn.test_type(Arn='arn:aws:cloudformation:<region>:<caller-account>:type/resource/SimParity-Fidelity-NoSuchaabbccddeeff/00000001') — confirms the gate is a property of the operation, not of the member set.
4. Ordering check that a message-only fix would leave broken: register a private extension type so it DOES resolve in the account, then call TestType against it from an account that is not a registered publisher. Expected: still 400 CFNRegistryException with the same fixed sentence. Observed: the emulator proceeds past the now-successful registry lookup instead of refusing.
5. Positive control that must keep working after the fix: cfn.describe_type(Type='RESOURCE', TypeName='AWS::SNS::Topic') -> 200 with TypeName='AWS::SNS::Topic', Visibility='PUBLIC'.
6. Negative controls that must keep working after the fix: DeactivateType / DeregisterType / SetTypeConfiguration / SetTypeDefaultVersion / DescribeType on the same unregistered TypeName must still answer 404 TypeNotFoundException with "The type '<TypeName>' cannot be found.", and ActivateType with a versioned PublicTypeArn must still answer 400 CFNRegistryException 'You cannot specify public version number in the arn'.
