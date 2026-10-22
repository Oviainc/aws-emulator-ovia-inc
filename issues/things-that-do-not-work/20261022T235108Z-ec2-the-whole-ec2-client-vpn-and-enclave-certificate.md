Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.e2e-client-vpn-route-flow ec2.associated-enclave-certificate-iam-roles-reflects-association ec2.op-contract-acceptTransitGatewayClientVpnAttachment-b ec2.op-contract-associate-enclave-certificate-iam-role ec2.op-contract-authorizeClientVpnIngress ec2.op-contract-deleteClientVpnEndpoint ec2.op-contract-describe-client-vpn-authorization-rules ec2.op-contract-describe-client-vpn-connections ec2.op-contract-disassociate-client-vpn-target-network ec2.op-contract-disassociate-enclave-certificate-iam-role ec2.op-contract-export-client-vpn-client-certificate-revocation-list ec2.op-contract-export-client-vpn-client-configuration ec2.op-contract-get-associated-enclave-certificate-iam-roles ec2.op-contract-import-client-vpn-client-certificate-revocation-list ec2.op-contract-modify-client-vpn-endpoint ec2.op-contract-revoke-client-vpn-ingress ec2.op-contract-terminate-client-vpn-connections ec2.op-contract-describe-client-vpn-routes ec2.op-contract-describe-client-vpn-target-networks
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ImportCertificate|NotImplemented
Issue-Key: ec2.op-contract-describe-client-vpn-routes|run#4|AssertionError|assert dry_code == "DryRunOperation", f"expected DryRunOperation, got {dry_code}"
Issue-Key: ec2.op-contract-describe-client-vpn-target-networks|run#4|AssertionError|assert dry_code == "DryRunOperation", f"expected DryRunOperation, got {dry_code}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# The whole EC2 Client VPN and enclave-certificate surface is unreachable: ACM ImportCertificate is a 501 stub and DescribeClientVpnRoutes/TargetNetworks answer NotImplemented (3 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCertificate

## Description
Three root causes; all three must be closed before submitting.

Cause 1 of 3: ACM ImportCertificate answers 501 NotImplemented, which blocks 17 EC2-led checks at setup.
AWS: acm:ImportCertificate accepts a PEM Certificate/PrivateKey (+CertificateChain) and returns 200 with CertificateArn (Smithy com-amazonaws-acm-certificatemanager: ImportCertificate -> ImportCertificateResponse.CertificateArn; errors include LimitExceededException, InvalidTagException, ResourceNotFoundException). Every Client VPN endpoint and every enclave-certificate association needs a server certificate ARN first.
Emulator: ImportCertificate answers NotImplemented ('operation ImportCertificate is scaffolded but not implemented in this emulator'), so all 17 checks abort before any EC2 assertion runs.
Why: Client VPN and Nitro-enclave certificate workflows cannot be modelled at all — not their happy path and not one of their refusal codes.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/import_certificate.rs.
Traces: ec2.e2e-client-vpn-route-flow; ec2.associated-enclave-certificate-iam-roles-reflects-association; ec2.op-contract-acceptTransitGatewayClientVpnAttachment-b; ec2.op-contract-associate-enclave-certificate-iam-role; ec2.op-contract-authorizeClientVpnIngress; ec2.op-contract-deleteClientVpnEndpoint; ec2.op-contract-describe-client-vpn-authorization-rules; ec2.op-contract-describe-client-vpn-connections; ec2.op-contract-disassociate-client-vpn-target-network; ec2.op-contract-disassociate-enclave-certificate-iam-role; ec2.op-contract-export-client-vpn-client-certificate-revocation-list; ec2.op-contract-export-client-vpn-client-configuration; ec2.op-contract-get-associated-enclave-certificate-iam-roles; ec2.op-contract-import-client-vpn-client-certificate-revocation-list; ec2.op-contract-modify-client-vpn-endpoint; ec2.op-contract-revoke-client-vpn-ingress; ec2.op-contract-terminate-client-vpn-connections.

Cause 2 of 3: EC2 DescribeClientVpnRoutes is an unimplemented stub.
AWS: with DryRun=true and permission it refuses 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.' (Smithy com-amazonaws-ec2-amazonec2: DryRun: Boolean, errors: [] — EC2 codes are ec2Query wire codes).
Emulator: NotImplemented.
Why: the permission probe and the endpoint-not-found boundary are both unobservable.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_client_vpn_routes.rs.
Traces: ec2.op-contract-describe-client-vpn-routes.

Cause 3 of 3: EC2 DescribeClientVpnTargetNetworks is an unimplemented stub.
AWS: as cause 2 — authorized DryRun=true gives DryRunOperation.
Emulator: NotImplemented.
Why: same as cause 2.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_client_vpn_target_networks.rs.
Traces: ec2.op-contract-describe-client-vpn-target-networks.

## Evidence
Cause 1 — actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportCertificate operation: operation ImportCertificate is scaffolded but not implemented in this emulator (17 checks abort here). Expected: HTTP 200 with CertificateArn.
Causes 2-3 — actual: AssertionError: expected DryRunOperation, got NotImplemented. Expected: HTTP 400 DryRunOperation with message 'Request would have succeeded, but DryRun flag is set.'.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) acm:ImportCertificate with a self-signed PEM cert+key. (2) ec2:DescribeClientVpnRoutes with ClientVpnEndpointId='cvpn-endpoint-00000000000000000', DryRun=True. (3) ec2:DescribeClientVpnTargetNetworks likewise.
