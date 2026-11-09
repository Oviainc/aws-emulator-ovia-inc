Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca__cloudwatch.issue-certificate-success-and-failure-counted-in-acmprivateca-namespace
Issue-Key: acm-pca__cloudwatch.issue-certificate-success-and-failure-counted-in-acmprivateca-namespace|_poll_until#18|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: cda81fc610349487f102bcc6f8ad570fe265d90b

# acm-pca__cloudwatch: IssueCertificate publishes no AWS/ACMPrivateCA metrics — ListMetrics reports no Success/Failure/Time series for a CA

> _Note: this issue was identified against the code base at commit `cda81fc610349487f102bcc6f8ad570fe265d90b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IssueCertificate

## Description
Cause 1 of 1: ACM Private CA never publishes its per-CA CloudWatch metrics, so the AWS/ACMPrivateCA namespace is empty for every certificate authority. Failing check: acm-pca__cloudwatch.issue-certificate-success-and-failure-counted-in-acmprivateca-namespace.

AWS: ACM Private CA emits metrics into the AWS/ACMPrivateCA namespace (documented in the privateca userguide, PcaCloudWatch.html): Success (the number of certificates ACM Private CA issued, applying only to IssueCertificate), Failure (the number of failed issuance attempts) and Time (the time in milliseconds between an issuance request and the completion or failure of issuance). Each series is dimensioned on the certificate authority it belongs to, so cloudwatch:ListMetrics(Namespace='AWS/ACMPrivateCA', MetricName='Success') reports at least one dimension set whose value names the CA (its id/ARN), and GetMetricStatistics over that dimension set returns datapoints whose Sum equals the number of issuances that actually succeeded against that CA. Metric emission is not expressed in the Smithy model for either service — these are CloudWatch-side series, so this is documented/recorded behavior only, not a modeled shape.

Emulator: the CA was created, activated as a ROOT CA by signing its own CSR, reported Status=ACTIVE, successfully issued a 1-day end-entity certificate, correctly refused a 7-day leaf with ValidationException ('The certificate validity specified exceeds the certificate authority validity.'), then issued a second 1-day leaf successfully. Despite three real issuance attempts (2 succeeded, 1 refused), ListMetrics(Namespace='AWS/ACMPrivateCA', MetricName='Success') returns an empty Metrics list for the whole 600 s poll window: the check fails with `AssertionError: ListMetrics reporting a AWS/ACMPrivateCA Success series dimensioned on this run's CA <ca-id> not met within 600.0s. Last result: []`. Because the series never appears, the dependent assertions were never reached: Success == 2 for the run's own window, the absence of a Failure series for a synchronous validation refusal, and Time being a positive latency bounded by the measured elapsed time. (The CA id in the recorded message is a per-run server-minted value and is not part of the requirement; the requirement is that SOME dimension set naming the CA under test exists.)

Why: a client that monitors private-CA issuance — an alarm on AWS/ACMPrivateCA Failure, a dashboard of Success volume per CA, or an SLO on issuance Time — reads an empty metric list and zero datapoints against the emulator no matter how many certificates are issued or refused. Alarms built on these series sit in INSUFFICIENT_DATA forever, and any test that distinguishes 'issuance succeeded' from 'issuance was refused' via metrics (the documented way to tell a synchronous refusal, which emits no Failure series, from an asynchronous signing failure, which does) cannot be written at all.

Fix: belongs in the ACM PCA IssueCertificate handler's metric emission path and the CloudWatch metric store it must write into — the handler should publish, per certificate authority, a Success datapoint on each completed issuance, a Failure datapoint on an issuance that fails during signing (and NOT for a synchronous validation refusal), and a Time datapoint carrying the issuance latency in milliseconds, dimensioned so the CA is identifiable, so that cloudwatch ListMetrics/GetMetricStatistics can read them back.

Traces: acm-pca__cloudwatch.issue-certificate-success-and-failure-counted-in-acmprivateca-namespace.

## Evidence
Expected (AWS): after two successful and one refused IssueCertificate against one CA, cloudwatch.list_metrics(Namespace='AWS/ACMPrivateCA', MetricName='Success') reports a dimension set identifying that CA; get_metric_statistics(..., MetricName='Success', Dimensions=<that set>, Statistics=['Sum']) sums to exactly 2 over the run's window; MetricName='Failure' has no series naming the CA (None, not 0.0) because the 7-day leaf was refused synchronously; MetricName='Time' reports a positive Average bounded by the wall-clock time spent on the issuances.

Actual (emulator): ListMetrics returns [] for Namespace='AWS/ACMPrivateCA', MetricName='Success' for the entire 600 s poll.
  AssertionError: ListMetrics reporting a AWS/ACMPrivateCA Success series dimensioned on this run's CA 829b3574-dee2-4d0d-a2f3-aff3a155b4e2 not met within 600.0s. Last result: []
The preceding ACM PCA calls all behaved: CreateCertificateAuthority, GetCertificateAuthorityCsr, IssueCertificate (root, 2 DAYS), GetCertificate, ImportCertificateAuthorityCertificate, DescribeCertificateAuthority -> Status 'ACTIVE', IssueCertificate (1 DAY) -> signed, IssueCertificate (7 DAYS) -> refused ValidationException 'The certificate validity specified exceeds the certificate authority validity.', IssueCertificate (1 DAY) -> signed. Only the metric delivery is missing.

## Steps to Reproduce
1. CreateCertificateAuthority with CertificateAuthorityType=ROOT, UsageMode=SHORT_LIVED_CERTIFICATE, KeyAlgorithm=RSA_2048, SigningAlgorithm=SHA256WITHRSA.
2. Wait for PENDING_CERTIFICATE, GetCertificateAuthorityCsr, IssueCertificate against the CA with TemplateArn=arn:aws:acm-pca:::template/RootCACertificate/V1 and Validity={Value:2,Type:DAYS}, GetCertificate, ImportCertificateAuthorityCertificate; wait for Status=ACTIVE.
3. IssueCertificate a locally generated end-entity CSR with TemplateArn=...EndEntityCertificate/V1 and Validity={Value:1,Type:DAYS}; GetCertificate until signed.
4. IssueCertificate the same CSR with Validity={Value:7,Type:DAYS} — observe the (correct) ValidationException refusal.
5. IssueCertificate again with Validity={Value:1,Type:DAYS}; GetCertificate until signed.
6. cloudwatch list_metrics(Namespace='AWS/ACMPrivateCA', MetricName='Success') and poll. Observe an empty Metrics list; repeat for MetricName='Time'. No AWS/ACMPrivateCA series is ever published for the CA.
