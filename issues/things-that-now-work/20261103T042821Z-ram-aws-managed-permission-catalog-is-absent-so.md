Issue-Source: trace-sweep-divergence
Issue-Trace: ram-permission-catalog-exploration
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-permission-catalog-exploration
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|GetPermission|2xx->4xx|$.permission
Issue-Key: ram|GetPermission|2xx->4xx|status
Issue-Commit: d36bb525c2331ae923943e9e5ae02c5704a8c1d0

# RAM AWS-managed permission catalog is absent, so GetPermission on an AWS managed permission ARN is refused

> _Note: this issue was identified against the code base at commit `d36bb525c2331ae923943e9e5ae02c5704a8c1d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPermission

## Description
1 cause.

Cause 1 of 1 — reading an AWS managed permission from the service catalog fails (fixture ram-permission-catalog-exploration, step task#2).
AWS: RAM ships a catalog of AWS managed permissions whose ARNs have no account component (arn:aws:ram::aws:permission/<PermissionName>). GetPermission on such an ARN succeeds with HTTP 200 and a ResourceSharePermissionDetail (Smithy: GetPermission input permissionArn:String required -> output permission: ResourceSharePermissionDetail) whose permissionType is AWS_MANAGED (Smithy enum member doc: 'AWS_MANAGED – Amazon Web Services created and manages this managed permission. You can associate it with your resource shares, but you can't modify it'), with the catalog entry's name, resourceType, policy document, version, defaultVersion/isResourceTypeDefault flags, featureSet STANDARD and status ATTACHABLE. The recording shows the entry for APIGatewayPrivateDomainNameManagementPolicyDefaultPermission (resourceType apigateway:Domainnames, a single apigateway:CreateAccessAssociation action, version '1'); the requirement is that the AWS managed permission the request names is served, not these particular values.
Emulator: GetPermission answers HTTP 400 with no permission member at all, so no AWS managed permission can be read.
Why: AWS managed permissions are the default permissions attached to resource shares. A client that discovers one (ListPermissions / resource-type defaults) and then reads it to choose a permission for CreateResourceShare or AssociateResourceSharePermission gets a hard refusal, and any flow built on the stock catalog cannot start.
Fix: seed the ram permission state with the AWS managed permission catalog (ARN without account id, permissionType AWS_MANAGED, per-resource-type default flags) and make the GetPermission handler resolve account-less arn:aws:ram::aws:permission/... ARNs against it rather than only against customer-created permissions. Related open issue 20261103T040218Z-ram-listpermissions-stub.md (ram ListPermissions unimplemented) shares the missing catalog but does not cover the GetPermission read path, which is what diverges here.

Traces: ram-permission-catalog-exploration#task#2.

## Evidence
ram-permission-catalog-exploration#task#2, GetPermission: status expected 200, got 400; $.permission missing — expected a ResourceSharePermissionDetail {arn: arn:aws:ram::aws:permission/APIGatewayPrivateDomainNameManagementPolicyDefaultPermission, name: APIGatewayPrivateDomainNameManagementPolicyDefaultPermission, permissionType: AWS_MANAGED, resourceType: apigateway:Domainnames, permission: '{ "Effect": "Allow", "Action": [ "apigateway:CreateAccessAssociation" ] }', version: '1', defaultVersion: true, isResourceTypeDefault: true, featureSet: STANDARD, status: ATTACHABLE, creationTime/lastUpdatedTime shaped-not-asserted}, got None.

## Steps to Reproduce
1. POST /getpermission with permissionArn = arn:aws:ram::aws:permission/APIGatewayPrivateDomainNameManagementPolicyDefaultPermission (any AWS managed permission ARN).
2. Emulator answers HTTP 400 with no permission member; AWS answers 200 with the AWS_MANAGED catalog entry.
