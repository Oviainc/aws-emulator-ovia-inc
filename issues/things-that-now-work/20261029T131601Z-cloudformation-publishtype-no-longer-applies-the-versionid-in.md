Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-not-found-error-across-apis
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-not-found-error-across-apis|run#16|AssertionError|assert publish_error == expected_publish, (
Issue-Commit: 939fc85144c938b9bc6d1f441a638c73ec9013b4

# cloudformation: PublishType no longer applies the versionId-in-Arn rule to an in-account private type ARN — it answers CFNRegistryException 'Type not found in your account.' where AWS answers 'Only default version can be published. Please remove versionId from Arn'

> _Note: this issue was identified against the code base at commit `939fc85144c938b9bc6d1f441a638c73ec9013b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishType

## Description
REOPENS 20261029T040629Z-cloudformation-2-net-new-root-causes-stackset-operationnotfoundexception.md: its fix left the call refused.

That issue's Cause 2 moved PublishType's account-scoped existence lookup AHEAD of the versionId-in-Arn rule, on the recording of check cloudformation.type-registry-not-found-errors. The ordering is now inverted for the OTHER recorded case: a private type ARN whose account segment is the CALLER'S OWN account and whose last segment is a version is now refused with the existence message, where two checks record AWS refusing it with the versionId message. The call is still refused — only the refusal class/message changed sides — so this is the same cause re-surfacing on the half of the differential the fix did not cover, not a new defect.

One root cause of service `cloudformation`, grounded in one failing behavioral check.

Cause 1 of 1 — PublishType applies its account-scoped "is this extension registered" lookup to every ARN and so never reaches the rule about the ARN's version segment for an ARN in the caller's own account.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models `PublishType` with input `Arn` (target PrivateTypeArn, @length 0..1024, pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$`), `Type` (ThirdPartyType enum), `TypeName`, `PublicVersionNumber`; output `PublicTypeArn`; errors `CFNRegistryException` (errorKind client, httpError 400, awsQueryError CFNRegistryException/400) and `TypeNotFoundException` (client, httpError 404). The pattern admits any `type/...` suffix, so an ARN carrying a trailing version segment is well-formed input and is never a constraint violation. The refusal text is recorded behavior only: for a private type ARN of the shape `arn:aws:cloudformation:<region>:<CALLER'S OWN 12-digit account>:type/resource/<Type-Name-With-Dashes>/<version>` — i.e. one whose last segment is a version id — AWS answers HTTP 400, Error.Code `CFNRegistryException`, Error.Message exactly `Only default version can be published. Please remove versionId from Arn`, and it answers that whether or not the named extension is registered. That message is ENTIRELY fixed text: no part of it is taken from the request (not the ARN, not the type name, not the version) or from state. Two independent checks record this same refusal for a caller-account ARN ending in `/00000001`: cloudformation.type-not-found-error-across-apis and cloudformation.type-version-publish-and-test. The contrasting recording cited by the issue being reopened, cloudformation.type-registry-not-found-errors, sends an ARN whose account segment is NOT the caller's and records `Type not found in your account.` for it — so both messages are real AWS refusals of this one operation and the handler must produce each on its own input. Hypothesis (stated by neither the model nor either recording): the discriminator is the ARN's account segment — an ARN outside the caller's account cannot be resolved in "your account" at all and takes the existence refusal, while an in-account ARN is parsed and its version segment refused before the registry is consulted.

Emulator: the in-account versioned ARN is refused with the right status and code but the WRONG message — HTTP 400 `CFNRegistryException` with Error.Message `Type not found in your account.` (also entirely fixed text). That is the existence refusal, which means the ARN's trailing version segment was never examined on this path: the handler resolved the ARN against the account's registered types, found nothing, and stopped there.

Why: a client relying on AWS's behavior cannot tell the two refusal classes apart, and is told the wrong one. Registry/publishing tooling that handles the version refusal by stripping the version from the ARN and retrying (publish the default version) never matches and never retries; instead it reads "this type is not in my account" and goes off to register or re-name an extension whose identity was never the problem. Both messages carry the same code and status, so the Error.Message is the ONLY signal a caller has here, and it names the wrong cause. The defect is also symmetric to the earlier fix: closing it by simply swapping the order back would re-break cloudformation.type-registry-not-found-errors, so the handler must decide per ARN rather than fix one global order.

Fix: the PublishType handler's ARN reading and refusal ordering in the cloudformation crate's type-registry ops (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/publish_type.rs) and the shared private-type-ARN split helper it calls: the ARN's account segment and its trailing version segment must both be read before the registry lookup, so that an in-account ARN carrying a version is refused with `Only default version can be published. Please remove versionId from Arn` while an ARN that is not resolvable in the caller's account keeps answering `Type not found in your account.` Validation ORDER (per-ARN, not global) is the contract here, not just the message text. Related but not covering: 20261029T085553Z-cloudformation-activatetype-does-not-refuse-a-publictypearn-that.md is the same family of defect on ActivateType's PublicTypeArn and says nothing about PublishType.

Traces: cloudformation.type-not-found-error-across-apis (the PublishType assertion at run line 81).

## Evidence
Failing check cloudformation.type-not-found-error-across-apis [mismatch]:
  File "check_cloudformation_type_not_found_error_across_apis.py", line 81, in run
    assert publish_error == expected_publish, (
  AssertionError: PublishType against an unregistered type identifier: expected ('CFNRegistryException', 'Only default version can be published. Please remove versionId from Arn'), got ('CFNRegistryException', 'Type not found in your account.')

Request that diverges:
  cfn.publish_type(Arn='arn:aws:cloudformation:<region>:<caller-account-id>:type/resource/Sim2RealTest-NoSuchResource-Type/00000001')
  — the account id is the caller's own (from sts.GetCallerIdentity in the check); the type-name segment and the trailing '00000001' are per-run values filling the template, not part of the requirement. The ARN satisfies the modelled PrivateTypeArn pattern.

Already passing in the same check BEFORE this line, so the type-management surface is otherwise healthy and this is a fact about this ARN alone:
  - positive control cfn.describe_type(Type='RESOURCE', TypeName='AWS::SNS::Topic') -> TypeName='AWS::SNS::Topic', Visibility='PUBLIC'.
  - the five-API not-found loop for an unregistered TypeName all answered ('TypeNotFoundException', "The type 'Sim2RealTest::NoSuchResource::Type' cannot be found."): DescribeType, DeregisterType, SetTypeDefaultVersion, DeactivateType, SetTypeConfiguration.

Message breakdown for the diverging refusal (same code and status on both sides, so Error.Message is the whole divergence):
  expected — HTTP 400, Error.Code 'CFNRegistryException', Error.Message the whole fixed sentence 'Only default version can be published. Please remove versionId from Arn', NO variable parts.
  observed — HTTP 400, Error.Code 'CFNRegistryException', Error.Message 'Type not found in your account.', also entirely fixed text, but the message of the account-scoped existence refusal class.

Corroborating recording of the same rule (second check, which fails earlier for an unrelated cause): cloudformation.type-version-publish-and-test line 44 asserts PublishType(Arn='arn:aws:cloudformation:<region>:<caller-account>:type/resource/Sim2Real-Probe-Nonexistent/00000001') == ('CFNRegistryException', 'Only default version can be published. Please remove versionId from Arn').

The other side of the differential, which must keep passing: cloudformation.type-registry-not-found-errors sends PublishType(Arn='arn:aws:cloudformation:<region>:<account that is not the caller and owns no such type>:type/resource/<name>/00000001') and records ('CFNRegistryException', 'Type not found in your account.') — the behavior the reopened issue's fix installed, and the behavior now leaking onto the in-account ARN.

Smithy consulted: PublishType input {Type: ThirdPartyType enum; Arn: PrivateTypeArn @length 0..1024 pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$; TypeName @length 10..204 pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$; PublicVersionNumber @length min 5 pattern ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$}, output {PublicTypeArn: TypeArn}, errors = CFNRegistryException (client/400/CFNRegistryException) and TypeNotFoundException (client/404/TypeNotFoundException), idempotent.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation.type-not-found-error-across-apis.
2. Get the caller's account: sts.GetCallerIdentity -> <account>.
3. Call cfn.publish_type(Arn='arn:aws:cloudformation:<region>:<account>:type/resource/Sim2RealTest-NoSuchResource-Type/00000001'). Expected: HTTP 400, Error.Code 'CFNRegistryException', Error.Message exactly 'Only default version can be published. Please remove versionId from Arn'. Observed: HTTP 400, 'CFNRegistryException', 'Type not found in your account.'.
4. Repeat with a different in-account type-name segment and a different version segment (e.g. .../type/resource/Sim2Real-Probe-Nonexistent/00000002) — same expectation, same observed divergence, confirming the divergence is the per-ARN ordering and not the particular name or version value.
5. Regression guard for the fix the reopened issue installed, which must still hold: cfn.publish_type(Arn='arn:aws:cloudformation:<region>:<a 12-digit account that is NOT the caller>:type/resource/<name>/00000001') must still answer 400 CFNRegistryException 'Type not found in your account.'. A fix that merely swaps the global order back re-breaks cloudformation.type-registry-not-found-errors.
6. Negative controls that must keep working after the fix: DescribeType / DeregisterType / SetTypeDefaultVersion / DeactivateType / SetTypeConfiguration against the unregistered TypeName 'Sim2RealTest::NoSuchResource::Type' must still answer 404 TypeNotFoundException "The type 'Sim2RealTest::NoSuchResource::Type' cannot be found.".
7. Re-replay cloudformation.type-not-found-error-across-apis, cloudformation.type-version-publish-and-test and cloudformation.type-registry-not-found-errors together to confirm all three agree after the fix.
