Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.deploy-cfn-certificate-tags-propagate acm.deploy-acme-endpoint-via-cloudformation-behaves acm.deploy-cfn-certificate-validation-behaves
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DescribeCertificate|ValidationException
Issue-Key: acm|DescribeAcmeEndpoint|ValidationException
Issue-Key: acm.deploy-cfn-certificate-validation-behaves|run#9|AssertionError|assert resource_status == "CREATE_IN_PROGRESS", (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# CloudFormation-deployed ACM resources: the physical id is not an ARN, and a Certificate resource reports CREATE_COMPLETE while still PENDING_VALIDATION (3 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResource

## Description
Three distinct root causes in the CloudFormation provisioning of ACM resources, all surfaced by ACM behavioral checks (service token 'acm'). The open issue 20261022T143245Z-acm-deploy-and-cross-service-prerequisites-broken-describestackresource.md recorded an EARLIER state of this surface (DescribeStackResource answering 501 and stack Outputs carrying no OutputValue); both of those now answer, and these three causes are what the deployed-resource identity and lifecycle do instead. None of the three is covered by an open issue.

Cause 1 of 3: the PhysicalResourceId reported for an AWS::CertificateManager::Certificate resource is not an ACM certificate ARN.
AWS: CloudFormation sets a resource's PhysicalResourceId to the provider's identifier; for AWS::CertificateManager::Certificate that identifier IS the CertificateArn, so DescribeStackResource's PhysicalResourceId can be passed straight to acm:DescribeCertificate. Smithy ACM DescribeCertificate.CertificateArn targets Arn with pattern ^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$ and length 20..2048.
Emulator: feeding the emulator's own PhysicalResourceId back into DescribeCertificate is refused BY THE EMULATOR with 400 ValidationException: "1 validation error detected: Value of the input at 'certificateArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*" — so the value the CFN ACM resource provider stores as the physical id is not an ACM ARN at all (Hypothesis: a bare uuid or a logical-id-derived token).
Why: the standard way to find what a template created — resolve the stack resource to its physical id, then call the service — is impossible for stack-owned certificates: every such call is rejected as malformed. Tag propagation, drift checks and teardown verification of CFN-managed certificates all break.
Fix: the AWS::CertificateManager::Certificate resource provider in the CloudFormation service crate — set the physical resource id (and the Ref/Fn::GetAtt value) to the CertificateArn returned by ACM RequestCertificate.
Traces: acm.deploy-cfn-certificate-tags-propagate.

Cause 2 of 3: the same defect for the ACME-endpoint resource type — the identifier is not an acme-endpoint ARN.
AWS: an AWS::CertificateManager ACME-endpoint resource's identifier/attribute resolves to the AcmeEndpointArn; Smithy DescribeAcmeEndpoint.AcmeEndpointArn is required with pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$ and length 1..200.
Emulator: DescribeAcmeEndpoint on the value the stack hands back is refused with 400 ValidationException: "1 validation error detected: Value of the input at 'acmeEndpointArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+".
Why: an ACME endpoint provisioned by a template can never be looked up, updated or verified through the ACM API, so the deploy path for the whole ACME surface is unusable even though CloudFormation reports success.
Fix: the ACME-endpoint resource provider in the CloudFormation service crate — carry the AcmeEndpointArn as the physical id and as the Fn::GetAtt attribute value.
Traces: acm.deploy-acme-endpoint-via-cloudformation-behaves.

Cause 3 of 3: a Certificate resource is reported CREATE_COMPLETE the moment it is created, even though the certificate is PENDING_VALIDATION.
AWS: CloudFormation holds an AWS::CertificateManager::Certificate resource at CREATE_IN_PROGRESS until ACM issues it; the PhysicalResourceId is already assigned during CREATE_IN_PROGRESS, and for a domain that cannot be validated the stack eventually gives up and rolls back (recorded behavior noted in the check: the rollback lands within roughly a minute, deleting the certificate) — it never reports the resource CREATE_COMPLETE while the certificate is unvalidated.
Emulator: at the first observation of the physical id the resource is already ResourceStatus='CREATE_COMPLETE' with ResourceStatusReason=None, for a DNS-validated certificate on a domain with no hosted zone that ACM has not issued.
Why: a client gets a success signal for a certificate that cannot serve traffic — a stack that depends on the certificate (listener, custom domain, distribution) proceeds and then fails at attach time, and no rollback ever happens. The create-time wait on validation, which is the entire point of the resource, is absent.
Fix: the AWS::CertificateManager::Certificate resource provider's create-stabilisation in the CloudFormation service crate — keep the resource CREATE_IN_PROGRESS until DescribeCertificate reports ISSUED, and fail/roll back otherwise.
Traces: acm.deploy-cfn-certificate-validation-behaves.

## Evidence
Cause 1: acm.deploy-cfn-certificate-tags-propagate — botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the DescribeCertificate operation: 1 validation error detected: Value of the input at 'certificateArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)* — raised on acm.describe_certificate(CertificateArn=_wait_for_stack_resource_physical_id(ctx, stack_name, 'Certificate')).
Cause 2: acm.deploy-acme-endpoint-via-cloudformation-behaves — botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the DescribeAcmeEndpoint operation: 1 validation error detected: Value of the input at 'acmeEndpointArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+ (this check previously failed on the missing OutputValue recorded as Cause 2 of 20261022T143245Z-acm-deploy-and-cross-service-prerequisites-broken-describestackresource.md; the output now resolves, to a non-ARN).
Cause 3: acm.deploy-cfn-certificate-validation-behaves — AssertionError: expected the Certificate resource to still be CREATE_IN_PROGRESS (unvalidated) when we first observed its physical id, got status='CREATE_COMPLETE' reason=None, for template resource {Type: AWS::CertificateManager::Certificate, Properties: {DomainName: <unhosted domain>, ValidationMethod: DNS}}.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Causes 1/3: cfn.create_stack with a single AWS::CertificateManager::Certificate (DomainName=<a domain with no hosted zone>, ValidationMethod=DNS), then poll cfn.describe_stack_resource(StackName=<stack>, LogicalResourceId='Certificate').
3. Cause 3: the first response already carries ResourceStatus='CREATE_COMPLETE' instead of CREATE_IN_PROGRESS.
4. Cause 1: pass that PhysicalResourceId to acm.describe_certificate(CertificateArn=...) — 400 ValidationException on the certificateArn pattern, so the id is not an ACM ARN.
5. Cause 2: deploy a template with an ACM ACME-endpoint resource, read its physical id / Fn::GetAtt output and pass it to acm.describe_acme_endpoint(AcmeEndpointArn=...) — 400 ValidationException on the acmeEndpointArn pattern.
6. Re-replay the three checks in trace_ids.
