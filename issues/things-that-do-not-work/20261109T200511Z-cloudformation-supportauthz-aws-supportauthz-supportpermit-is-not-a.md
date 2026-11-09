Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__supportauthz.deploy-cfn-supportauthz-supportpermit-round-trip-and-delete cloudformation__supportauthz.deploy-cfn-supportauthz-supportpermit-invalid-name-rejected
Issue-Key: cloudformation__supportauthz|CreateStack|ValidationError
Issue-Commit: 1793706895cc87acc2112e1210fbb2da9ff5c952

# cloudformation__supportauthz: AWS::SupportAuthZ::SupportPermit is not a registered resource type — CreateStack refuses every support-permit template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `1793706895cc87acc2112e1210fbb2da9ff5c952`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry does not know AWS::SupportAuthZ::SupportPermit, so CreateStack rejects the template outright and no SupportAuthZ permit can ever be provisioned through CloudFormation. Both failing checks die on their FIRST CreateStack call, so nothing downstream of stack creation (permit round-trip through GetSupportPermit/ListSupportPermits/ListTagsForResource, Ref/GetAtt outputs Arn/PermitId/Status/CreatedAt, PROPERTY_VALIDATION rollback of an invalid Name, and rollback/delete absence) was exercised at all — those behaviors are unverified behind this single refusal, not known-good.

AWS: CloudFormation supports the AWS::SupportAuthZ::SupportPermit resource type. A CreateStack whose template declares it is ACCEPTED (200, StackId returned) and the resource is provisioned by calling SupportAuthZ CreateSupportPermit (Smithy: SupportAuthZ restJson1, CreateSupportPermit POST /support-permits, required members name/permit/signingKeyInfo, optional description/supportCaseDisplayId/tags; name has @length min 1 max 256 and @pattern ^[a-zA-Z0-9]{1,256}$). Ref and GetAtt Arn yield the permit ARN (CreateSupportPermitOutput.arn, @pattern ^arn:[a-z0-9-]+:[a-z0-9-]+:[a-z0-9-]*:[0-9]{12}:.+$); the permit is then readable by GetSupportPermit (GET /support-permits/{supportPermitIdentifier}, by ARN or name) with status ACTIVE, and after stack delete it answers ResourceNotFoundException (modeled, httpError 404). CloudFormation reserves the "Template format error: Unrecognized resource types: [...]" ValidationError for types it genuinely does not support — never for a type in its supported set.

Emulator: CreateStack fails with HTTP 400, error code ValidationError, message "Template format error: Unrecognized resource types: [AWS::SupportAuthZ::SupportPermit]". The fixed text is "Template format error: Unrecognized resource types: [" + the comma-separated list of unknown type names from the submitted template + "]"; the bracketed type name is the value taken from the request's Resources/<logical id>/Type. The same message appears for both checks, including the one whose template is otherwise valid apart from an intentionally invalid permit Name — i.e. the refusal happens before any property validation.

Why: a client (CDK app, template, or `aws cloudformation create-stack`) that provisions a support permit declaratively cannot create the stack at all. There is no StackId to poll, no stack events, no rollback path, and the invalid-Name negative case cannot be distinguished from an unsupported-type error — a caller testing its template's property validation gets a template-format error instead of the PROPERTY_VALIDATION failure AWS reports, so it concludes its template is malformed rather than its property value wrong.

Fix: the CloudFormation template processor's resource-type registry plus a resource handler for AWS::SupportAuthZ::SupportPermit that maps template Properties (Name, Description, Permit{Actions,Resources,Conditions}, SigningKeyInfo.KmsKey, Tags) onto SupportAuthZ CreateSupportPermit/GetSupportPermit/DeleteSupportPermit, exposes Ref and GetAtt Arn/PermitId/Status/CreatedAt, validates Name against ^[a-zA-Z0-9]{1,256}$ as a PROPERTY_VALIDATION event with path /Resources/<logical id>/Properties/Name, and deletes the permit on stack delete so SupportAuthZ answers ResourceNotFoundException afterwards.

Traces: the two failing checks listed under the cloudformation__supportauthz heading, both erroring on the first CreateStack.

## Evidence
Expected (AWS): CreateStack with a template containing {"Permit": {"Type": "AWS::SupportAuthZ::SupportPermit", "Properties": {...}}} returns 200 with a StackId; the stack reaches CREATE_COMPLETE; ListSupportPermits/GetSupportPermit show the permit with status ACTIVE and the ARN that Ref/GetAtt Arn returned; the variant whose Name contains a hyphen reaches ROLLBACK_COMPLETE with a VALIDATION_ERROR operation event (ValidationName PROPERTY_VALIDATION, ValidationStatus FAILED, FailureMode FAIL, path /Resources/Permit/Properties/Name, reason of the form "Property value [<name>] does not match pattern: ^[a-zA-Z0-9]{1,256}$") and leaves no permit behind, GetSupportPermit answering ResourceNotFoundException "A support permit with identifier <identifier> could not be found".

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SupportAuthZ::SupportPermit]

Observed identically in both checks:
- cloudformation__supportauthz.deploy-cfn-supportauthz-supportpermit-round-trip-and-delete (happy path)
- cloudformation__supportauthz.deploy-cfn-supportauthz-supportpermit-invalid-name-rejected (negative: invalid Name)
In the second check the refusal fires on the POSITIVE-control stack (valid alphanumeric Name), so the type is unrecognized regardless of property values.

## Steps to Reproduce
1. Build and boot the emulator; point a botocore/boto3 client at it with cloudformation and supportauthz endpoints.
2. Build a JSON template with two resources: an AWS::KMS::Key (KeySpec ECC_NIST_P384, KeyUsage SIGN_VERIFY, a key policy allowing supportauthz.amazonaws.com to kms:Sign/GetPublicKey/DescribeKey) and a resource of Type "AWS::SupportAuthZ::SupportPermit" whose Properties are Name (alphanumeric, <=256 chars), Description, Permit {Actions:{AllActions:{}}, Resources:{Resources:[{Fn::GetAtt:[SigningKey,Arn]}]}, Conditions:[{AllowBefore:<ISO-8601 UTC timestamp in the future>}]}, SigningKeyInfo:{KmsKey:{Fn::GetAtt:[SigningKey,Arn]}}; add Outputs for Ref and GetAtt Arn/PermitId/Status/CreatedAt on the permit.
3. cf.create_stack(StackName=<unique>, TemplateBody=<that template>, Tags=[{Key:sim2real,Value:<unique>}]).
4. Observe ValidationError "Template format error: Unrecognized resource types: [AWS::SupportAuthZ::SupportPermit]" instead of a StackId.
5. Repeat with a permit Name containing a hyphen: the same refusal appears, so the intended PROPERTY_VALIDATION/ROLLBACK_COMPLETE path is never reached.
