Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui
Issue-Key: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui|_https_request#47|PublishedUrlRejected|with socket.create_connection(ctx.endpoint_host(dns_name, 443), timeout=15) as raw:
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# cognito-idp__elbv2: an ALB's published DNSName is a real-AWS elb.amazonaws.com host the emulator does not serve, so authenticate-cognito on a listener is unreachable

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
Cause 1 of 1: the load balancer address the emulator announces is not an address it answers on.
AWS: Smithy: LoadBalancer.DNSName is the name a client resolves and connects to; CreateLoadBalancer/DescribeLoadBalancers publish it. Whatever host the emulator reports there must be one it serves (its own host[:port], or a loopback address for a loopback endpoint).
Emulator: publishes an AWS-shaped name, <lb-name>-<id>.<region>.elb.amazonaws.com, and the HTTPS listener is reported on :443 — a host that is not one of the emulator's own hosts, so a client connecting to the announced address never reaches the emulator at all ('endpoint_host was given <name>.us-west-2.elb.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts'). The generated name and port are per-run values; the requirement is that the announced host be served.
Why: no client can reach an ALB the emulator created, so the whole authenticate-cognito behaviour on a listener rule — the 302 redirect of an unauthenticated request to the user pool's hosted UI, with the pool domain, client_id, response_type and redirect_uri query parameters — is unobservable.
Fix: the elbv2 CreateLoadBalancer/DescribeLoadBalancers state and serializer that mint DNSName, plus the listener front door that must serve the announced address.
Traces: cognito-idp__elbv2.alb-authenticate-cognito-redirects-an-unauthenticated-request-to-the-hosted-ui.

## Evidence
simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-cipalb-651e2362dbc0-1064393880.us-west-2.elb.amazonaws.com:443 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address.

## Steps to Reproduce
1) Create a Cognito user pool with a hosted-UI domain and an app client.
2) Create an application load balancer with an HTTPS listener whose default action is authenticate-cognito followed by a fixed-response.
3) Read the load balancer's DNSName and connect to it — the published name is a *.elb.amazonaws.com host the emulator does not serve, so the unauthenticated request never gets the hosted-UI redirect.
