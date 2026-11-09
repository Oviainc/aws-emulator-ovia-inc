Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog.deploy-servicecatalog-product-portfolio-constraints-round-trip cloudformation__servicecatalog.deploy-servicecatalog-stackset-constraint-round-trip cloudformation__servicecatalog.deploy-servicecatalog-stackset-constraint-account-list-rejected cloudformation__servicecatalog.deploy-servicecatalog-provisioned-product-round-trip cloudformation__servicecatalog.deploy-servicecatalog-portfolio-tagoption-and-serviceaction-round-trip-and-delete cloudformation__servicecatalog.deploy-servicecatalog-tagoption-value-over-max-length-rejected
Issue-Key: cloudformation__s3__servicecatalog|CreateStack|ValidationError
Issue-Key: cloudformation__s3__servicecatalog__sts|CreateStack|ValidationError
Issue-Key: cloudformation__servicecatalog|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__servicecatalog: CloudFormation does not register ten AWS::ServiceCatalog::* resource types

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: ten modelled AWS::ServiceCatalog::* resource types are not recognized.
AWS: AWS::ServiceCatalog::LaunchRoleConstraint, LaunchNotificationConstraint, LaunchTemplateConstraint, ResourceUpdateConstraint, StackSetConstraint, ServiceAction, ServiceActionAssociation, TagOption, TagOptionAssociation, PortfolioPrincipalAssociation and CloudFormationProvisionedProduct are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions them; a StackSetConstraint with an invalid account list, and a TagOption value over the maximum length, are then refused and roll the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of declared AWS::ServiceCatalog::* types>]`. The fixed text is "Template format error: Unrecognized resource types: "; the list comes from the request.
Why: Service Catalog portfolios, constraints, service actions, tag options and provisioned products cannot be deployed through CloudFormation at all, so the whole IaC path for the service is dead.
Fix: register those AWS::ServiceCatalog::* types in the CloudFormation resource-type registry and add their resource handlers.
Traces: the six failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ServiceCatalog::LaunchRoleConstraint, AWS::ServiceCatalog::LaunchNotificationConstraint, AWS::ServiceCatalog::LaunchTemplateConstraint, AWS::ServiceCatalog::ResourceUpdateConstraint, AWS::ServiceCatalog::ServiceAction, AWS::ServiceCatalog::ServiceActionAssociation]; ... [AWS::ServiceCatalog::StackSetConstraint]; ... [AWS::ServiceCatalog::TagOption, AWS::ServiceCatalog::PortfolioPrincipalAssociation, AWS::ServiceCatalog::TagOptionAssociation, AWS::ServiceCatalog::ServiceAction]; ... [AWS::ServiceCatalog::TagOption]; ... [AWS::ServiceCatalog::PortfolioPrincipalAssociation, AWS::ServiceCatalog::CloudFormationProvisionedProduct]. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring any of the listed AWS::ServiceCatalog::* types.
3. Observe the ValidationError.
