Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.certificate-authority-active-signer-deletion-rejected eks.certificate-authority-lifecycle-transitions eks.delete-certificate-authority-active-state-blocked
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.certificate-authority-active-signer-deletion-rejected|run#23|AssertionError|assert gone_msg.startswith(_CA_NOT_FOUND_HEAD) and gone_msg.endswith(_CA_NOT_FOUND_TAIL), gone_msg
Issue-Key: eks.certificate-authority-lifecycle-transitions|run#44|AssertionError|assert not_found_msg.startswith(
Issue-Key: eks.delete-certificate-authority-active-state-blocked|run#34|AssertionError|assert describe_dead_msg.startswith(
Issue-Commit: 33a45829f0914d4e090018a6741160dbd250b745

# eks: DescribeCertificateAuthority's deleted/unknown-CA 404 uses an emulator-authored sentence instead of EKS's wrapped not-found message (1 cause)

> _Note: this issue was identified against the code base at commit `33a45829f0914d4e090018a6741160dbd250b745`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificateAuthority

## Description
ONE net-new root cause in the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend), reached by three further behavioral checks that the suite report does NOT mark ALREADY FILED or OWNED.

Related open issue, named because it does not carry these three checks' replay keys: 20261103T041817Z-eks-describecertificateauthority-reports-an-unknown-deleted-certificate-authority.md describes the same DESCRIBE-side wording for the single check eks.certificate-authority-full-lifecycle-transitions (the only key in its Issue-Trace-Ids / Issue-Key). The three checks below exercise the same resolution path from three different CA lifecycles (sole-signer delete refusal + successor delete, full NOT_USED -> IN_USE rotation, and the active-state delete block), so a fixer must re-replay them as well to confirm the fix; they are recorded here for that purpose.

Cause 1 of 1 — the certificate-authority not-found refusal is the emulator's own interpolated sentence, not the message EKS returns.
AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): DescribeCertificateAuthority is GET /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}, both labels @required, and its only client error is ResourceNotFoundException with httpError 404. Recorded behavior: describing a certificateAuthorityId that no longer exists on the cluster — one whose DeleteCertificateAuthority Update reached Successful and which has already dropped out of ListCertificateAuthorities — is refused with HTTP 404, code ResourceNotFoundException, and a message whose FIXED text begins "certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: " and ends ") (SDK Attempt Count: 1)". The fixed parts are the lead-in sentence 'certificate authority not found' and the wrapped-service tail's fields 'Service: EksCertificateAuthority', 'Status Code: 404' and 'SDK Attempt Count: 1'. The ONLY variable part is the per-request request id inside the tail, which is not part of the requirement. Notably the message names neither the cluster nor the CA id — no value from the request appears in it.
Emulator: answers the right code (ResourceNotFoundException) but the message "No certificate authority found with id: <certificateAuthorityId> for cluster: <clusterName>." — a different sentence, with no wrapped-service tail, which additionally interpolates the request's CA id and cluster name into text the real service keeps constant.
Why: the message is the client-visible half of the refusal contract. A client, SDK retry/classification wrapper or operator runbook that matches EKS's wording, or that parses 'Service: EksCertificateAuthority' / 'Status Code: 404' out of the wrapped tail to decide the failure originated in the CA sub-service rather than the EKS frontend, matches nothing against the emulator; and the emulator leaks request detail into a message AWS deliberately keeps request-independent.
Fix: the not-found message composed by the certificate-authority id resolution shared by DescribeCertificateAuthority (and the other /clusters/{clusterName}/certificate-authorities readers that resolve an id) in the eks crate's ops/certificate-authority module — emit the fixed sentence plus the wrapped EksCertificateAuthority tail instead of an interpolated sentence naming the id and cluster.
Traces: eks.certificate-authority-active-signer-deletion-rejected, eks.certificate-authority-lifecycle-transitions, eks.delete-certificate-authority-active-state-blocked.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 175/342 passed; eks 166/306 (140 failed). None of the three checks below is marked ALREADY FILED or OWNED in the report.

1) eks.certificate-authority-active-signer-deletion-rejected — [mismatch] AssertionError at `assert gone_msg.startswith(_CA_NOT_FOUND_HEAD) and gone_msg.endswith(_CA_NOT_FOUND_TAIL)`; observed message: 'No certificate authority found with id: 7bc6b2e8-4aee-4283-a189-5b49d82e1e34 for cluster: sim2real-eks-clu-10b51043bc96.' Everything before it passed: the IN_USE sole signer's delete was refused ResourceInUseException with AWS's exact text, the CA stayed listed and IN_USE, a NOT_USED successor was created, its distribution reached COMPLETE, and its delete's Update reached Successful. Only the describe-side refusal text diverges.

2) eks.certificate-authority-lifecycle-transitions — [mismatch] AssertionError at `assert not_found_msg.startswith("certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: ") and not_found_msg.endswith(") (SDK Attempt Count: 1)")`; observed message: 'No certificate authority found with id: 958bbab9-a32e-4e40-a2ed-fd98c74a685b for cluster: sim2real-eks-cluster-2dea879a5bb2.' The preceding `assert not_found_code == "ResourceNotFoundException"` passed, as did the whole NOT_USED -> distribution COMPLETE -> IN_USE rotation and the retired CA's delete Update.

3) eks.delete-certificate-authority-active-state-blocked — [mismatch] AssertionError at `assert describe_dead_msg.startswith("certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: ") ...`; observed message: 'No certificate authority found with id: 7298ead1-8edd-491d-b531-b2338d53b359 for cluster: sim2real-eks-cadel-a2d17e7e424a.' The active-signer delete block (ResourceInUseException, exact text), the successor append/delete Updates and the post-delete listing all passed first.

The uuids, cluster names and request ids above are this run's values and are NOT part of the requirement; the requirement is the fixed sentence plus the wrapped EksCertificateAuthority/404/SDK-Attempt-Count tail, with only the request id varying.

Smithy model consulted: DescribeCertificateAuthority (GET /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}; errors ResourceNotFoundException 404, ServerException 500, ServiceUnavailableException 503).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, point a boto3 eks client at it.
2. Create an EKS cluster (role + two AZ-distinct subnets) and wait for status ACTIVE. list_certificate_authorities(clusterName=<cluster>) returns one CA with signingStatus IN_USE.
3. create_certificate_authority(clusterName=<cluster>) to append a NOT_USED successor; wait for the returned update to reach Successful and for the successor's distributionStatus to be COMPLETE.
4. delete_certificate_authority(clusterName=<cluster>, certificateAuthorityId=<successor id>) and wait for the returned update to reach Successful and the id to drop out of list_certificate_authorities.
5. describe_certificate_authority(clusterName=<cluster>, certificateAuthorityId=<the deleted id>) -> expect HTTP 404 ResourceNotFoundException whose message begins 'certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: ' and ends ') (SDK Attempt Count: 1)'; observe 'No certificate authority found with id: <id> for cluster: <cluster>.'
6. Re-replay eks.certificate-authority-active-signer-deletion-rejected, eks.certificate-authority-lifecycle-transitions and eks.delete-certificate-authority-active-state-blocked to confirm the fix.
