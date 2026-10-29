Issue-Source: trace-sweep-divergence
Issue-Trace: iam-cred-signing-cert-duplicate-across-users
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-cred-signing-cert-duplicate-across-users iam-server-certificate-lifecycle iam-server-certificate-tag-lifecycle iam-untag-server-certificate-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|UploadSigningCertificate|2xx->4xx|$.UploadSigningCertificateResponse
Issue-Key: iam|UploadSigningCertificate|2xx->4xx|status
Issue-Key: iam|ListServerCertificateTags|2xx->2xx|$.ListServerCertificateTagsResponse.ListServerCertificateTagsResult.Tags
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Signing certificate duplicate scope and server certificate tag persistence

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UploadSigningCertificate

## Description
Cause 1 of 2 - signing-certificate duplicate detection is scoped account-wide instead of per user. AWS: the duplicate check is per IAM user - uploading the same X.509 certificate body for a *different* user succeeds with HTTP 200 and returns UploadSigningCertificateResult/Certificate{UserName <that user>, CertificateId, CertificateBody, Status 'Active', UploadDate}; only re-uploading it for a user that already holds it is refused 409 (Smithy: UploadSigningCertificate models DuplicateCertificateException and EntityAlreadyExistsException distinctly). Emulator: the second user's upload is refused HTTP 409, so the certificate can only ever be held by one user in the account. Why: a client that shares one certificate across two users - the configuration AWS allows - gets a hard conflict and cannot provision the second user at all; the 409 also makes the account look as though the certificate is already registered where it is not. Fix: key the signing-certificate duplicate check by (UserName, certificate body) in the UploadSigningCertificate handler rather than by certificate body alone.
Traces: iam-cred-signing-cert-duplicate-across-users#task#4.

Cause 2 of 2 - server-certificate tags are not readable back. AWS: TagServerCertificate stores the tags on the certificate and ListServerCertificateTags returns them as Tags/member{Key, Value} (one member per tag, sorted by key). Emulator: ListServerCertificateTags serialises Tags as an empty element ('') after a successful tag write, on the tag fixture and on both untag fixtures - the tags are either never persisted or never read back. Why: a client that tags a server certificate and then reads the tags back to confirm, or that drives untag/retag from the current tag set, sees an untagged certificate; cost-allocation and ownership tagging on server certificates is silently a no-op. Fix: persist tags in the server-certificate state written by TagServerCertificate and read them in the ListServerCertificateTags handler.
Traces: iam-server-certificate-lifecycle#task#6.

Note: open issue 20261027T000203Z-iam-tagservercertificate-stub covers TagServerCertificate as an unimplemented scaffold stub; these replayed steps show the tag write answering 200 and the *read* path returning nothing, and the issue carries no replay key for this fixture step.

## Evidence
Cause 1: expected (recorded AWS) HTTP 200 with UploadSigningCertificateResult/Certificate{UploadDate, CertificateId, UserName <second user>, Status 'Active', CertificateBody <the PEM>}; actual HTTP 409 with no UploadSigningCertificateResponse (`$.UploadSigningCertificateResponse: missing (expected {...}, got None)`).
Cause 2: expected ListServerCertificateTagsResult/Tags/member{Key, Value} for the tag just written; actual Tags = '' (type_mismatch), reproduced on iam-server-certificate-lifecycle#task#6, iam-server-certificate-tag-lifecycle#task#3 and iam-untag-server-certificate-lifecycle#task#3 and #task#5.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-cred-signing-cert-duplicate-across-users: task#3 uploads a certificate for user A (200), task#4 uploads the same certificate body for user B -> 409 instead of 200.
3. Replay iam-server-certificate-lifecycle: upload a server certificate, TagServerCertificate, then task#6 ListServerCertificateTags -> empty Tags instead of the written tag.
