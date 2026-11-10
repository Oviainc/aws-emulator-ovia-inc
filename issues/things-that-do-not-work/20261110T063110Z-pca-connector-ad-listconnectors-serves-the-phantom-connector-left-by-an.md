Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-connector-lifecycle-missing-prerequisites
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-connector-lifecycle-missing-prerequisites
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|ListConnectors|2xx->2xx|$.Connectors[*].CertificateEnrollmentPolicyServerEndpoint
Issue-Related: 20261029T224504Z-pca-connector-ad-createconnector-skips-the-directoryregistration-prerequisite-and-persists-a.md
Issue-Commit: 2fcb12c3578baaa46d109a4b25748978aaf341c1

# ListConnectors serves the phantom connector left by an unverified CreateConnector

> _Note: this issue was identified against the code base at commit `2fcb12c3578baaa46d109a4b25748978aaf341c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConnectors

## Description
One net-new wire cause: the connector read path enumerates a connector that AWS never created, and describes it with an enrollment endpoint on a host the emulator does not serve.

Cause 1 of 1: ListConnectors returns a Connectors entry for a connector whose creation AWS refuses.
AWS: ListConnectors is GET /connectors returning HTTP 200 with `Connectors` -> ConnectorList (of ConnectorSummary) and an optional `NextToken` (Smithy: ListConnectors http GET /connectors, code 200, paginated items=Connectors; output member Connectors -> ConnectorList -> ConnectorSummary with members Arn, CertificateAuthorityArn, CertificateEnrollmentPolicyServerEndpoint, DirectoryId, VpcInformation, Status -> ConnectorStatus, StatusReason, CreatedAt, UpdatedAt). The list is a projection of the account/region's connector state: it contains exactly the connectors that CreateConnector actually created. CreateConnector models ResourceNotFoundException @httpError 404, so when its DirectoryId has no DirectoryRegistration the create is refused and nothing is persisted; the subsequent ListConnectors therefore returns `Connectors` with no entry for that directory (recorded behavior only for the empty list; the refusal status and error class are the model's). The general rule: a resource that a refused create never made must not appear in any read path.
Emulator: answers HTTP 200 whose `Connectors` list contains one extra ConnectorSummary — the resource minted by the CreateConnector call that should have been refused — carrying Arn (a pca-connector-ad connector ARN), CertificateAuthorityArn (the CA ARN from the create request), CertificateEnrollmentPolicyServerEndpoint, DirectoryId (the unregistered directory id from the create request), VpcInformation.SecurityGroupIds (from the create request), Status 'CREATING', and CreatedAt/UpdatedAt. The replay additionally flags CertificateEnrollmentPolicyServerEndpoint as announcing a host that is not the emulator's own: the emulator serves this API on its own listener but reports an enrollment endpoint of the form https://<connector-uuid>.pca-connector-ad.<region>.amazonaws.com/ADPolicyProvider_CEP_Kerberos/service.svc/CEP, i.e. a real-AWS hostname no client of the emulator can reach. (The ARN uuid, region, account, security-group id and timestamps are per-run values, not requirements.)
Why: a client that enumerates connectors to decide what exists sees a connector AWS does not have, and then drives templates, service principal names and teardown against it; a test asserting "no connector exists for an unregistered directory" passes here and fails against AWS. Separately, a client that follows CertificateEnrollmentPolicyServerEndpoint is sent off the emulator to a hostname it does not serve, so enrollment flows cannot be exercised locally. Hypothesis: the endpoint string is templated from the real AWS DNS pattern rather than from the emulator's own announced address.
Fix: the connector state store and the create gate it is written by (services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/state.rs, services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_connector.rs) — once CreateConnector resolves DirectoryId against the directory-registration store and refuses, nothing is inserted and this read path agrees; the ListConnectors serializer (src/ops/list_connectors.rs) must additionally build CertificateEnrollmentPolicyServerEndpoint from the emulator's own announced endpoint rather than an amazonaws.com host.
Traces: fixture pca-connector-ad-connector-lifecycle-missing-prerequisites, step task#2.

Related open issue (does NOT carry this step's replay key, so it does not cover it): 20261029T224504Z-pca-connector-ad-createconnector-skips-the-directoryregistration-prerequisite-and-persists-a.md is filed on CreateConnector with Issue-Step task#1 and discusses the leak in prose, but its replay pointer names only task#1 and the fixture id, so a fixer replaying it never exercises this ListConnectors step and never sees the enrollment-endpoint host defect at all.

## Evidence
pca-connector-ad-connector-lifecycle-missing-prerequisites#task#2 (ListConnectors, after CreateConnector against a DirectoryId with no DirectoryRegistration):
  status: 200 == 200 (expected 200)
  $.Connectors[0]: unexpected_present — expected NO entry, got {'Arn': 'arn:aws:pca-connector-ad:us-west-2:000000000000:connector/95110974-9373-4c60-85e1-4792002a9463', 'CertificateAuthorityArn': 'arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-1111-1111-1111-111111111111', 'CertificateEnrollmentPolicyServerEndpoint': 'https://95110974-9373-4c60-85e1-4792002a9463.pca-connector-ad.us-west-2.amazonaws.com/ADPolicyProvider_CEP_Kerberos/service.svc/CEP', 'DirectoryId': 'd-9067abcd12', 'VpcInformation': {'SecurityGroupIds': ['sg-0123456789abcdef0']}, 'Status': 'CREATING', 'CreatedAt': <creation time>, 'UpdatedAt': <creation time>}
  $.Connectors[0].CertificateEnrollmentPolicyServerEndpoint: host_not_own — the emulator announced 'https://<connector-uuid>.pca-connector-ad.<region>.amazonaws.com/ADPolicyProvider_CEP_Kerberos/service.svc/CEP', a host other than the one it is reached on.
  (The uuid, CA ARN, account 000000000000, region us-west-2, directory id d-9067abcd12, security-group id and timestamps above are the recording's values filling the template; the requirement is that the list carries no entry at all for a connector whose create was refused, and that any endpoint it does report resolves to the emulator's own address.)

The prior step in the same fixture (task#1, CreateConnector) answered 202 instead of the modeled 404 ResourceNotFoundException; that step is owned by an existing issue, this one is not.

## Steps to Reproduce
Boot the emulator and replay simparity-wire-aws-v1 (fixture pca-connector-ad-connector-lifecycle-missing-prerequisites), or drive it directly in region us-west-2:
1. Register no directory. Call CreateConnector(DirectoryId=<a pattern-valid id matching ^d-[0-9a-f]{10}$ with no DirectoryRegistration>, CertificateAuthorityArn=<pattern-valid acm-pca CA ARN>, VpcInformation={'SecurityGroupIds': ['sg-0123456789abcdef0']}). The emulator answers 202 with a ConnectorArn where AWS answers 404 ResourceNotFoundException and creates nothing.
2. Call ListConnectors(). The emulator returns 200 with one Connectors entry for that unregistered DirectoryId; on AWS the Connectors list has no such entry. (pca-connector-ad-connector-lifecycle-missing-prerequisites#task#2)
3. Inspect that entry's CertificateEnrollmentPolicyServerEndpoint: it names <uuid>.pca-connector-ad.<region>.amazonaws.com, a host the emulator does not serve, instead of its own announced endpoint.
