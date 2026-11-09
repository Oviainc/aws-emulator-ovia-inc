Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam__lakeformation.deploy-cfn-lakeformation-permissions-grant-round-trip-and-revoke cloudformation__glue__iam__lakeformation.deploy-cfn-lakeformation-principalpermissions-invalid-permission-rejected
Issue-Key: cloudformation__glue__iam__lakeformation|CreateStack|ValidationError
Issue-Commit: e19d7761cdba7dfb41b7452b9b8e275a18501f91

# cloudformation__glue__iam__lakeformation: AWS::LakeFormation::PrincipalPermissions is not a registered resource type, so CreateStack refuses every template that grants Lake Formation permissions

> _Note: this issue was identified against the code base at commit `e19d7761cdba7dfb41b7452b9b8e275a18501f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::LakeFormation::PrincipalPermissions, so template validation refuses the whole template at CreateStack and no stack is ever created.

AWS: AWS::LakeFormation::PrincipalPermissions is a public CloudFormation resource type (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-lakeformation-principalpermissions.html). CreateStack accepts a template declaring it, provisions it by calling Lake Formation GrantPermissions for the declared Principal/Resource/Permissions, and the stack reaches CREATE_COMPLETE. Its Ref and read-only attributes are the two halves of the composite identifier: Fn::GetAtt PrincipalIdentifier is a JSON object {"DataLakePrincipalIdentifier": <principal>} and Fn::GetAtt ResourceIdentifier is a JSON object with one populated member (here Database = {CatalogId, Name}) and the remaining Resource members (Catalog, Table, TableWithColumns, DataLocation, DataCellsFilter, LFTag, LFTagPolicy) present and null. Removing the resource in an UpdateStack revokes exactly the permissions it granted and leaves permissions it did not grant (e.g. the database's implicit ALL for an IAM-access-control account) in place. The sibling legacy type AWS::LakeFormation::Permissions in the same template IS recognized by the emulator — only the newer type is missing. recorded behavior only for the exact refusal text.

Emulator: CreateStack is refused with HTTP 400, error code 'ValidationError' and message 'Template format error: Unrecognized resource types: [AWS::LakeFormation::PrincipalPermissions]'. Fixed text of that message is 'Template format error: Unrecognized resource types: ' followed by the bracketed list of the type names the template declared that the registry does not know — here the single name taken from the request's Resources block. Because the call raises, nothing downstream runs: no role, no Glue databases, no grant, no Ref/GetAtt outputs, no update-driven revoke, no delete.

Why: a client that models Lake Formation grants in CloudFormation (the documented, supported way to grant table/database permissions as infrastructure) cannot deploy at all. The failure surfaces as a template-format error, which reads to the client as a bug in its own template rather than a missing provider, so an IaC pipeline that works against AWS stops at the first LakeFormation grant stack.

Fix: CloudFormation's resource-type registry plus a new AWS::LakeFormation::PrincipalPermissions resource handler (create -> lakeformation GrantPermissions, delete -> RevokePermissions, update -> revoke/grant of the changed set), with the Ref/Fn::GetAtt attribute projection described above wired into the stack's Outputs resolution.

Traces: the two failing checks below, one happy-path round trip (grant, read back through ListPermissions, revoke by update, delete) and one rejection check (an invalid Permissions value) — both die on the same CreateStack refusal before asserting anything about Lake Formation.

## Evidence
Expected: CreateStack accepts the template; DescribeStacks reports CREATE_COMPLETE; Outputs carry Ref(LegacyPerm) = 'LakeFormation-<role arn>::<database arn>', Fn::GetAtt NewPerm.PrincipalIdentifier = {"DataLakePrincipalIdentifier": <role arn>} and Fn::GetAtt NewPerm.ResourceIdentifier = {"Catalog": null, "Database": {"CatalogId": <account>, "Name": <db2>}, "Table": null, "TableWithColumns": null, "DataLocation": null, "DataCellsFilter": null, "LFTag": null, "LFTagPolicy": null}; lakeformation ListPermissions then reports (['ALL','DESCRIBE'], []) for the role on each database, and after the permission-removing UpdateStack only (['ALL'], []).

Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::LakeFormation::PrincipalPermissions]

## Steps to Reproduce
1. CreateStack with Capabilities=['CAPABILITY_NAMED_IAM'] and a template containing an AWS::IAM::Role, two AWS::Glue::Database resources, an AWS::LakeFormation::Permissions resource on the first database and an AWS::LakeFormation::PrincipalPermissions resource on the second:
   {"Type": "AWS::LakeFormation::PrincipalPermissions", "Properties": {"Principal": {"DataLakePrincipalIdentifier": {"Fn::GetAtt": ["PrincipalRole", "Arn"]}}, "Resource": {"Database": {"CatalogId": "<account>", "Name": "<db2>"}}, "Permissions": ["DESCRIBE"], "PermissionsWithGrantOption": []}}
2. Observe the CreateStack call itself raise ValidationError 'Template format error: Unrecognized resource types: [AWS::LakeFormation::PrincipalPermissions]' instead of returning a StackId.
3. Same result when the PrincipalPermissions resource carries an invalid Permissions value (the rejection check): the type is refused before any per-property validation can run.
