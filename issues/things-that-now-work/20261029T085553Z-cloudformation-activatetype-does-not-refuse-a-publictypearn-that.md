Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-not-found-across-apis-rejected cloudformation.type-not-found-error-across-apis
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-not-found-across-apis-rejected|run#14|AssertionError|assert activate_error == expected_activate, (
Issue-Key: cloudformation.type-not-found-error-across-apis|run#19|AssertionError|assert activate_error == expected_activate, (
Issue-Commit: ede2de3f7102f867619bb62880127888c08c3e6a

# cloudformation: ActivateType does not refuse a PublicTypeArn that carries a public version number — it absorbs the version segment and answers the execution-role refusal instead of CFNRegistryException 'You cannot specify public version number in the arn'

> _Note: this issue was identified against the code base at commit `ede2de3f7102f867619bb62880127888c08c3e6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateType

## Description
One net-new root cause of service `cloudformation`, grounded in two failing behavioral checks. A fixer must close it before submitting.

Cause 1 of 1 — ActivateType's reading of `PublicTypeArn` ignores the ARN's trailing public-version segment, so the refusal AWS owes for a versioned public type ARN never happens and the request falls through to the next rule in the handler.

AWS: Smithy (com.amazonaws.cloudformation#CloudFormation) — `ActivateType` input member `PublicTypeArn` targets `ThirdPartyTypeArn` (@length 0..1024, pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$`), and the operation models exactly two errors: `CFNRegistryException` (errorKind client, httpError 400, awsQueryError code `CFNRegistryException`) and `TypeNotFoundException` (client, httpError 404). The model does not state the version rule, so for that rule: recorded behavior only — a public type ARN of the documented shape `arn:aws:cloudformation:<region>::type/<category>/<publisherId>/<type-name-with-dashes>/<version>`, i.e. one whose last segment is a public version number, is refused up front with HTTP 400, error code `CFNRegistryException`, Message `You cannot specify public version number in the arn`. That message is entirely FIXED text — no segment of it is taken from the request, the ARN, or any state, so there is no variable part to template. The refusal is ordered BEFORE the registry lookup for the named extension and BEFORE the rule that a public RESOURCE activation must name an `ExecutionRoleArn`; AWS never activates such an ARN, whether or not an execution role is supplied.

Emulator: the same call is refused with the right status and code but the WRONG message — HTTP 400 `CFNRegistryException` with Message `Please provide execution role to activate the Resource type`. That is the handler's own no-execution-role refusal (`NO_EXECUTION_ROLE`), which means the version-bearing ARN was accepted as naming a real extension: the ARN reader split off `<category>`/`<publisherId>`/`<type-name>` and silently discarded the trailing version segment, so the only thing left to refuse the request was the missing execution role. Supplying an `ExecutionRoleArn` therefore makes the call SUCCEED and write an activation record for an ARN AWS rejects outright.

Why: a client relying on AWS's behavior gets two things wrong. First, the diagnosis: code handling the documented refusal by stripping the version from the ARN and retrying never matches, and instead reads a message saying its IAM wiring is incomplete — a completely different remediation. Second, and worse, the refusal is not merely reworded but absent: a caller that does pass an execution role is told the activation succeeded and is handed an `Arn` for an extension activation that cannot exist on AWS, so every later `DescribeType`/`SetTypeConfiguration`/template reference built on it diverges from real AWS with no error anywhere in the sequence.

Fix: the `PublicTypeArn` reading in ActivateType's own handler — `PublicExtension::read`, the `Some(public_type_arn)` branch, in the cloudformation crate's ActivateType operation, together with the shared `registry::split_public_type_arn` helper it calls: the ARN's trailing public-version segment must be recognised and refused with the modelled `CFNRegistryException` before the execution-role rule and before any registry/state read. Validation ORDER is part of the contract here, not just the message text. Already-open neighbour, which does NOT cover this: 20261026T235900Z-cloudformation-7-net-new-root-causes-in-the.md Cause 5 covers a DIFFERENT public ARN shape — one carrying no type-name segment at all (`.../type/resource/<publisherId>/00000001`) — which must answer CFNRegistryException `TypeName is invalid` and answered `ValidationError`; that cause says nothing about a well-formed publisher+type-name ARN whose last segment is a version, which is the ARN both checks here send. 20261028T094216Z-cloudformation-the-public-extension-registry-exposes-no-activatable.md covers the empty PUBLIC/THIRD_PARTY catalog, a different surface.

Traces: cloudformation.type-not-found-across-apis-rejected, cloudformation.type-not-found-error-across-apis

## Evidence
Both failing checks reach the same ActivateType assertion only AFTER their earlier assertions on the same surface PASS, so the type-management surface is otherwise healthy and this is a fact about the versioned public ARN alone.

cloudformation.type-not-found-across-apis-rejected [mismatch]:
  File "check_cloudformation_type_not_found_across_apis_rejected.py", line 68, in run
    assert activate_error == expected_activate, (
  AssertionError: ActivateType returned ('CFNRegistryException', 'Please provide execution role to activate the Resource type'), expected ('CFNRegistryException', 'You cannot specify public version number in the arn')
  Request: cfn.activate_type(Type='RESOURCE', PublicTypeArn='arn:aws:cloudformation:<region>::type/resource/<32-hex publisherId>/SimParity-Fidelity-NoSuch<hex>/00000001')
  Already passing in the same check before this line: DeactivateType, DeregisterType, SetTypeConfiguration, SetTypeDefaultVersion and DescribeType all answered ('TypeNotFoundException', "The type '<Namespace::Service::Resource>' cannot be found."), and the DescribeType(AWS::SNS::Topic) positive control returned Visibility=PUBLIC.

cloudformation.type-not-found-error-across-apis [mismatch]:
  File "check_cloudformation_type_not_found_error_across_apis.py", line 90, in run
    assert activate_error == expected_activate, (
  AssertionError: ActivateType against an unregistered public type: expected ('CFNRegistryException', 'You cannot specify public version number in the arn'), got ('CFNRegistryException', 'Please provide execution role to activate the Resource type')
  Request: cfn.activate_type(PublicTypeArn='arn:aws:cloudformation:<region>::type/resource/bbbbbbbbbbbbbbbbbbbb/Sim2RealBogusPublisher-NoSuchResource-Type/00000001')  — note: no Type and no ExecutionRoleArn are sent; the ARN alone identifies the extension.
  Already passing in the same check before this line: the five-API TypeNotFoundException loop, and the sibling ordering rule for PublishType — PublishType(Arn=<private type ARN ending /00000001>) correctly answered ('CFNRegistryException', 'Only default version can be published. Please remove versionId from Arn'). So the emulator already implements the "ARN carries a version segment" refusal for PublishType and omits the equivalent one for ActivateType.

Message breakdown for the diverging refusal: expected code `CFNRegistryException`, HTTP 400; expected Message is the whole fixed sentence `You cannot specify public version number in the arn` with NO variable parts. The emulator's `Please provide execution role to activate the Resource type` is also entirely fixed text but belongs to a different refusal class (missing ExecutionRoleArn on a public RESOURCE activation) and is correct only for a request whose ARN carries no version.

Smithy consulted: ActivateType input (Type ThirdPartyType enum; PublicTypeArn ThirdPartyTypeArn @length 0..1024 pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$; PublisherId @length 1..40 pattern ^[0-9a-zA-Z]{12,40}$; TypeName @length 10..204 pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$; TypeNameAlias; AutoUpdate; LoggingConfig; ExecutionRoleArn RoleARN2 pattern ^arn:.+:iam::[0-9]{12}:role/.+$; VersionBump; MajorVersion @range 1..100000), output Arn (PrivateTypeArn), errors CFNRegistryException (client/400/CFNRegistryException) and TypeNotFoundException (client/404). The ARN sent by both checks satisfies the modelled PublicTypeArn pattern, so this refusal is the registry's, not a constraint violation.

Code citation for the fix site (read at the current checkout): services/service-com-amazonaws-cloudformation-cloudformation/src/ops/activate_type.rs — `PublicExtension::read` lines 374-435, whose `Some(public_type_arn)` branch (lines 383-417) calls `registry::split_public_type_arn(public_type_arn)` and keeps only (type_kind, publisher_id, type_name), never examining a trailing version segment; the execution-role refusal that then fires is lines 214-217 with the constant `NO_EXECUTION_ROLE` at line 448.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation.type-not-found-across-apis-rejected and cloudformation.type-not-found-error-across-apis.
2. Directly, with no ExecutionRoleArn: call ActivateType(PublicTypeArn='arn:aws:cloudformation:<region>::type/resource/bbbbbbbbbbbbbbbbbbbb/Sim2RealBogusPublisher-NoSuchResource-Type/00000001'). Expected: HTTP 400, Error.Code 'CFNRegistryException', Error.Message exactly 'You cannot specify public version number in the arn'. Observed: HTTP 400, 'CFNRegistryException', 'Please provide execution role to activate the Resource type'.
3. Same ARN with Type='RESOURCE' added: same expectation, same observed divergence (confirms the divergence is the ARN reading, not the member set).
4. Ordering check — repeat step 2 but ALSO pass ExecutionRoleArn='arn:aws:iam::<account>:role/<any-role>'. Expected: still refused 400 CFNRegistryException 'You cannot specify public version number in the arn'. Observed: the emulator ACCEPTS the activation and returns 200 with an Arn, writing a registry record for an ARN AWS refuses — this is the part a message-only fix would leave in place.
5. Contrast that already behaves correctly in the emulator and shows the intended shape of the fix: PublishType(Arn='arn:aws:cloudformation:<region>:<account>:type/resource/Sim2RealTest-NoSuchResource-Type/00000001') is refused 400 CFNRegistryException 'Only default version can be published. Please remove versionId from Arn'.
6. Negative control that must keep working after the fix: ActivateType with a version-free public ARN for a RESOURCE extension and no ExecutionRoleArn must still answer 400 CFNRegistryException 'Please provide execution role to activate the Resource type'.
