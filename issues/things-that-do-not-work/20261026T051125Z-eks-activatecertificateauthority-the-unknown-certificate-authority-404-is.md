Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.activate-certificate-authority-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.activate-certificate-authority-wrong-state-rejected|run#19|AssertionError|assert activate_code == (
Issue-Commit: cc0bbf947ad758ff480d27eaf731680ddd50e2b8

# EKS ActivateCertificateAuthority: the unknown-certificate-authority 404 is not worded as AWS words it ("... is not in the trusted list for cluster ...")

> _Note: this issue was identified against the code base at commit `cc0bbf947ad758ff480d27eaf731680ddd50e2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateCertificateAuthority

## Description
ActivateCertificateAuthority refuses a certificate-authority id the named cluster does not hold with the right error class and status, but with a message real AWS never emits.

This is ONE cause, exercised by the single failing check eks.activate-certificate-authority-wrong-state-rejected.

Cause 1 of 1: the ResourceNotFoundException message for an unknown/no-longer-trusted certificateAuthorityId uses the emulator's own generic child-resource wording instead of AWS's.

AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) — ActivateCertificateAuthority, POST /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}/activate, success 200; inputs clusterName and certificateAuthorityId are both required @httpLabel members; modeled errors InvalidParameterException (400), ResourceNotFoundException (404), ServerException (500), ServiceUnavailableException (503). The model documents ResourceNotFoundException only generically ("The specified resource could not be found...") and carries no message template, so the exact text is recorded behavior only: when the request names a certificateAuthorityId that is not in the cluster's trust bundle (e.g. one that has since been deleted), real AWS answers HTTP 404 / ResourceNotFoundException with a message of the form "Certificate authority <certificateAuthorityId> is not in the trusted list for cluster <clusterName>". Fixed text: "Certificate authority ", then " is not in the trusted list for cluster " — with NO trailing period. Variable parts: <certificateAuthorityId> taken verbatim from the request's certificateAuthorityId @httpLabel, and <clusterName> taken verbatim from the request's clusterName @httpLabel. No id, cluster name or other value from any one run is part of the requirement. (Note that the sibling DescribeCertificateAuthority not-found text is NOT asserted by this check, because AWS embeds a per-request id in it; only its code is graded. So this requirement is specific to ActivateCertificateAuthority's own refusal.)

Emulator: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs lines 230-243 resolves the CA and, when the cluster does not hold it, builds crate::state::child_resource_not_found(format!("No certificate authority found with id: {authority_id} for cluster: {cluster_name}.")). The replayed response was HTTP 404 / ResourceNotFoundException (class and status are CORRECT) with message "No certificate authority found with id: 0b5afc24-de6e-450b-afcd-f35138ab7a35 for cluster: sim2real-eks-cl-42654a063f61." — entirely different fixed text ("No certificate authority found with id: " / " for cluster: " plus a trailing period) from AWS's "Certificate authority " / " is not in the trusted list for cluster ".

Why: the error message is part of the error contract. A client, operator runbook or CA-rotation tool that matches, asserts on, or surfaces this refusal sees text real AWS never produces, and loses AWS's statement of WHY the activation was refused — AWS says the id is not in the cluster's TRUSTED LIST (a trust-bundle membership fact that distinguishes a deleted/untrusted successor CA from a malformed id), whereas the emulator only says the record was not found. Anything keyed on AWS's wording mis-handles the deleted-CA case.

Fix: the message construction in the certificate-authority-resolution branch of the ActivateCertificateAuthority handler (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs, the `let Some(authority) = cluster.certificate_authority(&authority_id) else` arm) — it must word the refusal as AWS words it, from the request's own two @httpLabel values. The error class, HTTP status and the payload's omission of clusterName are already right and must not change.

Relationship to already-filed work: the resolved issue 20261025T233637Z-eks-activatecertificateauthority-concurrency-gate-is-ordered-behind-the.md fixed this operation's in-flight-update gate ordering and its ResourceInUseException wording; those two assertions of this check now PASS and the check runs on to this later assertion, which is a distinct refusal path (CA resolution, not the concurrency/distribution gates) and is net-new. No open issue covers it: the open EKS certificate-authority issues are unimplemented-stub items for other operations.

Traces: eks.activate-certificate-authority-wrong-state-rejected.

## Evidence
Failing check: [mismatch] eks.activate-certificate-authority-wrong-state-rejected (assertion at check line 81)

AssertionError: expected ResourceNotFoundException, got ('ResourceNotFoundException', 'No certificate authority found with id: 0b5afc24-de6e-450b-afcd-f35138ab7a35 for cluster: sim2real-eks-cl-42654a063f61.')

Expected (real AWS), ActivateCertificateAuthority on a certificateAuthorityId the cluster no longer trusts:
  HTTP status: 404
  Code:        ResourceNotFoundException
  Message:     "Certificate authority <certificateAuthorityId> is not in the trusted list for cluster <clusterName>"
               fixed text: "Certificate authority " / " is not in the trusted list for cluster " (no trailing period);
               variable: <certificateAuthorityId> from the request's certificateAuthorityId @httpLabel,
                         <clusterName> from the request's clusterName @httpLabel.

Actual (emulator):
  HTTP status: 404                        (correct)
  Code:        ResourceNotFoundException  (correct)
  Message:     "No certificate authority found with id: <caId> for cluster: <clusterName>."
               fixed text: "No certificate authority found with id: " / " for cluster: " / "." — AWS emits none of this.

Earlier phases of the same check PASS, which localises the divergence: the immediate-activation refusal (ResourceInUseException, "Cannot CertificateAuthorityUpdate because cluster <cluster> currently has update <updateId> in progress"), the untouched signingStatus NOT_USED, the settling of the append update, DeleteCertificateAuthority and its update, the CA's disappearance from ListCertificateAuthorities, and the DescribeCertificateAuthority 404 code all matched.

Handler evidence (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/activate_certificate_authority.rs):
  - lines 230-243: `let Some(authority) = cluster.certificate_authority(&authority_id) else { ... crate::state::child_resource_not_found(format!("No certificate authority found with id: {authority_id} for cluster: {cluster_name}.")) ... }` — the diverging message.

Smithy evidence (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json):
  ActivateCertificateAuthority: POST /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}/activate, code 200;
  input clusterName (required, @httpLabel), certificateAuthorityId (required, @httpLabel), clientRequestToken (@idempotencyToken);
  errors InvalidParameterException(400), ResourceNotFoundException(404), ServerException(500), ServiceUnavailableException(503).
  The model states no message text, so the wording above is recorded behavior only.

## Steps to Reproduce
1. Create an EKS cluster and wait for status ACTIVE.
2. Call CreateCertificateAuthority(clusterName=<cluster>); keep certificateAuthority.id (caId) and update.id (appendUpdateId).
3. Wait for appendUpdateId to reach a terminal status via DescribeUpdate(name=<cluster>, updateId=appendUpdateId).
4. Call DeleteCertificateAuthority(clusterName=<cluster>, certificateAuthorityId=caId) and wait for its returned update to reach a terminal status. Confirm caId is absent from ListCertificateAuthorities(clusterName=<cluster>).
5. Call ActivateCertificateAuthority(clusterName=<cluster>, certificateAuthorityId=caId).
6. Observe the refusal. Real AWS: 404 ResourceNotFoundException with "Certificate authority <caId> is not in the trusted list for cluster <cluster>". Emulator: 404 ResourceNotFoundException with "No certificate authority found with id: <caId> for cluster: <cluster>."

Replay: run the behavioral check eks.activate-certificate-authority-wrong-state-rejected.
