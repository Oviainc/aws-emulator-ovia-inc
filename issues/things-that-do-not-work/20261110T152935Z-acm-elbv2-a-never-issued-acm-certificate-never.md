Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.acm-never-issued-certificate-refused-by-an-https-listener-rejected
Issue-Key: acm__elbv2.acm-never-issued-certificate-refused-by-an-https-listener-rejected|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# acm__elbv2: a never-issued ACM certificate never settles into FAILED/ADDITIONAL_VERIFICATION_REQUIRED, so the ALB HTTPS listener's UnsupportedCertificate refusal is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestCertificate

## Description
One net-new root cause (Cause 1 of 1), grounded in one failing behavioral check that exhausts its whole wall clock before it reaches a single ELBv2 call.

Cause 1 of 1: a certificate requested for a name ACM cannot validate never becomes terminal, so the cross-service scenario "an ALB HTTPS listener refuses a certificate that holds no key material" can never be set up.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) RequestCertificate{DomainName required, ValidationMethod} -> CertificateArn; DescribeCertificate -> CertificateDetail.Status of CertificateStatus [PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED, FAILED], CertificateDetail.FailureReason of FailureReason [... ADDITIONAL_VERIFICATION_REQUIRED ...] documented as existing only when Status is FAILED, plus Type (AMAZON_ISSUED | IMPORTED), Serial and InUseBy. Recorded behavior: a public certificate requested for a non-validatable public name (any *.example.com label) settles within seconds into Status 'FAILED' with FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', Type 'AMAZON_ISSUED', the requested DomainName echoed verbatim, no Serial, InUseBy []. On the ELBv2 side the scenario's requirement is that elbv2.CreateListener(Protocol='HTTPS', Port=443, Certificates=[{CertificateArn: <that certificate>}]) is refused with error code 'UnsupportedCertificate' and the message "The certificate '<CertificateArn>' must have a fully-qualified domain name, a supported signature, and a supported key size." — the sentence is fixed text for that error class and its only variable part is the certificate ARN taken from the request — while no listener is created, the certificate's Status stays 'FAILED' and its InUseBy stays [], and the byte-identical call naming an ISSUED imported certificate succeeds and makes ACM report InUseBy == [<the load balancer ARN>].

Emulator: the setup never completes. After acm.request_certificate(DomainName='fail-<uid>.example.com', ValidationMethod='DNS'), the check's first blocking step polls DescribeCertificate for (Status == 'FAILED' and DomainName present) with a 180s budget; the predicate is never satisfied and the suite reports "Error: Check timed out after 60.0s". No load balancer, no listener and no CreateListener refusal is ever exercised. Hypothesis (consistent with the replay, not stated by the model): ACM's state machine has no request-failure path — a requested AMAZON_ISSUED certificate is parked non-terminal regardless of whether its names are validatable for the account — so no modeled FailureReason is ever produced and nothing downstream that needs a never-issued certificate can be driven.

Why: a client relying on AWS's behavior hangs where AWS fails fast, and the ELBv2 guard that protects it is never reachable. On AWS, tooling that requests a certificate for a name it cannot validate sees FAILED/ADDITIONAL_VERIFICATION_REQUIRED and stops; against the emulator it spins to its own timeout and cannot tell "still validating" from "will never issue". Worse for the cross-service contract: because no FAILED certificate can be produced, a caller cannot observe that ELBv2 refuses such a certificate with UnsupportedCertificate, so code that catches that error to fall back to another certificate looks dead and an emulator-only deployment can appear to attach a keyless certificate to a live HTTPS listener.

Fix: the ACM certificate state machine in the acm service crate — the RequestCertificate handler's validation/issuance driver and the stored state DescribeCertificate projects — must settle a certificate whose names cannot be validated into Status 'FAILED' with FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', Type 'AMAZON_ISSUED', no Serial, InUseBy [], and keep it there. Once that exists, ELBv2's CreateListener certificate validation (the listener handler's certificate check) must refuse such a certificate with 'UnsupportedCertificate' and the message above, create no listener, and leave the certificate's state untouched.

Traces: acm__elbv2.acm-never-issued-certificate-refused-by-an-https-listener-rejected.

=== Related open issues that do NOT cover this replay ===
20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md owns three other acm__elbv2 causes (including CreateListener accepting a FAILED certificate) and carries a different check id; it does not carry this one, and its CreateListener cause cannot even be observed while ACM cannot produce a FAILED certificate. 20261102T085352Z-acm-requestcertificate-for-a-long-aws-owned-elb.md reports the same ACM observable against the WIRE corpus (fixture acm-osc-public-cert-for-alb-dns-name) and is filed under 'acm'. 20261109T132410Z-acm-elbv2-the-alb-s-published-dnsname-is.md and 20261107T183213Z-acm-elbv2-an-alb-https-443-listener-holding.md concern the data plane (the published DNSName / a refused TCP connection), not this control-plane refusal.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: acm__elbv2 3/7 (43%), 4 failed; three of those four are marked OWNED by open issues and skipped. The surviving failure:

[mismatch] acm__elbv2.acm-never-issued-certificate-refused-by-an-https-listener-rejected
  Error: Check timed out after 60.0s

The check's first blocking step, before any ELBv2 or EC2 call: acm.request_certificate(DomainName='fail-<uid>.example.com', ValidationMethod='DNS'), then _poll_until(lambda: acm.describe_certificate(CertificateArn=<arn>)['Certificate'], lambda d: d.get('Status') == 'FAILED' and 'DomainName' in d, timeout_s=180, interval_s=5, description='the requested certificate settles into its terminal FAILED state'). Expected (AWS): the predicate holds within seconds, with Type 'AMAZON_ISSUED', FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', DomainName equal to the requested name, Serial absent. Actual: the predicate never holds and the check burns its entire wall clock.

Unreached as a consequence (the check's actual subject): elbv2.create_listener(LoadBalancerArn=<alb>, Protocol='HTTPS', Port=443, Certificates=[{'CertificateArn': <the never-issued arn>}], DefaultActions=[fixed-response]) must be refused with Error.Code 'UnsupportedCertificate' and Error.Message "The certificate '<arn>' must have a fully-qualified domain name, a supported signature, and a supported key size."; DescribeListeners must still return []; the certificate's Status must still read 'FAILED' and InUseBy []; and the identical call with an ISSUED imported certificate must succeed, be the only listener, hold exactly that certificate in DescribeListenerCertificates, and make ACM report InUseBy == [<load balancer ARN>].

Per-run values here (certificate and load-balancer ARNs, the 'fail-<uid>'/'good-<uid>' labels, subnet/security-group ids, the fixed-response nonce) are not requirements; the requirements are the modeled tokens 'FAILED', 'ADDITIONAL_VERIFICATION_REQUIRED', 'AMAZON_ISSUED' and 'UnsupportedCertificate' plus the fixed message sentence with the request's ARN interpolated.

Smithy consulted: acm RequestCertificate, DescribeCertificate, CertificateStatus, FailureReason, CertificateDetail.{Status,FailureReason,Type,Serial,InUseBy}.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay suite simparity-behavioral-aws-v1; acm__elbv2.acm-never-issued-certificate-refused-by-an-https-listener-rejected reproduces as "Check timed out after 60.0s".
2. acm.request_certificate(DomainName='fail-<uid>.example.com', ValidationMethod='DNS') and keep the CertificateArn.
3. Poll acm.describe_certificate(CertificateArn=<arn>). Observed: Status never becomes terminal. Expected (AWS): Status 'FAILED' with FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED', Type 'AMAZON_ISSUED', DomainName echoing the request, no Serial, InUseBy [].
4. Create an internet-facing application load balancer in two subnets, confirm DescribeListeners returns [], then call elbv2.create_listener(LoadBalancerArn=<alb>, Protocol='HTTPS', Port=443, Certificates=[{'CertificateArn': <the arn from step 2>}], DefaultActions=[{'Type':'fixed-response','FixedResponseConfig':{'StatusCode':'200','ContentType':'text/plain','MessageBody':'<nonce>'}}]).
5. Expected (AWS): error code 'UnsupportedCertificate' with message "The certificate '<arn>' must have a fully-qualified domain name, a supported signature, and a supported key size.", no listener created, and the certificate's Status/InUseBy unchanged.
6. Positive control: repeat step 4 with an ISSUED imported certificate — it must succeed, be the only listener, hold exactly that certificate, and ACM must report InUseBy == [<load balancer ARN>].
7. Re-replay the check id in trace_ids to confirm the fix.
