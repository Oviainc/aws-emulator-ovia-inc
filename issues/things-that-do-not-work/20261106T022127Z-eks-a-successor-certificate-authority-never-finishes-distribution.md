Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.certificate-authority-delete-while-active-blocked
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.certificate-authority-delete-while-active-blocked|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# eks: a successor certificate authority never finishes distribution, so CA rotation (and the delete-while-active guard behind it) is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCertificateAuthority

## Description
Cause 1 of 1: a certificate authority appended with CreateCertificateAuthority never reaches distributionStatus COMPLETE and the CertificateAuthorityUpdate it returns never reaches a terminal status, so a rotation can never be performed and everything downstream of a rotation is unreachable.

  AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): CreateCertificateAuthority (POST /clusters/{clusterName}/certificate-authorities) returns {update: Update, certificateAuthority: CertificateAuthoritySummary}; CertificateAuthority.distributionStatus targets CertificateAuthorityDistributionStatus with values IN_PROGRESS | COMPLETE | FAILED | DELETING and its documentation states "A successor CA can only be activated after its distribution status is COMPLETE"; signingStatus targets CertificateAuthoritySigningStatus with values NOT_USED | ACTIVATING | IN_USE. So on AWS the lifecycle settles: the appended successor starts NOT_USED with distributionStatus IN_PROGRESS and progresses to COMPLETE; ActivateCertificateAuthority (POST .../{certificateAuthorityId}/activate) then returns an Update whose DescribeUpdate status reaches Successful, after which the successor reads signingStatus IN_USE / distributionStatus COMPLETE and the outgoing CA reads NOT_USED. Only then are the two refusals this check asserts observable: DeleteCertificateAuthority on the IN_USE signer answers 409 ResourceInUseException (modeled httpError 409) with the message "Cannot delete the active certificate authority. Activate a different CA first." (recorded behavior; the whole sentence is fixed text with no variable part), while DeleteCertificateAuthority on the retired NOT_USED CA succeeds and a later DescribeCertificateAuthority of that id answers 404 ResourceNotFoundException and ActivateCertificateAuthority of that id answers 404 ResourceNotFoundException with "Certificate authority <certificateAuthorityId> is not in the trusted list for cluster <clusterName>" — fixed text with the CA id and cluster name taken from the request.
  Emulator: the check reaches no assertion at all. It is recorded as "Error: Check timed out after 60.0s": it burns the entire suite budget inside its own polls, whose deadlines are minutes long (DescribeCertificateAuthority polled for distributionStatus == COMPLETE with timeout_s=1800 / interval_s=15, then ActivateCertificateAuthority retried for up to 900s, then DescribeUpdate polled for a terminal status for up to 900s). Nothing in the check expects a 60-second settle, so the budget is only the bound on the symptom. Hypothesis: the emulator's CA records are created with a distributionStatus that never advances to COMPLETE and the CertificateAuthorityUpdate it records never advances to Successful, so the successor is never activatable and no CA ever becomes a second IN_USE signer.
  Why: CA rotation is exactly create-successor -> wait for COMPLETE -> activate -> retire the old CA. A client doing that against the emulator blocks forever on the documented precondition, and because no rotation can complete, the emulator's behavior for the post-rotation states is never exercised: deleting the active signer, deleting a retired NOT_USED CA, and reading/mutating a deleted CA id are all unreachable through the public API.
  Fix: the certificate-authority lifecycle/state machinery in the eks service crate — the handlers for CreateCertificateAuthority and ActivateCertificateAuthority and the CA/update records in the service's state must advance distributionStatus IN_PROGRESS -> COMPLETE and advance the associated Update to Successful, flipping signingStatus of the successor to IN_USE and of the outgoing CA to NOT_USED.
  Traces: eks.certificate-authority-delete-while-active-blocked.

Relation to already-open issues: 20261104T105252Z-eks-2-net-new-root-causes-the-metrics.md (Cause 2, "asynchronous EKS resources never reach a terminal state") and 20261029T175608Z-eks-3-net-new-root-causes-startinsightsrefresh-response.md describe this same family for other operations and other checks, but neither carries this check's replay key, so the suite reports it unowned and it is filed here. Not counted as a cause: eks.delete-certificate-authority-in-use-blocked fails in the same area but its failure is a runner artifact, not an emulator response — its poll deadline is computed as max(0, min(1800, ctx.time_left() - 60)), which collapses to 0.0s under the suite's 60-second per-check budget, so it raises "condition not met within 0.0s; last observed=None" without ever calling the emulator. Usefully, every emulator call that check DID make before that point passed, including DeleteCertificateAuthority on the fresh cluster's sole IN_USE CA answering ResourceInUseException with the exact AWS sentence and CreateCertificateAuthority returning a successor with signingStatus NOT_USED — which localizes this issue to the distribution/update progression rather than to the refusal wording.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (eks 166/306 passed).

Failing check eks.certificate-authority-delete-while-active-blocked — the suite records exactly:
  Error: Check timed out after 60.0s
and no assertion failure, i.e. the check never got past its lifecycle polls:
  _poll_until(lambda: eks.describe_certificate_authority(clusterName=cluster, certificateAuthorityId=successor_id)["certificateAuthority"], lambda ca: ca["distributionStatus"] == "COMPLETE", timeout_s=1800, interval_s=15)
  _retry_activation(eks, cluster, successor_id, timeout_s=900, interval_s=10)
  _poll_until(lambda: eks.describe_update(name=cluster, updateId=activate_resp["update"]["id"])["update"], lambda u: u["status"] in ("Successful", "Failed", "Cancelled"), timeout_s=900, interval_s=15)

Expected (AWS), after the successor reaches distributionStatus COMPLETE and its activation update reaches Successful:
  describe_certificate_authority(successor).signingStatus == "IN_USE", distributionStatus == "COMPLETE"
  delete_certificate_authority(successor) -> 409 ResourceInUseException, Message exactly "Cannot delete the active certificate authority. Activate a different CA first."
  delete_certificate_authority(retired NOT_USED CA) -> 200, the id leaves ListCertificateAuthorities, then DescribeCertificateAuthority(id) -> 404 ResourceNotFoundException and ActivateCertificateAuthority(id) -> 404 ResourceNotFoundException with "Certificate authority <id> is not in the trusted list for cluster <cluster>".
Actual (emulator): none of these states is ever reached — the successor's distributionStatus does not become COMPLETE within the whole budget, so no call after CreateCertificateAuthority is ever evaluated.

Corroborating, from eks.delete-certificate-authority-in-use-blocked on the same surface: the emulator DOES answer DeleteCertificateAuthority on a fresh cluster's sole IN_USE CA with ResourceInUseException and the exact AWS sentence, and CreateCertificateAuthority DOES return a successor with signingStatus NOT_USED plus an update id — the divergence is that the successor's distribution/update never progresses. (That check's own failure, "condition not met within 0.0s; last observed=None", is a collapsed runner deadline and is not part of this issue.)

Smithy model consulted: CertificateAuthorityDistributionStatus = [IN_PROGRESS, COMPLETE, FAILED, DELETING]; CertificateAuthoritySigningStatus = [NOT_USED, ACTIVATING, IN_USE]; DeleteCertificateAuthority errors include ResourceInUseException (409) and ResourceNotFoundException (404); ActivateCertificateAuthority errors include ResourceNotFoundException (404).

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run eks.certificate-authority-delete-while-active-blocked.
3. Manually, against the emulator's EKS endpoint:
   a. CreateCluster with two AZ-distinct subnets and wait for status ACTIVE.
   b. ListCertificateAuthorities(clusterName) -> one CA with signingStatus IN_USE.
   c. CreateCertificateAuthority(clusterName) -> note certificateAuthority.id (expect signingStatus NOT_USED) and update.id.
   d. Poll DescribeCertificateAuthority(clusterName, successor id) -> expect distributionStatus to reach COMPLETE. It stays IN_PROGRESS; also poll DescribeUpdate(name=cluster, updateId=update.id) -> expect Successful.
   e. Once COMPLETE, ActivateCertificateAuthority(clusterName, successor id), poll its update to Successful, then expect successor signingStatus IN_USE.
   f. DeleteCertificateAuthority(clusterName, successor id) -> expect 409 ResourceInUseException "Cannot delete the active certificate authority. Activate a different CA first." and the CA still IN_USE; DeleteCertificateAuthority on the retired CA -> expect success, then DescribeCertificateAuthority / ActivateCertificateAuthority on that id -> expect 404 ResourceNotFoundException. Steps e-f are unreachable today because (d) never completes.
