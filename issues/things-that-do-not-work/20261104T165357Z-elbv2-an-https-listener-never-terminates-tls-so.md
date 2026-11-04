Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.imported-certificate-is-presented-and-sni-selects-per-host
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.imported-certificate-is-presented-and-sni-selects-per-host|_poll_until#18|Timeout|last = probe()
Issue-Commit: 5378a66c6079b2af7ea00e2980dbc1f9f47fc08b

# elbv2: an HTTPS listener never terminates TLS, so neither the default certificate nor SNI-selected per-host certificates are ever presented on the wire

> _Note: this issue was identified against the code base at commit `5378a66c6079b2af7ea00e2980dbc1f9f47fc08b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
One net-new root cause, grounded in the single elbv2 behavioral failure that is neither ALREADY FILED nor OWNED.

Related open filings that do NOT cover it: 20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md (its cause 1 is the same class of defect — an ALB HTTPS listener that never presents its ACM certificate — but it is filed under service acm__elbv2 and its Issue-Keys/trace ids are the acm__elbv2 checks only; it carries no replay key for this elbv2 check and says nothing about SNI selection or the unmatched-host default), 20261104T055008Z-elbv2-s3-an-https-alb-writes-no-access.md (the ALB access-log writer, not TLS termination), and 20261104T094656Z-elbv2-ten-target-health-routing-and-lifecycle-root.md (target-health/routing causes; this check is not among its keys).

Cause 1 of 1: an HTTPS:443 ALB listener that holds an ACM-imported certificate (and a second certificate added with AddListenerCertificates) completes no TLS handshake at all, so certificate presentation, SNI selection and the default-certificate fallback are all unobservable.
AWS: Smithy CreateListener(LoadBalancerArn required, Protocol ProtocolEnum, Port 1..65535, SslPolicy, Certificates: CertificateList, DefaultActions required) -> CreateListenerOutput; AddListenerCertificates(ListenerArn required, Certificates required) -> Certificates, documented as "Adds the specified SSL server certificate to the certificate list for the specified HTTPS or TLS listener" — i.e. the added entries are the listener's SNI certificate list, while the certificate named in CreateListener is the listener default (IsDefault true). Beyond the model this is recorded behavior only: an internet-facing application load balancer that reports State.Code 'active' with an HTTPS:443 listener accepts a TCP connection on its DNSName:443 and completes a TLS handshake under the listener's security policy (ELBSecurityPolicy-2016-08); it presents the leaf whose key material ACM holds for the listener's default CertificateArn — byte-identical to the imported certificate, same serial, same subject CN — when the ClientHello carries no matching SNI, selects the certificate from the listener certificate list whose SAN/CN matches the SNI server_name when one matches, and then runs the listener's default action (here a fixed-response). There is no error response involved: the recording's expectation is a successful handshake, so no status code or error code is at stake.
Emulator: nothing answers TLS on the load balancer's DNSName:443. The control plane accepts CreateLoadBalancer (reaching active), CreateListener with Protocol=HTTPS/Port=443/SslPolicy=ELBSecurityPolicy-2016-08/Certificates=[the imported ACM ARN], AddListenerCertificates for the second certificate and DescribeListenerCertificates, but every TLS connect/handshake attempt from the client fails or hangs: the check's bounded poll for "the HTTPS listener presenting the imported certificate" never sees a peer certificate and the run is cut off with 'Check timed out after 60.0s', before the SNI and default-fallback assertions are ever reached.
Why: a client relying on AWS behavior gets a provisioned, 'active' HTTPS listener that cannot serve a single request. It is reported only as a hung connect or a transport error, never as an error code the caller can branch on, so HTTPS fronting, SNI-based multi-host hosting on one listener (the reason AddListenerCertificates exists) and any end-to-end flow behind an HTTPS ALB are unreachable; a caller cannot distinguish 'wrong certificate selected' from 'no listener at all'.
Fix: the ELBv2 data plane in the elbv2 service crate — the HTTPS/TLS listener path that must bind the listener's port, complete the handshake with the listener's SslPolicy, choose the certificate from the listener's certificate list by the ClientHello SNI server_name and fall back to the default (IsDefault) certificate when none matches, fetching key material through the ACM lookup, and then execute the listener's default action / matching rule action.
Traces: elbv2.imported-certificate-is-presented-and-sni-selects-per-host

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 197/554 passed; service heading '### elbv2' 148/294 (50%), 146 failed. Of those 146, every failure except one is marked ALREADY FILED or OWNED by an existing issue; the single unowned one is this check.

elbv2.imported-certificate-is-presented-and-sni-selects-per-host [mismatch]: 'Error: Check timed out after 60.0s'.

What the check does (from the replayed source): mints two self-signed RSA-2048 leaves with CN/SAN sim2real-a-<uniq>.example.com and sim2real-b-<uniq>.example.com, imports both with acm.import_certificate, creates a security group open on 443, creates an internet-facing application load balancer in two AZ subnets, waits for State.Code == 'active', then elbv2.create_listener(Port=443, Protocol='HTTPS', SslPolicy='ELBSecurityPolicy-2016-08', Certificates=[{'CertificateArn': <arn_a>}], DefaultActions=[fixed-response 200 'TLS-OK']). It then polls a real TLS handshake against the load balancer's DNSName:443 (verification off; the peer certificate is compared with the imported one, not a trust root) and requires:
  (a) the presented certificate's serial and subject CN to equal the imported leaf A (serials are minted locally, so a match proves that exact certificate was presented);
  (b) after add_listener_certificates(arn_b) and describe_listener_certificates listing both ARNs, a handshake with SNI host B to present leaf B's serial/CN, and to differ from A's;
  (c) a handshake with an unmatched SNI host to present the listener default, leaf A.
None of (a)-(c) is observed: no handshake ever completes, so the check exhausts the 60s budget inside the first poll ('the HTTPS listener presenting the imported certificate for <host_a>'). The hostnames, serials and ARNs are per-run values filling the template; the requirement is that an active HTTPS listener completes a handshake and presents the default certificate without SNI, the SNI-matching certificate with it, and the default again for an unmatched SNI host.

Gate: the service heading is 'elbv2' (crate services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10), which is bootstrapped, so this is a fidelity finding and is filed.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite; the failure above reproduces under the '### elbv2' heading.
2. Import two self-signed leaves into ACM, one with CN/SAN host-a.example.com and one with host-b.example.com (acm.import_certificate).
3. Create an internet-facing application load balancer in two AZ subnets with a security group permitting TCP 443, and wait until describe_load_balancers reports State.Code == 'active'.
4. elbv2.create_listener(LoadBalancerArn=<lb>, Protocol='HTTPS', Port=443, SslPolicy='ELBSecurityPolicy-2016-08', Certificates=[{'CertificateArn': <arn_a>}], DefaultActions=[{'Type':'fixed-response','FixedResponseConfig':{'StatusCode':'200','ContentType':'text/plain','MessageBody':'TLS-OK'}}]).
5. Open a TCP connection to the load balancer's DNSName:443 and complete a TLS handshake with SNI server_name = host-a.example.com: the emulator completes no handshake (the client hangs/errors); AWS presents the imported leaf A (same serial, same subject CN) and answers the fixed response.
6. elbv2.add_listener_certificates(ListenerArn=<listener>, Certificates=[{'CertificateArn': <arn_b>}]), confirm describe_listener_certificates lists both, then handshake with SNI host-b.example.com (AWS presents leaf B) and with an unrelated SNI host (AWS presents the default, leaf A).
7. Re-replay elbv2.imported-certificate-is-presented-and-sni-selects-per-host; it must pass before closing.
