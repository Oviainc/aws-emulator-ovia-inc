Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CreateConnector accepts a CertificateAuthorityArn from a different Region than the request and never reports a nonexistent CA

_Filed: 2026-10-24T02:08:31Z_
_Operation: CreateConnector | Class: validation_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the private CA named by CertificateAuthorityArn must be usable from the Region the connector is created in — a CA ARN whose Region segment differs from the request's Region cannot be linked, and a CA ARN that names no certificate authority is rejected (CreateConnector models ResourceNotFoundException, whose doc is "The operation tried to access a nonexistent resource"; the CA is the only resource this operation accesses). Alternatively the real service accepts the create and settles the connector into Status=FAILED with a ConnectorStatusReason of PRIVATECA_RESOURCE_NOT_FOUND / PRIVATECA_INVALID_STATE / PRIVATECA_ACCESS_DENIED.

Actual: the handler performs no check on CertificateAuthorityArn beyond the model's own @pattern/@length: any pattern-valid acm-pca ARN — including one in another Region — is stored verbatim and the connector settles into ACTIVE. ResourceNotFoundException and the PRIVATECA_* / Status=FAILED paths are therefore unreachable for CreateConnector, so an emulator user never sees the error a wrong or unusable CA ARN produces against AWS.

## Evidence

Smithy model: CreateConnector declares ResourceNotFoundException (404) and its input `CertificateAuthorityArn` is @required with pattern ^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$ (OPERATIONS.md, smithy_query operation=CreateConnector); ConnectorStatusReason = [INTERNAL_FAILURE, PRIVATECA_ACCESS_DENIED, PRIVATECA_INVALID_STATE, PRIVATECA_RESOURCE_NOT_FOUND, VPC_ENDPOINT_RESOURCE_NOT_FOUND, VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND] — values only a CA/VPC-endpoint check can produce. Handler: src/ops/create_connector.rs lines 198-304 — the CA ARN goes straight into StoredConnector.certificate_authority_arn with status CREATING and a pending transition to ACTIVE; no Region comparison against crate::state::request_region(ctx) and no error path for the CA. No test exercises a cross-Region or unknown CA ARN (tests/all/connector_lifecycle.rs CA_ARNS are all in the request Region eu-west-2).

## Steps to Reproduce

1. Sign a CreateConnector request for Region us-east-1 but pass CertificateAuthorityArn=arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555.
2. Observe HTTP 202 and a connector ARN in us-east-1; after the transition span GetConnector reports Status=ACTIVE with that foreign-Region CA echoed back.
3. Expected: the mismatched/nonexistent CA is reported — either ResourceNotFoundException (404) at create time, or a connector that settles into Status=FAILED with StatusReason=PRIVATECA_RESOURCE_NOT_FOUND.
