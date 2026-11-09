Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog.deploy-servicecatalog-provisioned-product-round-trip
Issue-Key: cloudformation__s3__servicecatalog__sts|CreateStack|ValidationError
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__s3__servicecatalog__sts: CreateStack rejects AWS::ServiceCatalog::PortfolioPrincipalAssociation and ::CloudFormationProvisionedProduct as unrecognized resource types

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One cause.

Cause 1 of 1: the CloudFormation resource-type registry does not know the Service Catalog principal-association and provisioned-product resource types.

AWS: AWS::ServiceCatalog::PortfolioPrincipalAssociation and AWS::ServiceCatalog::CloudFormationProvisionedProduct are supported CloudFormation resource types; CreateStack accepts them, the principal association is created, and the provisioned product is launched (and is then describable through servicecatalog DescribeProvisionedProduct). AWS answers 'Template format error: Unrecognized resource types' only for types that do not exist.

Emulator: CreateStack answers 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ServiceCatalog::PortfolioPrincipalAssociation, AWS::ServiceCatalog::CloudFormationProvisionedProduct]'. Fixed text 'Template format error: Unrecognized resource types: [...]', list taken from the template.

Why: self-service provisioning through Service Catalog cannot be expressed in IaC at all — neither granting a principal access to a portfolio nor launching a product — and the caller is told the types are invalid rather than unimplemented.

Fix: register AWS::ServiceCatalog::PortfolioPrincipalAssociation and AWS::ServiceCatalog::CloudFormationProvisionedProduct in the CloudFormation resource-type registry and wire their handlers to the Service Catalog association and provisioned-product state.

Traces: cloudformation__servicecatalog.deploy-servicecatalog-provisioned-product-round-trip (listed under this heading).

## Evidence
Expected: CreateStack accepts the template; the portfolio principal association and the CloudFormation-provisioned product are created, round-trip through the servicecatalog API, and delete with the stack.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ServiceCatalog::PortfolioPrincipalAssociation, AWS::ServiceCatalog::CloudFormationProvisionedProduct].

## Steps to Reproduce
1. Build a template with a Service Catalog portfolio/product plus AWS::ServiceCatalog::PortfolioPrincipalAssociation (for an STS-assumed principal) and AWS::ServiceCatalog::CloudFormationProvisionedProduct.
2. cloudformation.create_stack(...) -> ValidationError 'Template format error: Unrecognized resource types: [...]'.
