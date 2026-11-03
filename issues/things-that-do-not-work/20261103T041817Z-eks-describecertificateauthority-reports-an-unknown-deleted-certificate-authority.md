Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.certificate-authority-full-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.certificate-authority-full-lifecycle-transitions|run#28|AssertionError|assert message.startswith(
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# eks: DescribeCertificateAuthority reports an unknown/deleted certificate authority with its own sentence instead of EKS's wrapped not-found message

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificateAuthority

## Description
ONE net-new root cause in the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend): the certificate-authority resolution path composes its own not-found sentence instead of the message EKS returns, so the error code is right and the text is a sentence real EKS never emits.

Related open issue, named because it does not cover this message: 20261029T175608Z-eks-8-net-new-root-causes-in-certificate.md carries the CA refusal causes for ActivateCertificateAuthority, the per-cluster CA cap and DeleteCertificateAuthority's in-use guard; none of its eight causes is the DESCRIBE-side not-found wording below.

Cause 1 of 1 — the not-found refusal for a certificate authority is the emulator's own sentence, not EKS's wrapped message.
AWS: Smithy: DescribeCertificateAuthority is GET /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId} and models ResourceNotFoundException (httpError 404) as its only client error. Recorded behavior: describing a certificateAuthorityId that no longer exists on the cluster (here, one deleted by a DeleteCertificateAuthority whose Update went Successful) is refused with code ResourceNotFoundException and a message that BEGINS with the fixed text "certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: " and ENDS with the fixed text ") (SDK Attempt Count: 1)". The fixed parts are the lead-in sentence 'certificate authority not found' and the wrapped-service tail's fields 'Service: EksCertificateAuthority', 'Status Code: 404' and 'SDK Attempt Count: 1'; the only variable part is the per-run request id inside the tail, which is NOT part of the requirement. Note the message names neither the cluster nor the CA id.
Emulator: answers ResourceNotFoundException (correct code) with 'No certificate authority found with id: <certificateAuthorityId> for cluster: <clusterName>.' — a different sentence, with no wrapped-service tail, that also interpolates the request's CA id and cluster name into text AWS keeps constant.
Why: the message is the user- and test-visible half of the contract. A client (or operator runbook) that matches EKS's wording, or that parses the originating service / status out of the wrapped tail to decide whether the failure came from the CA sub-service, matches nothing; and the emulator discloses request detail in a message the real service deliberately keeps fixed.
Fix: the not-found message composed by the certificate-authority resolution shared by DescribeCertificateAuthority (and the other /certificate-authorities readers that resolve an id) in the eks crate's ops module — emit the fixed sentence plus the wrapped EksCertificateAuthority tail rather than an interpolated sentence.
Traces: eks.certificate-authority-full-lifecycle-transitions.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 80/342 passed; eks 73/306. The check below is not marked ALREADY FILED or OWNED in the report.

eks.certificate-authority-full-lifecycle-transitions — [mismatch] AssertionError at `assert message.startswith("certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: ") and message.endswith(") (SDK Attempt Count: 1)")`; the message observed was 'No certificate authority found with id: 220f2c00-4703-44b3-b4aa-29be08130a78 for cluster: sim2real-eks-9377c3187078.' (ids/names shown are this run's values and are not part of the requirement). The preceding `assert code == "ResourceNotFoundException"` passed, and so did the whole lifecycle leading up to it (successor CA created NOT_USED, distribution COMPLETE, activation making it IN_USE and retiring the original, DeleteCertificateAuthority's Update reaching Successful), so only the refusal text diverges.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — DescribeCertificateAuthority (GET /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}, errors ResourceNotFoundException 404 / ServerException 500 / ServiceUnavailableException 503), ResourceNotFoundException (httpError 404, members clusterName/nodegroupName/fargateProfileName/addonName/subscriptionId/message).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, point a boto3 eks client at it.
2. Create a cluster and wait for it to be ACTIVE; list_certificate_authorities(clusterName=<cluster>) and note the single IN_USE CA id.
3. create_certificate_authority(clusterName=<cluster>), wait for the successor's distribution to be COMPLETE, activate_certificate_authority on it, then delete_certificate_authority(clusterName=<cluster>, certificateAuthorityId=<the retired original>) and wait for the returned update to reach Successful.
4. describe_certificate_authority(clusterName=<cluster>, certificateAuthorityId=<the deleted id>) -> expect 404 ResourceNotFoundException whose message begins 'certificate authority not found (Service: EksCertificateAuthority, Status Code: 404, Request ID: ' and ends ') (SDK Attempt Count: 1)'; observe 'No certificate authority found with id: <id> for cluster: <cluster>.'
5. Re-replay eks.certificate-authority-full-lifecycle-transitions to confirm.
