Issue-Source: trace-sweep-divergence
Issue-Trace: acm-acm-error-paths
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-acm-error-paths acm-certificate-domain-validations-lifecycle acm-certificate-export-renew-eligibility-edge-cases acm-certificate-not-found-errors acm-certificate-operations-after-delete-error acm-export-certificate-pending-validation-error acm-operations-on-deleted-certificate-fail acm-renew-certificate-not-eligible-error acm-request-certificate-dns-lifecycle acm-request-certificate-email-validation-resend acm-request-public-certificate-dns-validation-lifecycle acm-acm-resource-tagging-error-paths acm-certificate-delete-then-operate-rejects acm-certificate-error-paths acm-delete-certificate-then-operate-on-deleted acm-operate-on-deleted-or-missing-certificate acm-list-tags-for-certificate-contract
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|DescribeCertificate|4xx->4xx|$.message
Issue-Key: acm|DeleteCertificate|4xx->4xx|$.message
Issue-Key: acm|AddTagsToCertificate|4xx->4xx|$.message
Issue-Key: acm|ListTagsForCertificate|4xx->4xx|$.message
Issue-Commit: 30db2a8818f1221c3278e244a1430bae2ca83b41

# ACM certificate-not-found message drops the trailing period

> _Note: this issue was identified against the code base at commit `30db2a8818f1221c3278e244a1430bae2ca83b41`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
Every ACM operation that resolves a certificate ARN renders ResourceNotFoundException's message one character short of AWS's: the terminal period of 'Could not find certificate <arn>.' is missing. The message is built from the request (it interpolates the requested ARN) and the recording shows the same template on four different operations across 30+ replayed steps, so the template - including its final period - is the rule; the particular ARN in each recording is only an example. HTTP 400 and the error type match everywhere. Four causes, one per operation reached.

Cause 1 of 4 - DescribeCertificate on a missing certificate.
AWS: recorded behavior only - Smithy ResourceNotFoundException (httpError 400, member message: String) documents no text. DescribeCertificate models ResourceNotFoundException; the recording returns 400 with message 'Could not find certificate <requested-arn>.'
Emulator: 400 with 'Could not find certificate <requested-arn>' (no trailing period).
Why: clients and tests that match AWS's exact not-found sentence (log scrapers, assertion suites, message-equality checks) fail against the emulator even though the status and error type are right.
Fix: the ResourceNotFoundException message construction in the ACM certificate-lookup path - append the period in the shared template.

Cause 2 of 4 - DeleteCertificate on a missing/already-deleted certificate: same template, same missing period (this cluster covers 15 replayed steps, including the post-delete error paths).
AWS: DeleteCertificate models ResourceNotFoundException (400); recorded message 'Could not find certificate <arn>.'
Emulator: same sentence without the period.
Why/Fix: as Cause 1 - same template reached from the delete path.

Cause 3 of 4 - AddTagsToCertificate on a missing certificate.
AWS: AddTagsToCertificate models ResourceNotFoundException (400); recorded message 'Could not find certificate arn:...:certificate/00000000-0000-0000-0000-000000000000.'
Emulator: same without the period.
Why/Fix: as Cause 1 - confirms the tagging path shares the defect.

Cause 4 of 4 - ListTagsForCertificate on a missing certificate.
AWS: ResourceNotFoundException (400) with the same recorded template.
Emulator: same without the period.
Why/Fix: as Cause 1.

Traces: acm-acm-error-paths#task#2 (representative; cluster covers 15 steps incl. acm-certificate-not-found-errors#task#1, acm-request-public-certificate-dns-validation-lifecycle#task#6), acm-acm-error-paths#task#3 (cluster covers 15 steps incl. acm-certificate-delete-then-operate-rejects#task#3), acm-certificate-not-found-errors#task#3, acm-list-tags-for-certificate-contract#task#5.

## Evidence
All four: HTTP 400 expected and returned; only $.message diverges, by the terminal period.
acm-acm-error-paths#task#2 (DescribeCertificate): expected 'Could not find certificate arn:aws:acm:us-west-2:000000000000:certificate/sim2real-probe-acm-nonexistent-10066c6d.', got the same string without the final '.'. Cluster affected_count 15.
acm-acm-error-paths#task#3 (DeleteCertificate): same expected/actual pair. Cluster affected_count 15.
acm-certificate-not-found-errors#task#3 (AddTagsToCertificate): expected 'Could not find certificate arn:aws:acm:us-west-2:000000000000:certificate/00000000-0000-0000-0000-000000000000.', got it without the final '.'.
acm-list-tags-for-certificate-contract#task#5 (ListTagsForCertificate): same expected/actual pair.
Smithy: ResourceNotFoundException - errorKind client, httpError 400 (defaulted), member message: String; bound as an error on DescribeCertificate, DeleteCertificate, AddTagsToCertificate, ListTagsForCertificate, ListTagsForResource, UntagResource.

## Steps to Reproduce
1. Replay the ACM wire corpus against the emulator.
2. Call DescribeCertificate, DeleteCertificate, AddTagsToCertificate and ListTagsForCertificate with a well-formed ACM certificate ARN that does not exist (or one already deleted).
3. Each returns HTTP 400 ResourceNotFoundException with 'Could not find certificate <arn>' while the recording has 'Could not find certificate <arn>.'
4. Named steps: acm-acm-error-paths#task#2, acm-acm-error-paths#task#3, acm-certificate-not-found-errors#task#3, acm-list-tags-for-certificate-contract#task#5.
