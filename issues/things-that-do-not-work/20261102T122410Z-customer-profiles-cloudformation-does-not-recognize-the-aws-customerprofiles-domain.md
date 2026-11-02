Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.e2e-connect-create-customer-profiles-lifecycle
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|CreateStack|ValidationError
Issue-Commit: 75d52ea0c257a701244c018d7c3f550d51e1d46f

# CloudFormation does not recognize the AWS::CustomerProfiles::Domain resource type, so a Customer Profiles domain cannot be deployed by a stack (1 cause)

> _Note: this issue was identified against the code base at commit `75d52ea0c257a701244c018d7c3f550d51e1d46f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation template declaring a Customer Profiles domain is refused at CreateStack as an unknown resource type, so the whole deploy-driven Customer Profiles lifecycle is unreachable.
AWS: recorded behavior only (the Customer Profiles Smithy model does not describe CloudFormation resource types; the model does show the domain is a first-class resource — CreateDomain POST /domains/{DomainName}, GetDomain GET /domains/{DomainName}). The recording shows CreateStack ACCEPTING a template whose Resources contain a `AWS::CustomerProfiles::Domain` resource and the stack reaching CREATE_COMPLETE, after which the domain exists in Customer Profiles and can be driven through its data plane (seed, search, create, restore, sweep) and then released by the stack's destroy. `AWS::CustomerProfiles::Domain` is a supported CloudFormation resource type, so AWS never answers ValidationException for it.
Emulator: CreateStack is refused with HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::CustomerProfiles::Domain]` — the resource type is absent from the emulator's CloudFormation resource registry, so phase 1/6 (deploy) of the lifecycle fails and no later phase runs. The error code and the fixed text 'Template format error: Unrecognized resource types: [' + ']' are AWS's wording for a genuinely unknown type; the divergence is that this type IS known to AWS, so the refusal itself must not happen.
Why: every infrastructure-as-code path to Customer Profiles is blocked. A client that provisions its domain by stack (the normal way a Connect/Customer Profiles estate is stood up) cannot deploy at all, and because the refusal is a template-format ValidationError it looks like the customer's template is malformed rather than like missing emulator coverage. Nothing downstream of the domain — profiles, object types, search — can be exercised through the deployed path.
Fix: belongs in the CloudFormation resource-type registry and its Customer Profiles provisioning handler (the CFN crate's resource-type table plus a resource implementation that calls CreateDomain/UpdateDomain/DeleteDomain and exposes the domain name/ARN as attributes), so a stack carrying AWS::CustomerProfiles::Domain validates, reaches CREATE_COMPLETE with the domain really created, and releases it on destroy.
Traces: customer-profiles.e2e-connect-create-customer-profiles-lifecycle (phase 1/6 deploy).

## Evidence
Suite: simparity-behavioral-aws-v1, check customer-profiles.e2e-connect-create-customer-profiles-lifecycle [error], listed under service heading '### customer-profiles' (cloudformation is the replay's deploy front end, not the subject).

expected: CreateStack accepts the template at deployments/customer-profiles-domain/template.json (Resources include AWS::CustomerProfiles::Domain) and the stack reaches CREATE_COMPLETE; the six phases deploy / pre_invoke / solve / post_invoke / cleanup / destroy then run against the created domain.
actual:   _PhaseError: [phase 1/6 deploy] ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CustomerProfiles::Domain]

The five remaining phases never executed, so this one refusal is the only observed divergence for the check.

## Steps to Reproduce
1. Author a CloudFormation template whose Resources contain one resource of type AWS::CustomerProfiles::Domain (properties: a DomainName and DefaultExpirationDays), with an Output exporting the domain name.
2. cloudformation.CreateStack(StackName=<unique>, TemplateBody=<that template>).
   Expect the stack to be accepted and to reach CREATE_COMPLETE; observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CustomerProfiles::Domain]'.
3. After CREATE_COMPLETE, customerprofiles.GetDomain(DomainName=<the deployed name>) must succeed, and the domain must accept CreateProfile / SearchProfiles.
4. cloudformation.DeleteStack must release the domain (GetDomain then answers ResourceNotFoundException).
