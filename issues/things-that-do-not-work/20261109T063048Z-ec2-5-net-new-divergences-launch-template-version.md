Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asgssh-launch-template-latest-and-default-resolve-to-different-versions ec2.asgssh-launch-template-version-from-missing-source-rejected ec2.launch-template-version-management ec2.op-contract-modify-launch-template ec2.op-contract-associate-address-already-associated-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asgssh-launch-template-latest-and-default-resolve-to-different-versions|run#13|AssertionError|assert latest_no == 2 and latest == {**base, "KeyName": key}, (
Issue-Key: ec2.asgssh-launch-template-version-from-missing-source-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ec2.launch-template-version-management|run#17|AssertionError|assert dup_message == "Launch template name already in use.", f"unexpected duplicate-name message {dup_message!r}"
Issue-Key: ec2.op-contract-modify-launch-template|run#17|AssertionError|assert message == "The launch template version 999 is not found for the specified launch template.", message
Issue-Key: ec2.op-contract-associate-address-already-associated-rejected|run#31|AssertionError|assert code2 == "InvalidInstanceID.Malformed", f"unexpected error code: {code2}"
Issue-Commit: bdb5184d827fec951c886c257ec0292728205109

# EC2: 5 net-new divergences — launch-template version data invents MetadataOptions.State, three launch-template refusal messages are emulator-authored prose, and AssociateAddress misclassifies an unissued instance id

> _Note: this issue was identified against the code base at commit `bdb5184d827fec951c886c257ec0292728205109`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplateVersion

## Description
Five net-new root causes found by replaying the behavioral suite against the emulator. All five are in the EC2 service (sdkId ec2, crate services/service-com-amazonaws-ec2-amazonec2), which is bootstrapped and answering, so each is a fidelity defect rather than a bootstrap gap. Related OPEN issues exist in the same area but do NOT carry these replay keys and do not cover these causes: 20261108T203125Z-ec2-launch-template-version-operations-default-version-delete.md (launch-template version ops: default-version delete), 20261104T142958Z-ec2-ten-ec2-refusal-messages-are-emulator-authored-prose.md and 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md (other operations' refusal prose), 20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md (other operations' error codes). Five causes must be closed before this issue is submitted.

Cause 1 of 5: launch-template version data reports a MetadataOptions.State the request never set.
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models ResponseLaunchTemplateData.MetadataOptions as LaunchTemplateInstanceMetadataOptions, whose members (State, HttpTokens, HttpPutResponseHopLimit, HttpEndpoint, HttpProtocolIpv6, InstanceMetadataTags) are all optional; State is documented as "The state of the metadata option changes" (pending | applied) — i.e. the state of applying metadata options to a running instance, not a property of stored launch-template data. Recorded behavior: a launch template created with LaunchTemplateData.MetadataOptions = {HttpTokens: required} reads back, through DescribeLaunchTemplateVersions, a MetadataOptions carrying only the members the request set; State is absent. The derived version created with SourceVersion inherits exactly the source version's members plus the ones the new LaunchTemplateData supplies, and nothing else.
Emulator: DescribeLaunchTemplateVersions for $Latest returns LaunchTemplateData whose MetadataOptions is {"State": "applied", "HttpTokens": "required"} — a State member the request never supplied — so version data is not equal to the submitted data plus the inherited data.
Why: a client that round-trips a launch-template version (reads it, edits it, resubmits it, or diffs it against its desired configuration, as CloudFormation/CDK and Auto Scaling tooling do) sees a member it never set. Diffs report permanent drift, and resubmitting the read-back data sends a State the create API does not accept.
Fix: the launch-template version read/serialization path — populate only the MetadataOptions members the stored version actually holds; do not default State when the stored version has none.
Traces: ec2.asgssh-launch-template-latest-and-default-resolve-to-different-versions.

Cause 2 of 5: CreateLaunchTemplateVersion's missing-SourceVersion refusal message is emulator-authored prose.
AWS: recorded behavior only (EC2's ec2Query errors are not modeled as Smithy error shapes). A CreateLaunchTemplateVersion naming a SourceVersion that does not exist is refused with HTTP 400, error code InvalidLaunchTemplateId.VersionNotFound, and the message "Launch template version does not exist." — that whole string is fixed text; it interpolates no version number and no template id.
Emulator: same status and code, but the message is "Launch template version '<n>' does not exist for launch template '<lt-id>'" (observed: "Launch template version '9' does not exist for launch template 'lt-00000000000000081'"; the quoted number is the request's SourceVersion and the quoted id is the request's LaunchTemplateId — both are per-run values, the divergence is the wording and the fact that any values appear at all).
Why: a client that matches on the refusal text (log assertions, user-facing surfacing, retry/branching on a known message) does not recognise AWS's message.
Fix: the CreateLaunchTemplateVersion handler's validation/refusal construction for InvalidLaunchTemplateId.VersionNotFound.
Traces: ec2.asgssh-launch-template-version-from-missing-source-rejected.

Cause 3 of 5: CreateLaunchTemplate's duplicate-name refusal message is emulator-authored prose.
AWS: recorded behavior only. A second CreateLaunchTemplate with a LaunchTemplateName already in use in the account/region is refused with HTTP 400, error code InvalidLaunchTemplateName.AlreadyExistsException, and the message "Launch template name already in use." — fixed text, with no interpolated name.
Emulator: same code, but the message is "The launch template name '<name>' is already in use" (observed with the request's LaunchTemplateName interpolated, and no trailing period).
Why: same client impact as Cause 2 — text-matching clients and operators comparing emulator output against AWS see a different message for the same refusal class.
Fix: the CreateLaunchTemplate handler's duplicate-name validation/refusal construction.
Traces: ec2.launch-template-version-management.

Cause 4 of 5: ModifyLaunchTemplate's unknown-DefaultVersion refusal message is emulator-authored prose.
AWS: recorded behavior only. ModifyLaunchTemplate with a DefaultVersion that was never created is refused with HTTP 400, error code InvalidLaunchTemplateId.VersionNotFound, and the message "The launch template version <n> is not found for the specified launch template." — fixed text around one variable part, the requested version number, unquoted; the template id does not appear.
Emulator: same code, but the message is "Launch template version '<n>' does not exist for launch template '<lt-id>'" (observed "Launch template version '999' does not exist for launch template 'lt-00000000000000080'") — i.e. the same single template the emulator uses for Cause 2, although AWS words these two refusals differently even though both carry InvalidLaunchTemplateId.VersionNotFound.
Why: the two AWS messages for this one code are distinct; a client (or a human runbook) that distinguishes "bad SourceVersion on create" from "bad DefaultVersion on modify" by message cannot do so against the emulator, and neither emulator string matches AWS.
Fix: the ModifyLaunchTemplate handler's DefaultVersion validation/refusal construction (and keep it distinct from CreateLaunchTemplateVersion's, Cause 2).
Traces: ec2.op-contract-modify-launch-template.

Cause 5 of 5: AssociateAddress classifies a well-formed but never-issued instance id as NotFound instead of Malformed.
AWS: recorded behavior only. EC2 instance ids carry an internal checksum that is validated before any lookup, so an id that is syntactically well formed (correct i- prefix and suffix length) but was never issued is refused with HTTP 400, error code InvalidInstanceID.Malformed and the message 'Invalid id: "<the id from the request>"' (fixed text "Invalid id: " plus the request's InstanceId in double quotes). InvalidInstanceID.NotFound is AWS's answer only for an id that passes the checksum and still does not resolve.
Emulator: AssociateAddress with such an id answers error code InvalidInstanceID.NotFound, so the pre-lookup malformed-id class never occurs.
Why: a client that distinguishes a typo/corrupted id (Malformed — never retry, surface to the user) from a resource that has gone away or is not yet visible (NotFound — retry or re-resolve) takes the wrong branch, and retry loops wait on an id that can never exist.
Fix: the shared EC2 instance-id parsing/validation used by the AssociateAddress handler — validate the id's checksum and refuse InvalidInstanceID.Malformed before the state lookup that yields InvalidInstanceID.NotFound.
Traces: ec2.op-contract-associate-address-already-associated-rejected.

## Evidence
Cause 1 — check ec2.asgssh-launch-template-latest-and-default-resolve-to-different-versions:
  expected $Latest LaunchTemplateData == submitted base + KeyName, i.e. MetadataOptions == {'HttpTokens': 'required'}
  actual   AssertionError: $Latest should be v2 = v1's fields plus KeyName; got v2 {'IamInstanceProfile': {...}, 'InstanceType': 't3.micro', 'KeyName': ..., 'MetadataOptions': {'State': 'applied', 'HttpTokens': 'required'}}
  (only divergence: the extra MetadataOptions.State member)

Cause 2 — check ec2.asgssh-launch-template-version-from-missing-source-rejected (CreateLaunchTemplateVersion, SourceVersion='9'):
  code     InvalidLaunchTemplateId.VersionNotFound (matches AWS)
  expected message 'Launch template version does not exist.'
  actual   message "Launch template version '9' does not exist for launch template 'lt-00000000000000081'"

Cause 3 — check ec2.launch-template-version-management (second CreateLaunchTemplate with the same name):
  code     InvalidLaunchTemplateName.AlreadyExistsException (matches AWS)
  expected message 'Launch template name already in use.'
  actual   message "The launch template name 'sim2real-behav-lt-2bbadb21d32b' is already in use"

Cause 4 — check ec2.op-contract-modify-launch-template (ModifyLaunchTemplate, DefaultVersion='999'):
  code     InvalidLaunchTemplateId.VersionNotFound (matches AWS)
  expected message 'The launch template version 999 is not found for the specified launch template.'
  actual   message "Launch template version '999' does not exist for launch template 'lt-00000000000000080'"

Cause 5 — check ec2.op-contract-associate-address-already-associated-rejected (AssociateAddress with a well-formed, never-issued InstanceId):
  expected code InvalidInstanceID.Malformed, message 'Invalid id: "<requested id>"'
  actual   AssertionError: unexpected error code: InvalidInstanceID.NotFound
  (the earlier Resource.AlreadyAssociated assertions in the same check passed)

## Steps to Reproduce
Re-replay the five named checks against the emulator (suite simparity-behavioral-aws-v1).

Cause 1: ec2.create_launch_template(LaunchTemplateName=N, LaunchTemplateData={'InstanceType':'t3.micro','IamInstanceProfile':{'Name':P},'MetadataOptions':{'HttpTokens':'required'}}); ec2.create_launch_template_version(LaunchTemplateId=ID, SourceVersion='1', LaunchTemplateData={'KeyName':K}); ec2.describe_launch_template_versions(LaunchTemplateId=ID, Versions=['$Latest']) -> LaunchTemplateVersions[0].LaunchTemplateData.MetadataOptions must contain only HttpTokens (no State).

Cause 2: create a launch template (one version), then ec2.create_launch_template_version(LaunchTemplateId=ID, SourceVersion='9', LaunchTemplateData={'KeyName':K}) -> expect 400 InvalidLaunchTemplateId.VersionNotFound with message exactly 'Launch template version does not exist.'; DescribeLaunchTemplateVersions must still list only version 1.

Cause 3: ec2.create_launch_template(LaunchTemplateName=N, ...) twice with the same N -> second call expects 400 InvalidLaunchTemplateName.AlreadyExistsException with message exactly 'Launch template name already in use.'

Cause 4: create a template, add version 2, ec2.modify_launch_template(LaunchTemplateId=ID, DefaultVersion='2') (succeeds), then ec2.modify_launch_template(LaunchTemplateId=ID, DefaultVersion='999') -> expect 400 InvalidLaunchTemplateId.VersionNotFound with message exactly 'The launch template version 999 is not found for the specified launch template.', and DescribeLaunchTemplates must still report DefaultVersionNumber 2.

Cause 5: allocate two VPC EIPs, run an instance, associate EIP1 to it, then ec2.associate_address(InstanceId=<well-formed instance id of the same shape that was never issued>, AllocationId=<alloc2>) -> expect 400 InvalidInstanceID.Malformed with message 'Invalid id: "<that id>"'.
