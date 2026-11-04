Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.deploy-workgroup-cfn-then-query-honors-config athena.deploy-workgroup-cloudformation-enforces-configuration athena.deploy-workgroup-cloudformation-enforces-output-location athena.deploy-datacatalog-cloudformation-queryable
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetWorkGroup|InvalidRequestException
Issue-Key: athena|GetDataCatalog|InvalidRequestException
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# CloudFormation stacks containing AWS::Athena::WorkGroup / AWS::Athena::DataCatalog report success but create no Athena resource (2 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkGroup

## Description
A CloudFormation stack whose template declares an Athena workgroup or data catalog completes, but the resource does not exist in Athena afterwards: the very next GetWorkGroup / GetDataCatalog on the declared name is refused as not found. The CFN provisioning path for these two Athena resource types does not reach the Athena control plane, so no CFN-deployed Athena configuration is ever in effect. 2 causes.

Cause 1 of 2: AWS::Athena::WorkGroup declared in a stack is never created.
AWS: Smithy: GetWorkGroup returns WorkGroup { Name, State, Configuration, Description, CreationTime } and binds InvalidRequestException (httpError 400) only when the workgroup does not exist. After a stack containing an AWS::Athena::WorkGroup reaches CREATE_COMPLETE, GetWorkGroup on that resource's Name succeeds with HTTP 200 and the Configuration the template declared (ResultConfiguration.OutputLocation, EnforceWorkGroupConfiguration, etc.), and queries submitted to that workgroup honour it.
Emulator: GetWorkGroup on the declared name is refused with HTTP 400 InvalidRequestException and the message 'WorkGroup <name> is not found.', so the check cannot even reach its configuration assertions.
Why: infrastructure-as-code is silently a no-op for Athena workgroups — a stack reports success while nothing was provisioned, and every subsequent query that depends on the declared output location or enforcement setting fails or writes nowhere.
Fix: the CloudFormation resource provider for AWS::Athena::WorkGroup — its create handler must call the athena crate's CreateWorkGroup and register the physical resource; the athena crate's workgroup state store must then serve it.
Traces: athena.deploy-workgroup-cfn-then-query-honors-config, athena.deploy-workgroup-cloudformation-enforces-configuration, athena.deploy-workgroup-cloudformation-enforces-output-location.

Cause 2 of 2: AWS::Athena::DataCatalog declared in a stack is never created.
AWS: Smithy: GetDataCatalog returns DataCatalog { Name, Description, Type, Parameters } and binds InvalidRequestException (httpError 400) only when the catalog does not exist. After a stack containing an AWS::Athena::DataCatalog reaches CREATE_COMPLETE, GetDataCatalog on that Name succeeds with HTTP 200 and the declared Type/Parameters, and the catalog is queryable.
Emulator: GetDataCatalog on the declared name is refused with HTTP 400 InvalidRequestException, message 'DataCatalog <name> is not found.' (note this message is itself non-AWS — AWS words it 'DataCatalog <name> was not found.').
Why: a CFN-declared federated/Glue catalog is absent, so every query routed through it fails and stack-managed catalog lifecycle (update, delete) manages nothing.
Fix: the CloudFormation resource provider for AWS::Athena::DataCatalog — its create handler must call the athena crate's CreateDataCatalog and register the physical resource.
Traces: athena.deploy-datacatalog-cloudformation-queryable.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. All four checks fail with [error] — an unhandled refusal on the first read after the stack completed.

Cause 1 — athena.deploy-workgroup-cfn-then-query-honors-config: botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetWorkGroup operation: WorkGroup sim2real-behav-wg-11dc037e9be6 is not found. Same exception in athena.deploy-workgroup-cloudformation-enforces-configuration (WorkGroup sim2real-athena-wg-7dc53be39bdd is not found.) and athena.deploy-workgroup-cloudformation-enforces-output-location (WorkGroup sim2real-behav-wg-8d8336fe79ac is not found.). Expected: HTTP 200 with WorkGroup.Configuration matching the template.
Cause 2 — athena.deploy-datacatalog-cloudformation-queryable: botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetDataCatalog operation: DataCatalog sim2real-athena-dc-771fbd440e26 is not found. Expected: HTTP 200 with DataCatalog.Type/Parameters matching the template.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the four athena checks listed in trace_ids.
3. Minimal manual repro:
   a. CreateStack with a template declaring an AWS::Athena::WorkGroup (Name=W, WorkGroupConfiguration.ResultConfiguration.OutputLocation=s3://<bucket>/out/, EnforceWorkGroupConfiguration=true); wait for CREATE_COMPLETE; call GetWorkGroup(WorkGroup=W) — expect 200 with that Configuration; observe 400 InvalidRequestException '... is not found.'
   b. CreateStack with a template declaring an AWS::Athena::DataCatalog (Name=C, Type=GLUE, Parameters={catalog-id:<account>}); wait for CREATE_COMPLETE; call GetDataCatalog(Name=C) — expect 200 with that Type/Parameters; observe 400 InvalidRequestException '... is not found.'
