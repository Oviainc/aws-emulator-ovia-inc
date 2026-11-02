Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.https-listener-round-trips-the-alb-san-certificate-and-tls13-policy
Issue-Key: acm__elbv2.https-listener-round-trips-the-alb-san-certificate-and-tls13-policy|_poll#15|Timeout|time.sleep(interval_s)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# acm__elbv2: ACM never reports an imported certificate's InUseBy, so an HTTPS listener using it is never listed as a consumer

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
Cause 1 of 1: DescribeCertificate's InUseBy never names the load balancer whose HTTPS listener serves the certificate.
AWS: after an ELBv2 HTTPS listener is created with Certificates=[{CertificateArn: <arn>}], ACM DescribeCertificate for that certificate reports Certificate.InUseBy containing exactly the load balancer's ARN (the association appears within a short settling window; the identity of the consumer, not the latency, is the requirement), alongside Type 'IMPORTED'. Recorded behavior only — InUseBy is a modeled list of consumer ARNs with no constraint stating who populates it.
Emulator: the certificate's InUseBy never comes to contain the load balancer ARN: the check's only unbounded wait is the poll on DescribeCertificate(...)['Certificate'].get('InUseBy', []) (180 s budget) and the check ends at the suite's 60 s ceiling with the predicate unsatisfied. Everything before that poll is non-blocking (VPC/subnet/SG creation, CreateLoadBalancer, CreateTargetGroup, ImportCertificate, CreateListener and a DescribeListeners round-trip), so the stall is the association never being recorded, not a provisioning wait.
Why: certificate-hygiene automation (find certificates safe to delete, audit which balancer serves which certificate, block a delete while in use) reads InUseBy; an always-empty list makes every in-use certificate look unused.
Fix: cross-service consumer tracking for ACM certificates — ELBv2 listener create/modify/delete must register and release the certificate's consumer, and ACM DescribeCertificate must project it as InUseBy.
Traces: acm__elbv2.https-listener-round-trips-the-alb-san-certificate-and-tls13-policy.

## Evidence
Expected: DescribeCertificate(CertificateArn=<imported cert>)['Certificate']['InUseBy'] == [<the ALB's LoadBalancerArn>] after CreateListener(Protocol='HTTPS', Port=443, SslPolicy='ELBSecurityPolicy-TLS13-1-2-2021-06', Certificates=[{'CertificateArn': <cert>}]).
Actual: the poll described as 'ACM listing the ALB among the certificate's users' never satisfies and the check reports 'Check timed out after 60.0s'; no assertion before the poll fails, so the listener round-trip itself was answered.

## Steps to Reproduce
Replay the behavioral suite and re-run acm__elbv2.https-listener-round-trips-the-alb-san-certificate-and-tls13-policy. Equivalently: create an internal ALB and an ip target group, ImportCertificate a self-signed leaf whose SAN is the ALB's DNSName, CreateListener HTTPS:443 with that certificate forwarding to the group, then poll ACM DescribeCertificate and expect InUseBy to name the load balancer ARN.
