Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog.deploy-servicecatalog-product-portfolio-constraints-round-trip cloudformation__servicecatalog.deploy-servicecatalog-stackset-constraint-round-trip cloudformation__servicecatalog.deploy-servicecatalog-stackset-constraint-account-list-rejected
Issue-Key: cloudformation__s3__servicecatalog|CreateStack|ValidationError
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__s3__servicecatalog: CreateStack rejects every AWS::ServiceCatalog constraint/service-action resource type as unrecognized

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause.

Cause 1 of 1: the CloudFormation resource-type registry does not know the Service Catalog constraint and service-action resource types.

AWS: AWS::ServiceCatalog::LaunchRoleConstraint, ::LaunchNotificationConstraint, ::LaunchTemplateConstraint, ::ResourceUpdateConstraint, ::ServiceAction, ::ServiceActionAssociation and ::StackSetConstraint are supported CloudFormation resource types; CreateStack accepts templates declaring them and provisions the corresponding Service Catalog constraints, whose ids then resolve through the servicecatalog API. The StackSetConstraint negative case (an account list AWS rejects) must be refused by the resource's own validation, not by template-format validation. A 'Template format error: Unrecognized resource types' ValidationError is AWS's answer only for types that do not exist.

Emulator: CreateStack answers 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ServiceCatalog::LaunchRoleConstraint, AWS::ServiceCatalog::LaunchNotificationConstraint, AWS::ServiceCatalog::LaunchTemplateConstraint, AWS::ServiceCatalog::ResourceUpdateConstraint, AWS::ServiceCatalog::ServiceAction, AWS::ServiceCatalog::ServiceActionAssociation]' and 'Template format error: Unrecognized resource types: [AWS::ServiceCatalog::StackSetConstraint]'. Fixed text is 'Template format error: Unrecognized resource types: [...]'; the bracketed list comes from the template.

Why: no Service Catalog portfolio can be governed through IaC — constraints, service actions and their associations cannot be declared at all — and the caller is told the types are invalid rather than unimplemented. The negative StackSetConstraint test also cannot distinguish a bad account list from a missing type.

Fix: register the AWS::ServiceCatalog::* constraint, ServiceAction and ServiceActionAssociation (and StackSetConstraint) resource types in the CloudFormation resource-type registry and wire their handlers to the Service Catalog constraint/service-action state, including the StackSetConstraint account-list validation.

Traces: cloudformation__servicecatalog.deploy-servicecatalog-product-portfolio-constraints-round-trip, cloudformation__servicecatalog.deploy-servicecatalog-stackset-constraint-round-trip, cloudformation__servicecatalog.deploy-servicecatalog-stackset-constraint-account-list-rejected (all listed under this heading).

## Evidence
Expected: CreateStack accepts the template, the constraints/service actions are provisioned and round-trip, and the invalid StackSetConstraint account list is refused by the resource's validation (stack rolls back).
Actual (constraints/service actions): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ServiceCatalog::LaunchRoleConstraint, AWS::ServiceCatalog::LaunchNotificationConstraint, AWS::ServiceCatalog::LaunchTemplateConstraint, AWS::ServiceCatalog::ResourceUpdateConstraint, AWS::ServiceCatalog::ServiceAction, AWS::ServiceCatalog::ServiceActionAssociation].
Actual (stack-set constraint, both checks): ... Template format error: Unrecognized resource types: [AWS::ServiceCatalog::StackSetConstraint].

## Steps to Reproduce
1. Create a Service Catalog portfolio/product template that also declares AWS::ServiceCatalog::LaunchRoleConstraint, ::LaunchNotificationConstraint, ::LaunchTemplateConstraint, ::ResourceUpdateConstraint, ::ServiceAction and ::ServiceActionAssociation.
2. cloudformation.create_stack(...) -> ValidationError 'Template format error: Unrecognized resource types: [...]'.
3. Repeat with a template declaring only AWS::ServiceCatalog::StackSetConstraint (valid and invalid AccountList) -> same ValidationError.
