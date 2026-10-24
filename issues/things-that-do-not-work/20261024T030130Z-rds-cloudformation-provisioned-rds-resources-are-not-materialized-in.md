Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.deploy-rds-instance-cfn-parameter-group-effective rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBInstances|DBInstanceNotFound
Issue-Key: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored|run#14|AssertionError|assert echoed.get("DbPassword") == "****", (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CloudFormation-provisioned RDS resources are not materialized in RDS, and NoEcho stack parameters are returned in clear text

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two causes observed through the RDS deployment checks. Both stacks reach CREATE_COMPLETE, so the defects are in what CloudFormation actually did, not in the stack status.

Cause 1 of 2: an AWS::RDS::DBInstance that CloudFormation reports as created does not exist in RDS. AWS: when a stack containing AWS::RDS::DBInstance reaches CREATE_COMPLETE, the resource's PhysicalResourceId is the DBInstanceIdentifier of a DB instance that RDS itself will return from DescribeDBInstances; the two views cannot disagree. (Smithy: DescribeDBInstances models DBInstanceNotFoundFault — client, httpError 404, awsQueryError code DBInstanceNotFound — which is exactly what AWS returns only for an identifier that was never created.) Emulator: after the stack completes, DescribeDBInstances on the physical id CloudFormation reported answers 404 DBInstanceNotFound ('DBInstance <stack>-instance-<suffix> not found.'). The identifier also carries a CloudFormation-minted numeric suffix, i.e. CFN minted a name for a resource it never created in RDS. Why: every IaC workflow is broken end to end — a client that deploys a database with CloudFormation and then addresses it through the RDS API (to read its endpoint, attach a parameter group, snapshot it, or delete it) finds nothing, and stack deletion cannot reconcile a resource that does not exist. Fix: the CloudFormation resource provisioner for AWS::RDS::DBInstance — it must create the instance through the RDS service state and record that identifier as the PhysicalResourceId. Traces: rds.deploy-rds-instance-cfn-parameter-group-effective.

Cause 2 of 2: DescribeStacks returns the value of a NoEcho template parameter instead of masking it. AWS: a template parameter declared NoEcho: true is masked everywhere CloudFormation echoes parameters back — DescribeStacks returns the literal four-asterisk string '****' as that parameter's ParameterValue, never the submitted value, regardless of its length. Recorded behavior only for the exact mask string, which AWS documents as '****'. Emulator: DescribeStacks returns the submitted secret verbatim (a 24-character value, the master password the stack was deployed with). Why: this is a secret disclosure — any principal with cloudformation:DescribeStacks can read the database master password out of the stack description, and a client or log pipeline that records DescribeStacks output persists it. Fix: the DescribeStacks (and ListStacks/stack-parameter) serializer must substitute the mask for every parameter whose template declaration carries NoEcho. Traces: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored.

Service attribution note: both causes are grounded in checks whose id leads with the 'rds' token, so this issue carries service 'rds'; the handlers to change are CloudFormation's AWS::RDS::DBInstance provisioner and CloudFormation's DescribeStacks parameter serializer.

## Evidence
Cause 1: rds.deploy-rds-instance-cfn-parameter-group-effective — botocore.errorfactory.DBInstanceNotFoundFault: An error occurred (DBInstanceNotFound) when calling the DescribeDBInstances operation: DBInstance sim2real-rds-cfn-ddd86b3b4df8-instance-902598130012 not found. (the stack had reached CREATE_COMPLETE and the identifier is the one CloudFormation reported).
Cause 2: rds.deploy-rds-1-protected-stack-delete-blocked-then-snapshot-retain-honored — AssertionError: NoEcho parameter DbPassword was not masked by DescribeStacks (got a value of length 24, expected '****').

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: CreateStack with a template containing an AWS::RDS::DBInstance (plus its DBSubnetGroup/DBParameterGroup), wait for CREATE_COMPLETE, read the DBInstance resource's PhysicalResourceId from DescribeStackResources, then call Action=DescribeDBInstances with that DBInstanceIdentifier — it answers 404 DBInstanceNotFound.
3. Cause 2: CreateStack with a template declaring a parameter with NoEcho: true (e.g. DbPassword), pass a value, then call DescribeStacks and read that parameter's ParameterValue — the submitted value is returned instead of '****'.
