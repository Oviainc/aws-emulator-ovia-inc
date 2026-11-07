Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate
Issue-Key: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate|_https_get#30|AssertionError|raise AssertionError(
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# acm__elbv2: an ALB HTTPS:443 listener holding an ISSUED ACM certificate refuses the TCP connection (ConnectionRefused), so the certificate is never presented and no HTTPS request is ever answered

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
One net-new root cause (1 of 1).

Related open filing that does NOT cover this replay: 20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md lists this check id under its Cause 1, but its recorded replay key for it is `_https_get#30|Timeout|time.sleep(5)` — a 60 s budget exhaustion. This replay no longer matches that key: the connect now fails IMMEDIATELY and definitively with `ConnectionRefusedError: [Errno 111] Connection refused`, repeated for the full 120 s window, so the suite does not mark the check OWNED and it survives as a live failure. The observable has changed from "nothing answered in time" to "nothing is bound to the listener's port at all". Filing it so the cause is tracked against the behavior the emulator shows today; a fixer closing it should close that issue's Cause 1 in the same change.

Cause 1 of 1: an application load balancer in State.Code 'active' whose HTTPS:443 listener names an ISSUED ACM certificate accepts no TCP connection on its DNSName:443.
AWS: recorded behavior only for the data plane (the Smithy model describes only the control plane). Smithy: elbv2 CreateListener(LoadBalancerArn required, Protocol, Port, Certificates: CertificateList, DefaultActions required) -> CreateListenerOutput; its modeled errors (CertificateNotFoundException, UnsupportedProtocolException, InvalidConfigurationRequestException, ...) are all client errors at HTTP 400, i.e. the model offers no state in which a created, active HTTPS listener legitimately refuses connections. Recorded AWS behavior: once DescribeLoadBalancers reports State.Code 'active', a TCP connect to DNSName:443 succeeds, the TLS handshake completes under the listener's security policy, the leaf presented is byte-for-byte the certificate stored in ACM for the listener's CertificateArn (same serial, same DER, same DNS SANs), and the listener's default action (here a fixed-response) produces the HTTP response.
Emulator: every connect to the load balancer's DNSName on port 443 is refused at the TCP layer. The check retries for 120 s and reports `AssertionError: the load balancer '<dns-name>' was active but never answered an HTTPS request within 120s ... Last error: ConnectionRefusedError: [Errno 111] Connection refused`. The control plane accepted CreateLoadBalancer, CreateListener and reported 'active'; nothing listens. (The host name and the 120 s window are per-run values; the requirement is that an active HTTPS listener accepts the connection and completes the handshake with the ACM leaf.)
Why: a client cannot do the one thing an ACM certificate on an ALB exists for — serve TLS under its own name. A refused connection is also the worst possible signal: callers and IaC tooling see a successful 200 control-plane rollout, then a transport-level refusal with no AWS error code to branch on, so certificate presentation, SNI selection, hostname verification against a private anchor and live certificate rotation are all unreachable and indistinguishable from a network fault.
Fix: the ELBv2 data plane in the elbv2 service crate — the listener-socket path that must bind the listener's Port once the load balancer becomes active, terminate TLS presenting the leaf and chain that ACM holds for each Certificates[].CertificateArn, and then run the listener's DefaultActions; plus the ACM -> ELBv2 key-material lookup it needs.
Traces: acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 202/554 passed; acm__elbv2 3/7 (43%), 4 failed. Three of the four are marked OWNED by 20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md; this one is not marked and survives.

[mismatch] acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate:
  File "check_acm__elbv2_acm_alb_https_listener_serves_the_imported_certificate.py", line 366, in run
    result = _https_get(ctx, lb_dns, host, _pem(root), tmpdir)
  File "...", line 243, in _https_get
  AssertionError: the load balancer 'sim2real-xsvc-lb-603645bd7a30-9978363479.us-west-2.elb.amazonaws.com' was active but never answered an HTTPS request within 120s -- provisioned but unconnectable is a failure, not an environment limit. Last error: ConnectionRefusedError: [Errno 111] Connection refused

The check mints a root CA and a leaf, imports the leaf with acm.import_certificate, waits for Status ISSUED, builds its own VPC/subnets/security group, creates an internet-facing ALB plus an HTTPS:443 listener with Certificates=[the imported ARN] and a fixed-response default action, waits for State.Code 'active', and only then connects. Expected: TLS handshake completes, the presented leaf equals the imported one, and the fixed response is returned. Actual: connection refused for the entire 120 s window, so none of the certificate assertions is reached. The DNS name shown is a per-run value.

Smithy consulted: elbv2 CreateListener (input Certificates: CertificateList; errors CertificateNotFoundException/UnsupportedProtocolException/InvalidConfigurationRequestException, all client, HTTP 400) — no modeled outcome in which an active HTTPS listener refuses connections.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; acm__elbv2.acm-alb-https-listener-serves-the-imported-certificate reproduces.
2. acm.import_certificate a leaf chained to a self-signed root; poll acm.describe_certificate until Status == ISSUED.
3. Create a VPC with two public subnets and a security group allowing 443; elbv2.create_load_balancer(Scheme='internet-facing', Type='application'); poll until State.Code == 'active'.
4. elbv2.create_listener(LoadBalancerArn=<lb>, Protocol='HTTPS', Port=443, Certificates=[{'CertificateArn': <arn>}], DefaultActions=[{'Type':'fixed-response', ...}]).
5. Open a TCP connection to the load balancer's DNSName on port 443 (SNI = the certificate's SAN). The emulator refuses the connection (ECONNREFUSED) on every attempt for 120 s; AWS completes the handshake, presents the imported leaf and returns the fixed response.
6. Re-replay the check id in trace_ids to confirm the fix.
