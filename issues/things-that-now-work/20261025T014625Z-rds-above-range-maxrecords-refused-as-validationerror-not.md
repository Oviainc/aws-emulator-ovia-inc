Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.describe-major-engine-versions rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance rds.copy-option-group rds.authorize-ingress-invalid-cidr-format-rejected rds.duplicate-ingress-rule-second-call-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.describe-major-engine-versions|run#8|AssertionError|assert err.get("Code") == "InvalidParameterValue", f"unexpected error: {err}"
Issue-Key: rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows|_wait_stack#17|AssertionError|raise AssertionError(f"stack {stack_name} reached {st!r} waiting for {want!r}: {reasons!r}")
Issue-Key: rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance|_wait_stack#17|AssertionError|raise AssertionError(f"stack {stack_name} reached {st!r} waiting for {want!r}: {reasons!r}")
Issue-Key: rds.copy-option-group|run#8|AssertionError|assert code == ERR_OPTION_GROUP_NOT_FOUND, f"unexpected error code: {code}"
Issue-Key: rds.authorize-ingress-invalid-cidr-format-rejected|run#5|AssertionError|raise AssertionError(f"unexpected error creating DB security group: {create_code!r}")
Issue-Key: rds.duplicate-ingress-rule-second-call-rejected|run#6|AssertionError|assert create_code is None, f"unexpected error creating DB security group: {create_code!r}"
Issue-Commit: 5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b

# RDS: above-range MaxRecords refused as 'ValidationError' not 'InvalidParameterValue', CloudFormation-minted AWS::EC2::Subnet ids break every CFN MySQL stack, and CopyOptionGroup/CreateDBSecurityGroup still answer NotImplemented (4 causes)

> _Note: this issue was identified against the code base at commit `5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
FOUR net-new root causes, all grounded in failing behavioral checks of the `rds` service, none of them marked ALREADY FILED or OWNED by the replay report. FOUR causes — close all four before submitting. Each cause below names the open issue it is adjacent to, where one exists.

Note on a neighbouring FIXED issue: 20261024T030130Z-rds-describedbmajorengineversions-stub.md (fixed) did land — DescribeDBMajorEngineVersions no longer answers 501 and its happy path now returns real mysql major versions, so this is NOT a reopen of that issue. What cause 1 reports is the classification of its range refusal.

Cause 1 of 4: an above-range MaxRecords is refused with error code `ValidationError` where AWS uses `InvalidParameterValue`.
AWS: Smithy DescribeDBMajorEngineVersions.MaxRecords targets `MaxRecords` with @range min 20 max 100, and the operation declares no error shapes of its own (errors: []), so a range violation surfaces through RDS's common awsQuery parameter error. Recorded behavior: HTTP 400 with Error.Code `InvalidParameterValue` and the message "1 validation error detected: Value '1000' at 'maxRecords' failed to satisfy constraint: Member must have value less than or equal to 100" — a message built from the request (member name + the violated constraint), so that template is the requirement and the emulator already reproduces it verbatim.
Emulator: HTTP 400 with Error.Type `Sender`, Error.Code `ValidationError` and exactly that message. Only the code diverges.
Why: `ValidationError` is not an RDS awsQuery error code. Every SDK's error-code matching, every `except ClientError if code == 'InvalidParameterValue'` branch and every retry/catch rule keyed on the documented code silently fails to fire, and a caller cannot distinguish a parameter violation from any other generic failure.
Fix: the shared MaxRecords/range-constraint refusal mapping in the RDS awsQuery request-validation layer (the one every paginated Describe* shares) must raise the service's `InvalidParameterValue` query error rather than the framework's generic validation error. This is the same shared validator described as Cause 6 of the OPEN issue 20261022T092913Z-rds-awsquery-error-envelopes-omit-requestid-and-misclassify.md (which was grounded in DescribeDBEngineVersions MaxRecords=19 on the wire corpus and reported the emitted code as `ValidationException`); the emitted code has since changed to `ValidationError` but is still not `InvalidParameterValue`, and DescribeDBMajorEngineVersions is a second operation reaching the same validator. Close this cause by fixing the shared mapping, not the one handler.
Traces: rds.describe-major-engine-versions.

Cause 2 of 4: a CloudFormation-provisioned AWS::EC2::Subnet is given a synthesized physical resource id that is not an EC2 subnet id, so !Ref of it is refused by RDS and every CloudFormation stack that puts a MySQL DB instance in its own VPC ends CREATE_FAILED.
AWS: a provisioned AWS::EC2::Subnet's PhysicalResourceId IS its EC2 subnet id, so `!Ref <Subnet>` yields `subnet-` followed by 8 or 17 hexadecimal characters, which is what AWS::RDS::DBSubnetGroup's SubnetIds requires. Smithy models CreateDBSubnetGroup with the client error `InvalidSubnet` (httpError 400, awsQueryError code InvalidSubnet) for a value that is not a subnet of the account. Recorded behavior for these templates is CREATE_COMPLETE: the subnets are created, their ids are substituted, the DB subnet group and the DB instance are created, and the instance then serves SQL on Endpoint.Address.
Emulator: CloudFormation gives the AWS::EC2::Subnet resource a generic name-derived physical id of the form `<stack-name>-<LogicalId>-<hex>`; `!Ref` resolves to that string; RDS then correctly refuses it with `InvalidSubnet: <that string> is not an EC2 subnet id (expected subnet- followed by 8 or 17 hexadecimal characters)`, and DescribeStacks reports CREATE_FAILED with "The following resource(s) failed to create: [DbSubnetGroup]."
Why: no IaC deployment that creates its own VPC/subnets and then places an RDS DB subnet group, DB instance, DB cluster or DB proxy in them can ever reach CREATE_COMPLETE, so the whole CloudFormation+RDS deployment path (change sets, in-place Modify vs Replacement, the data plane behind the stack's Endpoint output) is unreachable; and the failure is misattributed to RDS, which is behaving correctly here.
Hypothesis: AWS::EC2::Subnet has no dedicated resource provider in the CloudFormation engine and falls through to the generic physical-id generator instead of provisioning through EC2 and recording the returned SubnetId.
Fix: the CloudFormation engine's resource-provisioning / physical-resource-id resolution for AWS::EC2::Subnet — it must provision through the EC2 service and record the real `subnet-...` id as the resource's physical id so Ref/GetAtt hand downstream services a usable value. The RDS side needs no change. This is the same cause as Cause 3 of the OPEN issue 20261024T120544Z-rds-db-subnet-group-fidelity-wrong-not-found.md, which carries only the replay key rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored; the two checks listed here are additional, un-owned replay keys for it and must be re-replayed before this cause is closed. It is also distinct from the open 20261022T193451Z-ec2-createsubnet-stub.md: the EC2 CreateSubnet API being a stub is a separate gap, while what fails here is CloudFormation's own physical-id minting.
Traces: rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows, rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance.

Cause 3 of 4: CopyOptionGroup answers NotImplemented, so neither its happy path nor its not-found boundary can be observed.
AWS: Smithy models CopyOptionGroup with the client errors OptionGroupNotFoundFault (httpError 404), OptionGroupAlreadyExistsFault and OptionGroupQuotaExceededFault; copying an existing option group produces an independently describable target group, and an unknown SourceOptionGroupIdentifier is refused with Code `OptionGroupNotFoundFault`.
Emulator: HTTP 501 with Error.Code `NotImplemented` ("operation CopyOptionGroup is scaffolded but not implemented in this emulator") for every call, including the one that should be a 404 refusal.
Why: a caller that copies an option group as part of an engine-upgrade or clone workflow gets a server error instead of either the copied group or the modelled refusal, and cannot tell a missing source group from an emulator gap.
Fix: implement the handler at services/service-com-amazonaws-rds-amazonrdsv19/src/ops/copy_option_group.rs against the shared option-group store. The OPEN issue 20261024T030130Z-rds-copyoptiongroup-stub.md names that operation and file; the replay report accounted only 1 blocked check for it (rds.copy-option-group-round-trip), so rds.copy-option-group is an additional un-owned replay key for the same work.
Traces: rds.copy-option-group.

Cause 4 of 4: CreateDBSecurityGroup answers NotImplemented, so the legacy (EC2-Classic-era) DB-security-group ingress contract cannot be exercised at all.
AWS: Smithy models CreateDBSecurityGroup with DBSecurityGroupAlreadyExistsFault, DBSecurityGroupQuotaExceededFault and DBSecurityGroupNotSupportedFault. On a current VPC-only account either the create is refused with `DBSecurityGroupNotSupportedFault`, or it succeeds and every subsequent AuthorizeDBSecurityGroupIngress on the VPC-backed group is refused with `InvalidParameterValue` regardless of whether the CIDR is well formed (recorded message: "VPC DB Security Groups cannot be modified with this API version.  Please use an API version between 2012-01-15 and 2012-10-31 to modify this group."). Both shapes are a client-visible contract; neither is a 501.
Emulator: HTTP 501 Error.Code `NotImplemented` from CreateDBSecurityGroup, so the checks cannot even reach the ingress call.
Why: a caller gets a server error for a modelled operation and can observe neither of the two refusals AWS actually produces, so code that branches on DBSecurityGroupNotSupportedFault to fall back to VPC security groups never takes that branch.
Fix: implement services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_security_group.rs (and the ingress authorization path that reads its store). The OPEN issue 20261024T030130Z-rds-createdbsecuritygroup-stub.md names that operation and file and the report counts 3 blocked checks for it; the two keys below are the two of those three that the report did not mark, and must be re-replayed with it.
Traces: rds.authorize-ingress-invalid-cidr-format-rejected, rds.duplicate-ingress-rule-second-call-rejected.

## Evidence
Cause 1 — rds.describe-major-engine-versions [mismatch]:
  AssertionError at `assert err.get("Code") == "InvalidParameterValue", f"unexpected error: {err}"`
  actual: {'Type': 'Sender', 'Code': 'ValidationError', 'Message': "1 validation error detected: Value '1000' at 'maxRecords' failed to satisfy constraint: Member must have value less than or equal to 100"}
  The message already matches AWS exactly; only Error.Code diverges (ValidationError vs InvalidParameterValue). The first half of the check (DescribeDBMajorEngineVersions(Engine='mysql') returning real major versions) passed, confirming the operation itself is implemented.
  Smithy: DescribeDBMajorEngineVersions input MaxRecords @range {min: 20, max: 100}; operation errors: [].

Cause 2 — rds.deploy-rds-2-cfn-mysql-param-change-in-place-keeps-instance-and-rows [mismatch]:
  AssertionError: stack sim2real-rds2-ip-82dc0cb8ab5d reached 'CREATE_FAILED' waiting for 'CREATE_COMPLETE': ['sim2real-rds2-ip-82dc0cb8ab5d: The following resource(s) failed to create: [DbSubnetGroup]. InvalidSubnet: sim2real-rds2-ip-82dc0cb8ab5d-Subnet0-860494405E2D is not an EC2 subnet id (expected subnet- followed by 8 or 17 hexadecimal characters)']
  rds.deploy-rds-2-cfn-mysql-sql-then-dbname-change-replaces-instance [mismatch]:
  AssertionError: stack sim2real-rds2-cfn-500ab56af643 reached 'CREATE_FAILED' waiting for 'CREATE_COMPLETE': ['sim2real-rds2-cfn-500ab56af643: The following resource(s) failed to create: [DbSubnetGroup]. InvalidSubnet: sim2real-rds2-cfn-500ab56af643-Subnet0-1883FCC1DAB0 is not an EC2 subnet id (expected subnet- followed by 8 or 17 hexadecimal characters)']
  In both cases the rejected value is the CloudFormation-minted PhysicalResourceId of the template's AWS::EC2::Subnet resource, substituted into the DBSubnetGroup's SubnetIds by !Ref. The generated-name/hash parts differ per run; the SHAPE of the id (`<stack>-<LogicalId>-<hex>` instead of `subnet-<hex>`) is the requirement that is violated.

Cause 3 — rds.copy-option-group [mismatch]:
  AssertionError at `assert code == ERR_OPTION_GROUP_NOT_FOUND, f"unexpected error code: {code}"` — "unexpected error code: NotImplemented". The preceding CreateOptionGroup succeeded, so the store exists; only CopyOptionGroup is a stub.

Cause 4 — rds.authorize-ingress-invalid-cidr-format-rejected [mismatch]:
  AssertionError: unexpected error creating DB security group: 'NotImplemented'
  rds.duplicate-ingress-rule-second-call-rejected [mismatch]:
  AssertionError: unexpected error creating DB security group: 'NotImplemented'
  Both abort on the first CreateDBSecurityGroup call; neither reaches AuthorizeDBSecurityGroupIngress.

## Steps to Reproduce
0. Boot the emulator from this checkout and replay the behavioral suite; the six check ids in Issue-Trace-Ids are the ones to re-replay.

1. Cause 1: POST an awsQuery request with Action=DescribeDBMajorEngineVersions, Engine=mysql, MaxRecords=1000. The response is HTTP 400 with <Error><Type>Sender</Type><Code>ValidationError</Code>... where AWS answers <Code>InvalidParameterValue</Code> with the same "1 validation error detected: Value '1000' at 'maxRecords' ..." message. Repeat with MaxRecords=19 (below the modelled minimum) and with another paginated RDS Describe* to confirm the shared validator, not one handler, produces the code.

2. Cause 2: CreateStack with a template containing an AWS::EC2::VPC, two AWS::EC2::Subnet resources in distinct AZs, an AWS::RDS::DBSubnetGroup whose SubnetIds is [!Ref Subnet0, !Ref Subnet1], an AWS::RDS::DBParameterGroup and an AWS::RDS::DBInstance (engine mysql) in that group. Poll DescribeStacks: the stack reaches CREATE_FAILED and DescribeStackEvents shows DbSubnetGroup CREATE_FAILED with `InvalidSubnet: <stack>-Subnet0-<hex> is not an EC2 subnet id`. Read the Subnet0 resource's PhysicalResourceId via DescribeStackResources — it is the synthesized `<stack>-Subnet0-<hex>` string instead of a `subnet-...` id. Expected: CREATE_COMPLETE, with Ref Subnet0 resolving to a real subnet id.

3. Cause 3: Action=CreateOptionGroup (mysql, the engine's default MajorEngineVersion), then Action=CopyOptionGroup with SourceOptionGroupIdentifier naming a group that does not exist. Expected HTTP 404 Code OptionGroupNotFoundFault; actual HTTP 501 Code NotImplemented. Then CopyOptionGroup from the group that DOES exist: expected 200 and a target group DescribeOptionGroups can read back; actual 501.

4. Cause 4: Action=CreateDBSecurityGroup with a fresh DBSecurityGroupName and a description. Expected either 200 (then AuthorizeDBSecurityGroupIngress on the VPC-backed group refused InvalidParameterValue for both a malformed and a well-formed CIDR, with no IPRanges stored) or the modelled DBSecurityGroupNotSupportedFault; actual HTTP 501 Code NotImplemented.
