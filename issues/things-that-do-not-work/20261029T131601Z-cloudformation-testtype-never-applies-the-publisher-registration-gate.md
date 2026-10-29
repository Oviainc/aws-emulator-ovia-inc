Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-version-publish-and-test
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-version-publish-and-test|run#10|AssertionError|assert test_error == expected_test, (
Issue-Commit: 939fc85144c938b9bc6d1f441a638c73ec9013b4

# cloudformation: TestType never applies the publisher-registration gate — a caller that is not a registered extension publisher gets 404 TypeNotFoundException instead of 400 CFNRegistryException 'You must register as a publisher with CloudFormation to call test-type.'

> _Note: this issue was identified against the code base at commit `939fc85144c938b9bc6d1f441a638c73ec9013b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestType

## Description
One net-new behavioral root cause of service `cloudformation`, grounded in one failing behavioral check. A fixer must close it before submitting.

Cause 1 of 1 — TestType resolves the named extension in the registry FIRST and has no publisher-registration pre-flight at all, so the refusal AWS owes an account that never registered as an extension publisher is never produced, and the request instead falls through to the registry's not-found refusal.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models `TestType` with input members `Arn` (target TypeArn, @length 0..1024, pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$`), `Type` (ThirdPartyType enum), `TypeName` (@length 10..204, pattern `^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$`), `VersionId`, `LogDeliveryBucket`; output `TypeVersionArn`; and exactly two errors — `CFNRegistryException` (errorKind client, httpError 400, awsQueryError code CFNRegistryException/400) and `TypeNotFoundException` (client, httpError 404, awsQueryError TypeNotFoundException/404). The model does not state the publisher rule, so for that rule: recorded behavior only — TestType requires the CALLING ACCOUNT to be a registered CloudFormation extension publisher, and an account that is not one is refused up front with HTTP 400, Error.Code `CFNRegistryException`, Error.Message exactly `You must register as a publisher with CloudFormation to call test-type.` That message is ENTIRELY fixed text: no segment of it is taken from the request (not the Arn, not the TypeName, not the VersionId) or from any state, so there is no variable part to template. The refusal is ordered BEFORE the registry lookup of the named extension: the recording sends an ARN/type name that is registered nowhere and still gets the publisher refusal, never TypeNotFoundException. The same ordering is asserted on the TypeName form of the call (`TestType(Type='RESOURCE', TypeName=<unregistered Namespace::Service::Resource>)`) in the sibling check cloudformation.type-not-found-error-across-apis, so the gate is a property of the operation, not of the Arn-vs-TypeName spelling.

Emulator: the same call is refused with a DIFFERENT error class, status and message — HTTP 404, Error.Code `TypeNotFoundException`, Error.Message `The type 'Sim2Real::Probe::Nonexistent' cannot be found.` (fixed words `The type '` … `' cannot be found.` with the three-part extension type name resolved from the ARN as the only variable part). That is the registry's not-found refusal, which means the publisher pre-flight does not exist: the handler went straight to resolving the extension. Consequence beyond the wording: for a type that DOES resolve in the account, the emulator will let TestType proceed and start/report a test AWS would have refused outright, because nothing in the handler ever asks whether the caller is a publisher.

Why: a client relying on AWS's behavior gets both the classification and the remediation wrong. Error-taxonomy code catches `CFNRegistryException` (a 400 registry/authorization-state refusal) to tell the operator "register as a publisher first" (RegisterPublisher), and retries/branches on 404 `TypeNotFoundException` by registering or re-naming the extension — so against the emulator it diagnoses a missing extension and performs the wrong remediation, while HTTP-status-based retry layers see a 404 where AWS sends a 400. And because the gate is absent rather than merely reworded, a caller whose type does resolve is told its test was accepted for an account AWS would never have let call test-type, so every subsequent DescribeTypeRegistration/TestType-status poll built on that is divergent with no error anywhere in the sequence.

Fix: the TestType handler in the cloudformation crate's type-registry ops (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/test_type.rs) together with the publisher state RegisterPublisher writes: the handler must consult the calling account's publisher registration and refuse with the modelled `CFNRegistryException` carrying the fixed message above BEFORE any registry/extension resolution and before any state write. Validation ORDER is part of the contract here, not just the message text. Neighbouring open issues that do NOT cover this: 20261026T235900Z-cloudformation-7-net-new-root-causes-in-the.md Cause 6 covers the TypeNotFoundException MESSAGE rendering on DescribeType for the same check (that cause looks addressed — the emulator now renders the resolved type name `Sim2Real::Probe::Nonexistent` rather than the raw ARN — and the check now fails earlier, at the TestType assertion, with a different replay key); 20261029T085553Z-cloudformation-activatetype-does-not-refuse-a-publictypearn-that.md is ActivateType's version-segment rule; 20261028T094216Z-cloudformation-the-public-extension-registry-exposes-no-activatable.md is the empty PUBLIC/THIRD_PARTY catalog. None of them mentions TestType's publisher gate.

Traces: cloudformation.type-version-publish-and-test (the TestType assertion at run line 38).

## Evidence
Failing check cloudformation.type-version-publish-and-test [mismatch]:
  File "check_cloudformation_type_version_publish_and_test.py", line 38, in run
    assert test_error == expected_test, (
  AssertionError: expected ('CFNRegistryException', 'You must register as a publisher with CloudFormation to call test-type.') from TestType on a nonexistent type version, got ('TypeNotFoundException', "The type 'Sim2Real::Probe::Nonexistent' cannot be found.")

Request that diverges:
  cfn.test_type(Arn='arn:aws:cloudformation:<region>:<caller-account-id>:type/resource/Sim2Real-Probe-Nonexistent/00000001')
  (the account id comes from sts.GetCallerIdentity in the check; the type-name segment and the version segment are per-run values that fill the template and are not part of the requirement)

Already passing in the same check BEFORE this line, so the type-management surface is otherwise healthy and this is a fact about the publisher gate alone:
  cfn.describe_type(Type='RESOURCE', TypeName='AWS::SNS::Topic') returns TypeName='AWS::SNS::Topic' (positive control that the registry API is reachable), and sts.get_caller_identity() answers an Account.

Message breakdown for the diverging refusal:
  expected — HTTP 400, Error.Code 'CFNRegistryException', Error.Message the whole fixed sentence 'You must register as a publisher with CloudFormation to call test-type.' with NO variable parts.
  observed — HTTP 404, Error.Code 'TypeNotFoundException', Error.Message "The type 'Sim2Real::Probe::Nonexistent' cannot be found." — fixed text "The type '" + "' cannot be found." with the resolved three-part type name as its one variable part. Correct message for a DescribeType-class not-found, wrong refusal class for TestType.

Same rule asserted on the other spelling of the call (sibling check cloudformation.type-not-found-error-across-apis, line 99): cfn.test_type(Type='RESOURCE', TypeName='Sim2RealTest::NoSuchResource::Type') is also expected to answer ('CFNRegistryException', 'You must register as a publisher with CloudFormation to call test-type.').

Smithy consulted: TestType input {Arn: TypeArn @length 0..1024 pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$; Type: ThirdPartyType enum; TypeName @length 10..204 pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$; VersionId @length 1..128 pattern ^[A-Za-z0-9-]+$; LogDeliveryBucket @length 3..63}, output {TypeVersionArn}, errors = CFNRegistryException (client/400/CFNRegistryException) and TypeNotFoundException (client/404/TypeNotFoundException), idempotent. The ARN the check sends satisfies the modelled TypeArn pattern, so this refusal belongs to the registry/publisher layer, not to constraint validation.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation.type-version-publish-and-test.
2. Directly, from an account that has NOT called RegisterPublisher: cfn.test_type(Arn='arn:aws:cloudformation:<region>:<caller-account>:type/resource/Sim2Real-Probe-Nonexistent/00000001'). Expected: HTTP 400, Error.Code 'CFNRegistryException', Error.Message exactly 'You must register as a publisher with CloudFormation to call test-type.'. Observed: HTTP 404, 'TypeNotFoundException', "The type 'Sim2Real::Probe::Nonexistent' cannot be found.".
3. Same expectation via the TypeName spelling: cfn.test_type(Type='RESOURCE', TypeName='Sim2RealTest::NoSuchResource::Type') — confirms the divergence is the missing gate, not the member set.
4. Ordering check that a message-only fix would leave broken: register a private extension type so that it DOES resolve in the account, then call TestType against it from an account that is not a registered publisher. Expected: still refused 400 CFNRegistryException with the same fixed sentence. Observed: the emulator proceeds past the (now successful) registry lookup instead of refusing.
5. Positive control that must keep working after the fix: cfn.describe_type(Type='RESOURCE', TypeName='AWS::SNS::Topic') answers 200 with TypeName='AWS::SNS::Topic' and Visibility='PUBLIC'.
6. Negative control that must keep working after the fix: DescribeType against the same nonexistent type version ARN must still answer 404 TypeNotFoundException with "The type 'Sim2Real::Probe::Nonexistent' cannot be found.".
