Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui
Issue-Key: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui|_https_request#47|TimeoutError|raise TimeoutError(f"an HTTPS answer from {dns_name!r} for {path!r}: the run budget ended first; not graded")
Issue-Commit: 44f88a38fb8560ba6fbbf2b389df854ff00d57e4

# cognito-idp__elbv2: an ALB HTTPS listener carrying an authenticate-cognito action never answers on the load balancer endpoint, so the hosted-UI redirect, the deny 401 and the open 200 are all unobservable

> _Note: this issue was identified against the code base at commit `44f88a38fb8560ba6fbbf2b389df854ff00d57e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
One root cause.

Cause 1 of 1: the ALB data plane serves no HTTPS answer at all for a listener/rule set whose actions are authenticate-cognito + fixed-response, so none of the three documented outcomes can be observed. Failing check: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui

AWS: the elbv2 control plane accepts an HTTPS listener whose DefaultActions are an authenticate-cognito action (Smithy: AuthenticateCognitoActionConfig with required UserPoolArn, UserPoolClientId, UserPoolDomain, plus Scope and OnUnauthenticatedRequest) followed by an ordered fixed-response action, and the load balancer then SERVES requests on the DNSName it returned from CreateLoadBalancer, over TLS using the listener's ACM certificate. Per the modeled documentation of AuthenticateCognitoActionConfig.OnUnauthenticatedRequest: 'authenticate' redirects the request to the IdP authorization endpoint (an HTTP 302 to the user pool's hosted UI with client_id, redirect_uri built from the request Host header, response_type=code, the configured scope and a state value, setting an AWSALBAuthNonce* cookie); 'deny' returns 'an HTTP 401 Unauthorized error'; a rule with no authenticate action simply forwards and the fixed-response action serves HTTP 200 with its MessageBody. A request bearing an unverifiable AWSALBAuthSessionCookie-0 is treated as unauthenticated and redirected, not forwarded. The data-plane wire behavior itself is not in the Smithy model — recorded behavior and the ELB documentation the check cites (listener-authenticate-users) only.

Emulator: every control-plane step succeeds (ACM import reaches ISSUED, CreateLoadBalancer returns a DNSName, the user pool/domain/app client are created, CreateListener and both CreateRule calls succeed, and DescribeRules correctly echoes the authenticate-cognito action with the pool ARN, client id, domain and OnUnauthenticatedRequest), but the first data-plane request gets no answer: the check opens a TLS socket to the emulator endpoint that the announced load balancer DNS name maps to (ctx.endpoint_host(dns_name, 443)) and retries connection/TLS failures until the run budget ends, raising 'TimeoutError: an HTTPS answer from <lb-dns-name> for "/open": the run budget ended first; not graded'. Nothing is served on the load balancer's HTTPS port, so the status, the Location redirect, the cookies and the body are all absent — the emulator never reaches the point where a status could be compared.

Why: a client that provisions an ALB with Cognito authentication cannot exercise any of it against the emulator. An unauthenticated browser gets no 302 to the hosted UI, a deny rule yields no 401, an unauthenticated-but-open path yields no 200, and a forged AWSALBAuthSessionCookie-0 can neither be rejected nor observed — so the whole sign-in-at-the-edge pattern, including its negative paths, silently cannot be tested; a hang is indistinguishable from a wrong answer.

Fix: the ELBv2 load balancer data plane — the HTTPS listener front end must listen on the endpoint the returned DNSName resolves to, terminate TLS with the listener's ACM certificate, match rules by path-pattern in priority order, and implement the authenticate-cognito action (redirect-to-hosted-UI with nonce cookie for OnUnauthenticatedRequest=authenticate, 401 for deny, pass-through for allow) ahead of the ordered fixed-response/forward action. Related but distinct open issue: 20261104T074116Z-acm-elbv2-3-net-new-root-causes-an.md reports an ALB HTTPS listener never presenting its ACM certificate on the wire; it does not carry this check's replay key and says nothing about the authenticate-cognito action semantics.

Traces: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui.

## Evidence
Expected (AWS): GET /open over HTTPS to the ALB (Host = the certificate's host) -> HTTP 200 with the fixed-response MessageBody; GET / unauthenticated -> HTTP 302 with Location = the user pool hosted UI authorize endpoint carrying client_id = the created app client, redirect_uri = https://<request Host>/oauth2/idpresponse, response_type=code, scope = 'openid email' and a non-empty state, plus a Set-Cookie whose name starts with AWSALBAuthNonce; the same request carrying Cookie: AWSALBAuthSessionCookie-0=<forged> -> still HTTP 302 and never the protected body; GET /deny (OnUnauthenticatedRequest=deny) -> HTTP 401 with no AWSALBAuthNonce cookie. (Fixed text in each case is the status/redirect structure; client_id, the domain prefix, the Host, the nonce body and the state value are per-run values taken from the request and emulator state.)

Actual (emulator): no HTTP response is ever produced. The first data-plane call fails with
  File "check_cognito_idp__elbv2_alb_authenticate_cognito_redirects_an_unauthenticated_request_to_the_hosted_ui.py", line 199, in run
    served = _https_request(ctx, lb_dns, host, "/open", trust_path)
  File "...", line 626, in _https_request
    raise TimeoutError(f"an HTTPS answer from {dns_name!r} for {path!r}: the run budget ended first; not graded")
  TimeoutError: an HTTPS answer from '<generated>.us-west-2.elb.amazonaws.com' for '/open': the run budget ended first; not graded
_https_request connects to ctx.endpoint_host(dns_name, 443) — i.e. the emulator — and loops retrying OSError/ssl.SSLError, so the timeout means the emulator accepted no HTTPS conversation on the load balancer's listener port. All preceding control-plane assertions passed, including DescribeRules echoing AuthenticateCognitoConfig.UserPoolArn/UserPoolClientId/UserPoolDomain and OnUnauthenticatedRequest=deny.

## Steps to Reproduce
1. acm.ImportCertificate with a self-signed root + leaf for host <h>.example.com; wait for Status=ISSUED.
2. Create a VPC with two public subnets and a security group; elbv2.CreateLoadBalancer (Type=application, Scheme=internet-facing) and note DNSName.
3. cognito-idp.CreateUserPool, CreateUserPoolDomain(<prefix>) (wait for ACTIVE), CreateUserPoolClient with GenerateSecret=true, AllowedOAuthFlows=[code], AllowedOAuthScopes=[openid,email], CallbackURLs=[https://<h>.example.com/oauth2/idpresponse].
4. elbv2.CreateListener Protocol=HTTPS Port=443 with the imported certificate and DefaultActions = [authenticate-cognito(UserPoolArn, UserPoolClientId, UserPoolDomain=<prefix>, Scope='openid email', OnUnauthenticatedRequest=authenticate) Order=1, fixed-response 200 <nonce> Order=2].
5. elbv2.CreateRule priority 1 path-pattern /deny* with the same authenticate-cognito action but OnUnauthenticatedRequest=deny plus the fixed-response; CreateRule priority 2 path-pattern /open* with only the fixed-response.
6. Open a TLS socket to the load balancer endpoint (port 443, SNI = <h>.example.com, trusting the minted root) and send 'GET /open HTTP/1.1'. Expected: HTTP 200 with the nonce body. Observed: no answer — the connection never yields an HTTP response until the run budget ends.
7. Repeat for 'GET /' (expect 302 to the hosted UI + AWSALBAuthNonce cookie), 'GET /' with a forged AWSALBAuthSessionCookie-0 (expect 302), and 'GET /deny' (expect 401).
