Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.deploy-certificate-authority-cfn-issues-certificate acm-pca.deploy-cloudformation-ca-issues-certificate
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|CreateStack|ValidationError
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# CloudFormation rejects every AWS::ACMPCA::* resource type as unrecognized, so no private CA can be provisioned through a stack

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the emulator's CloudFormation resource-type registry does not know the acm-pca resource types, so a template that declares them is refused at template validation and the stack is never created.
AWS: recorded behavior only (resource types are not part of the acm-pca Smithy model): CreateStack accepts a template declaring AWS::ACMPCA::CertificateAuthority, AWS::ACMPCA::Certificate and AWS::ACMPCA::CertificateAuthorityActivation, returns a StackId, and the stack reaches CREATE_COMPLETE with a real, activated private CA whose ARN is readable from the stack resources/outputs and which can then sign a leaf through IssueCertificate/GetCertificate. These three are supported CloudFormation resource types for ACM Private CA.
Emulator: CreateStack is refused with HTTP 400 ValidationError, message of the form 'Template format error: Unrecognized resource types: [AWS::ACMPCA::CertificateAuthority]' (and, for the three-resource template, '[AWS::ACMPCA::CertificateAuthority, AWS::ACMPCA::Certificate, AWS::ACMPCA::CertificateAuthorityActivation]'). No stack is created, so everything downstream in the check (activation, IssueCertificate, GetCertificate) is unreachable.
Why: infrastructure-as-code is the normal way private CAs are created. Any user whose stack contains an ACM Private CA cannot deploy it at all against the emulator — the failure is at template validation, so it is not even a partial deployment, and CI that provisions its CA via CloudFormation before exercising acm-pca cannot run. The error class is also misleading: a supported resource type is reported as a template format error, which looks like a defect in the customer's template.
Fix: the CloudFormation resource-type registry / template-validation path in the emulator's cloudformation service, plus provisioning handlers for the three AWS::ACMPCA::* types that drive CreateCertificateAuthority, GetCertificateAuthorityCsr + IssueCertificate (template arn:aws:acm-pca:::template/RootCACertificate/V1) and ImportCertificateAuthorityCertificate, and expose the CA ARN as the resource's physical id.
Traces: acm-pca.deploy-certificate-authority-cfn-issues-certificate, acm-pca.deploy-cloudformation-ca-issues-certificate.

Note: this is distinct from the two open acm-pca issues, which concern the direct-SDK IssueCertificate/CSR lifecycle and CreateCertificateAuthority request validation; here the stack is refused before any acm-pca call is made.

## Evidence
acm-pca.deploy-certificate-authority-cfn-issues-certificate:
  expected: CreateStack accepts the template (Resources: Ca: AWS::ACMPCA::CertificateAuthority, Type ROOT, KeyAlgorithm RSA_2048, SigningAlgorithm SHA256WITHRSA) and the stack reaches CREATE_COMPLETE.
  actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ACMPCA::CertificateAuthority]

acm-pca.deploy-cloudformation-ca-issues-certificate:
  expected: CreateStack accepts a template with AWS::ACMPCA::CertificateAuthority + AWS::ACMPCA::Certificate + AWS::ACMPCA::CertificateAuthorityActivation, the stack reaches CREATE_COMPLETE, and the deployed CA can then sign a leaf via IssueCertificate/GetCertificate.
  actual: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ACMPCA::CertificateAuthority, AWS::ACMPCA::Certificate, AWS::ACMPCA::CertificateAuthorityActivation]

## Steps to Reproduce
1. Build a CloudFormation template declaring Resources with Type: AWS::ACMPCA::CertificateAuthority (Type ROOT, KeyAlgorithm RSA_2048, SigningAlgorithm SHA256WITHRSA, Subject.CommonName from a parameter); optionally add AWS::ACMPCA::Certificate (TemplateArn arn:aws:acm-pca:::template/RootCACertificate/V1) and AWS::ACMPCA::CertificateAuthorityActivation.
2. cloudformation.CreateStack(StackName=<unique>, TemplateBody=<that template>, Parameters=[...]).
3. Observe the 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ACMPCA::...]' instead of a StackId.
4. Expected continuation once accepted: wait for CREATE_COMPLETE, read the CA ARN from the stack resources, then IssueCertificate + GetCertificate against it.
