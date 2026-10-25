Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.add-listener-certificates-malformed-arn-rejected elbv2.https-to-http-redirect-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.add-listener-certificates-malformed-arn-rejected|_error_code#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: elbv2.https-to-http-redirect-rejected|_error#21|AssertionError|raise AssertionError(f"call was ACCEPTED but real AWS rejects it: {resp!r}")
Issue-Commit: e2f860a6b07ef9d9819261c01b0be0239c04fcd6

# ELBv2 accepts two configurations real AWS refuses: a malformed certificate ARN on AddListenerCertificates, and an HTTPS→HTTP redirect downgrade on CreateRule/ModifyListener

> _Note: this issue was identified against the code base at commit `e2f860a6b07ef9d9819261c01b0be0239c04fcd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddListenerCertificates

## Description
Two distinct net-new root causes in the elasticloadbalancingv2 emulator, both of the same family: a request real AWS REFUSES is answered 200 OK and mutates state. Neither is covered by an open issue — the nearest neighbours are 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md and 20261025T135132Z-elbv2-addtags-answers-501-notimplemented-instead-of-the.md, which are about operations answering 501 NotImplemented; here the operations ARE implemented and answer success where AWS answers a modeled 400. This issue carries 2 causes; both must be closed before submitting.

Cause 1 of 2 — AddListenerCertificates accepts a syntactically-malformed certificate ARN.
Failing check: elbv2.add-listener-certificates-malformed-arn-rejected
AWS: AddListenerCertificates resolves each Certificates[].CertificateArn against the caller's ACM/IAM certificates. A value that is not a certificate ARN at all resolves to nothing, so the call is refused with the modeled error CertificateNotFoundException (Smithy: AddListenerCertificates errors include CertificateNotFoundException, errorKind client, httpError 400, awsQueryError code "CertificateNotFound", httpResponseCode 400; doc "The specified certificate does not exist."). The recorded refusal is HTTP 400, Error/Code "CertificateNotFound", Error/Message of the form "Certificate '<arn>' not found" — the fixed text is "Certificate '" … "' not found" and the only variable part is the CertificateArn value echoed verbatim from the request (in the recording that value was the literal the check sent, not a generated id). The listener's certificate list is left untouched by the refusal.
Emulator: the call succeeds with HTTP 200 and echoes the bogus ARN back as an attached certificate: {'Certificates': [{'CertificateArn': 'not-a-certificate-arn-at-all', 'IsDefault': False}]}. No existence or syntax resolution of the ARN happens, and the bogus entry is added to the listener's certificate list, so the subsequent DescribeListenerCertificates no longer shows only the real default certificate.
Why: a client that relies on AWS refusing an unresolvable certificate sees a listener it believes is serving a certificate that does not exist. Typo/stale-ARN detection — the whole point of CertificateNotFound — never fires, error-handling branches for it are dead in the emulator, and the listener's certificate list is corrupted with an entry no real ACM certificate backs. Code that then reads the list back and feeds the ARNs to ACM gets a second, unrelated failure far from the cause.
Fix: validation in the AddListenerCertificates handler (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/add_listener_certificates.rs) plus the certificate-resolution lookup it must consult: resolve every requested CertificateArn against stored certificate state before mutating the listener, and on failure raise CertificateNotFoundException (400 / "CertificateNotFound") with the message template above and apply NO partial mutation to the listener's certificate list.

Cause 2 of 2 — CreateRule and ModifyListener accept a redirect action that downgrades HTTPS to HTTP.
Failing check: elbv2.https-to-http-redirect-rejected
AWS: a redirect action's protocol is validated against the protocol of the listener the action is attached to. Smithy models RedirectActionConfig.Protocol as RedirectActionProtocol with pattern ^(HTTPS?|#\{protocol\})$ — so the string "HTTP" is pattern-valid and a purely model-driven emulator accepts it — but the member's own documentation states the rule the pattern cannot express: "You can redirect HTTP to HTTP, HTTP to HTTPS, and HTTPS to HTTPS. You can't redirect HTTPS to HTTP." On an HTTPS listener AWS therefore refuses the action, on both CreateRule and ModifyListener's DefaultActions, with InvalidLoadBalancerActionException (Smithy: both operations list InvalidLoadBalancerActionException, errorKind client, httpError 400, awsQueryError code "InvalidLoadBalancerAction"). The recorded refusal is HTTP 400, Error/Code "InvalidLoadBalancerAction", Error/Message exactly "You cannot redirect HTTPS to HTTP." — this message is entirely fixed text with no variable parts. The identical RedirectConfig attached to an HTTP listener is accepted, which is what makes this a relationship between the action and the listener's protocol rather than a request-local constraint.
Emulator: CreateRule on the HTTPS listener with Actions=[{Type: redirect, RedirectConfig: {Protocol: HTTP, Port: 8080, StatusCode: HTTP_302}}] returns HTTP 200 and a created rule carrying that redirect action, and the rule is persisted on the HTTPS listener (DescribeRules then shows a non-default rule where AWS shows only the default). ModifyListener with the same action as DefaultActions likewise succeeds instead of refusing. The emulator validates only the member pattern, never the listener-protocol relationship.
Why: a client gets a listener rule that real AWS would never create, so a deployment that validates clean against the emulator fails at the real API — the exact inversion the emulator exists to prevent. Worse, the downgrade is a security regression the API is designed to block: traffic the caller believes stays on TLS is redirected to cleartext, and the InvalidLoadBalancerAction branch clients write for it is never exercised.
Fix: validation in the CreateRule and ModifyListener handlers (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/create_rule.rs and .../src/ops/modify_listener.rs), reading the owning listener's Protocol out of listener state: when the listener protocol is HTTPS (or TLS) and a redirect action's RedirectConfig.Protocol resolves to HTTP, refuse with InvalidLoadBalancerActionException (400 / "InvalidLoadBalancerAction") and the fixed message "You cannot redirect HTTPS to HTTP.", creating no rule and leaving the default actions unchanged. The check pins both call sites, so fixing only CreateRule will not close it.

## Evidence
Cause 1 — elbv2.add-listener-certificates-malformed-arn-rejected [mismatch]
  Expected (AWS): HTTP 400, Code "CertificateNotFound", Message "Certificate 'not-a-certificate-arn-at-all' not found"; listener certificate list unchanged (only the imported default certificate present).
  Actual (emulator): HTTP 200 OK — AssertionError: expected the call to be rejected, but it succeeded: {'Certificates': [{'CertificateArn': 'not-a-certificate-arn-at-all', 'IsDefault': False}], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', ...}}}. The bogus ARN is attached to the listener.

Cause 2 — elbv2.https-to-http-redirect-rejected [mismatch]
  Expected (AWS): HTTP 400, Code "InvalidLoadBalancerAction", Message "You cannot redirect HTTPS to HTTP." from BOTH CreateRule (on the HTTPS listener) and ModifyListener (DefaultActions on the same listener); no rule created; the same RedirectConfig accepted on the HTTP:80 listener.
  Actual (emulator): HTTP 200 OK — AssertionError: call was ACCEPTED but real AWS rejects it: {'Rules': [{'RuleArn': 'arn:aws:elasticloadbalancing:<region>:<account>:listener-rule/app/<lb-name>/<lb-id>/<listener-id>/<rule-id>', 'Priority': '10', 'Conditions': [{'Field': 'path-pattern', 'Values': ['/plain'], ...}], 'Actions': [{'Type': 'redirect', 'RedirectConfig': {'Protocol': 'HTTP', 'Port': '8080', 'StatusCode': 'HTTP_302'}}], 'IsDefault': False}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (ARN components, request id and date are per-run values, not requirements).

Smithy citations: AddListenerCertificates -> CertificateNotFoundException (client, 400, awsQueryError code CertificateNotFound); CreateRule and ModifyListener -> InvalidLoadBalancerActionException (client, 400, awsQueryError code InvalidLoadBalancerAction); RedirectActionConfig.Protocol target RedirectActionProtocol pattern ^(HTTPS?|#\{protocol\})$ with doc "You can't redirect HTTPS to HTTP."

## Steps to Reproduce
Cause 1 (elbv2.add-listener-certificates-malformed-arn-rejected):
1. Create a VPC with two subnets in two AZs; CreateLoadBalancer (application) on them and wait for active.
2. ACM ImportCertificate a self-signed certificate; CreateTargetGroup in the VPC.
3. CreateListener Protocol=HTTPS, Port=443, Certificates=[{CertificateArn: <imported>}], DefaultActions=[forward to the target group].
4. Call AddListenerCertificates(ListenerArn=<listener>, Certificates=[{CertificateArn: "not-a-certificate-arn-at-all"}]).
   Expected: 400 CertificateNotFound, message "Certificate 'not-a-certificate-arn-at-all' not found". Observed: 200 with the bogus ARN echoed as attached.
5. DescribeListenerCertificates(ListenerArn=<listener>) — expected to list only the imported default certificate; the emulator also lists the bogus ARN.

Cause 2 (elbv2.https-to-http-redirect-rejected):
1. Create a VPC (e.g. 10.87.0.0/16) with two subnets in two available AZs; ACM ImportCertificate a self-signed certificate.
2. CreateLoadBalancer (application, internal) on those subnets.
3. CreateListener Protocol=HTTPS, Port=443 with that certificate and a fixed-response 404 default action; CreateListener Protocol=HTTP, Port=80 with the same default action.
4. CreateRule(ListenerArn=<HTTPS listener>, Priority=10, Conditions=[{Field: path-pattern, Values: ["/plain"]}], Actions=[{Type: redirect, RedirectConfig: {Protocol: "HTTP", Port: "8080", StatusCode: "HTTP_302"}}]).
   Expected: 400 InvalidLoadBalancerAction, message "You cannot redirect HTTPS to HTTP.". Observed: 200 with the rule created.
5. ModifyListener(ListenerArn=<HTTPS listener>, DefaultActions=[the same redirect action]) — same expected refusal; the emulator accepts it.
6. DescribeRules(ListenerArn=<HTTPS listener>) — expected only the default rule; the emulator shows the redirect rule persisted.
7. Control: the identical redirect action on the HTTP:80 listener must still be ACCEPTED.
