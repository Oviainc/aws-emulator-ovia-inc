Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__emr.deploy-emr-security-configuration-resource-lifecycle cloudformation__emr.deploy-emr-security-configuration-malformed-json-rejected cloudformation__emr.deploy-emr-studio-iam-resource-lifecycle
Issue-Key: cloudformation__emr.deploy-emr-security-configuration-resource-lifecycle|run#12|AssertionError|assert listed == [ref_name], (
Issue-Key: cloudformation__emr|DescribeSecurityConfiguration|InvalidRequestException
Issue-Key: cloudformation__emr|CreateStack|ValidationError
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudformation__emr: an AWS::EMR::SecurityConfiguration stack reaches CREATE_COMPLETE without creating the configuration in EMR, and AWS::EMR::Studio is refused as an unrecognized resource type (2 causes)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes clustered from this run's failures under the `cloudformation__emr` service heading. Each cause is numbered and carries its own failing check ids; a fixer must close BOTH before submitting.

--- Cause 1 of 2: a CREATE_COMPLETE stack declaring AWS::EMR::SecurityConfiguration provisions nothing in EMR, so the configuration it reports as created does not exist ---
AWS: recorded behavior only (the EMR Smithy model does not describe CloudFormation). AWS::EMR::SecurityConfiguration is a published CloudFormation resource type: CreateStack with a template whose resource carries Name + SecurityConfiguration reaches CREATE_COMPLETE, !Ref resolves to the configuration's Name, and the configuration then exists in EMR — DescribeSecurityConfiguration(Name=<that name>) answers 200 with Name == the submitted name and SecurityConfiguration == the submitted document (Smithy: DescribeSecurityConfiguration output = Name, SecurityConfiguration, CreationDateTime; errors InvalidRequestException (client, 400) and InternalServerException), and ListSecurityConfigurations reports that name exactly once. After the stack is deleted the configuration is gone and DescribeSecurityConfiguration refuses 400 InvalidRequestException with 'Security configuration with name "<Name>" does not exist.' (fixed text 'Security configuration with name "' / '" does not exist.'; the variable part is the Name from the request).
Emulator: the stack does reach CREATE_COMPLETE and the Ref output is the right name, but EMR never learns about the resource: DescribeSecurityConfiguration(Name=<the stack's name>) is refused 400 InvalidRequestException 'Security configuration with name "<name>" does not exist.' while the stack is still CREATE_COMPLETE, and ListSecurityConfigurations returns an empty list where the Ref'd name must appear once. The CloudFormation side is therefore a stub provisioner: it records a resource it never creates in the owning service.
Why: an infrastructure-as-code client that declares an EMR security configuration in a stack and then uses it (RunJobFlow SecurityConfiguration=<name>, or any read/audit of it) is told by CloudFormation that it exists and by EMR that it does not; every negative control that needs the positive leg (same template with a well-formed document) is unreachable, and stack deletion cannot be distinguished from stack creation because neither state is observable in EMR.
Why (same cause, second symptom): because the positive leg never materialises, the malformed-document leg of the same check is never reached, so CloudFormation's handler-failure path for this resource type is unverified too.
Fix: the CloudFormation crate's resource provisioner for AWS::EMR::SecurityConfiguration — route the resource's create/delete lifecycle to the EMR crate's CreateSecurityConfiguration / DeleteSecurityConfiguration (and surface EMR's refusal as the resource's CREATE_FAILED handler reason) instead of recording a synthesized resource only in stack state.
Traces: cloudformation__emr.deploy-emr-security-configuration-resource-lifecycle, cloudformation__emr.deploy-emr-security-configuration-malformed-json-rejected.

--- Cause 2 of 2: CreateStack refuses AWS::EMR::Studio as an unrecognized resource type ---
AWS: recorded behavior only. AWS::EMR::Studio (with AWS::EMR::StudioSessionMapping) is a published CloudFormation resource type; a template declaring it is accepted — CreateStack answers 200 with a StackId, the stack provisions the Studio through EMR, and !Ref resolves to the es-* Studio id.
Emulator: HTTP 400, error code 'ValidationError', message 'Template format error: Unrecognized resource types: [AWS::EMR::Studio]'. Fixed text: 'Template format error: Unrecognized resource types: [' + the list of types taken from the submitted template + ']'; AWS emits that sentence only for types it genuinely does not support.
Why: EMR Studio cannot be deployed by a stack at all, so the whole Studio/IAM deploy lifecycle (studio id, its IAM wiring, its session mappings, its deletion) is unreachable for every IaC client.
Fix: the CloudFormation crate's resource-type registry and provisioners — register AWS::EMR::Studio (and AWS::EMR::StudioSessionMapping) and route their lifecycle to the EMR crate's CreateStudio / DeleteStudio / CreateStudioSessionMapping.
Related open issue, which does NOT cover this: 20261104T102001Z-emr-8-net-new-root-causes-createvpcendpoint-refuses.md describes the same unrecognized-type refusal under the `emr` service heading (its Cause 2) and carries only the emr check key emr.deploy-emr-studio-via-cloudformation-session-mapping-usable; the cloudformation__emr check below is not in it and must be re-replayed.
Traces: cloudformation__emr.deploy-emr-studio-iam-resource-lifecycle.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (service heading `cloudformation__emr`: 0/3 passing).

Cause 1 — cloudformation__emr.deploy-emr-security-configuration-resource-lifecycle [mismatch]:
  expected: after the stack reaches CREATE_COMPLETE, ListSecurityConfigurations reports the Ref'd configuration name exactly once.
  actual:   AssertionError: expected ListSecurityConfigurations to report the Ref 'sim2real-emr-secconf-<run suffix>' once, got [].
Cause 1 — cloudformation__emr.deploy-emr-security-configuration-malformed-json-rejected [error]:
  the positive-control stack (same template, well-formed document) reached CREATE_COMPLETE and its Ref output equalled the submitted Name, then
  expected: DescribeSecurityConfiguration(Name=<that name>) -> 200 with Name and the submitted SecurityConfiguration document.
  actual:   botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the DescribeSecurityConfiguration operation: Security configuration with name "sim2real-emr-secconf-ok-<run suffix>" does not exist. (the name is a per-run value; the divergence is that the configuration is absent while the stack is CREATE_COMPLETE).

Cause 2 — cloudformation__emr.deploy-emr-studio-iam-resource-lifecycle [error]:
  expected: CreateStack with a template declaring 'Type: AWS::EMR::Studio' -> 200 with a StackId.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EMR::Studio].

## Steps to Reproduce
Boot the emulator and replay these checks from suite simparity-behavioral-aws-v1 (service heading `cloudformation__emr`):
1. Cause 1 — CreateStack with Resources: {SecConf: {Type: 'AWS::EMR::SecurityConfiguration', Properties: {Name: <name>, SecurityConfiguration: <well-formed EMR security-configuration document>}}} and Outputs: {ConfigName: {Value: {Ref: SecConf}}}. Poll DescribeStacks until CREATE_COMPLETE, read the Outputs Ref, then call emr.DescribeSecurityConfiguration(Name=<name>) (expect 200 with the submitted document; observe 400 InvalidRequestException 'does not exist') and emr.ListSecurityConfigurations (expect the name once; observe []). Then DeleteStack and re-check that DescribeSecurityConfiguration now refuses.
2. Cause 1, second check — repeat with a second stack whose SecurityConfiguration document is malformed JSON; expect ROLLBACK_COMPLETE with a resource-handler CREATE_FAILED reason carrying EMR's malformed-document refusal.
3. Cause 2 — CreateStack with a template whose Resources declare 'Type: AWS::EMR::Studio'. Expect 200 + StackId; observe 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::EMR::Studio]'.
