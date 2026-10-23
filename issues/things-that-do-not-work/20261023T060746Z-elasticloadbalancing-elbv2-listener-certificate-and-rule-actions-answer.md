Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-listener-attribute-and-certificate-error-paths
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-listener-attribute-and-certificate-error-paths elbv2-listener-family-not-found-error-paths elbv2-listener-modify-and-attributes-nonexistent-listener-errors elbv2-listener-not-found-error-paths elbv2-listener-not-found-errors elbv2-listener-notfound-error-paths elbv2-modify-listener-inplace-update-and-notfound elbv2-not-found-error-paths-for-listener-and-lb-ops elbv2-listener-certificate-management-nonexistent-listener-errors elbv2-listener-certificates-full-workflow-not-found elbv2-listener-certificates-not-found-error-paths elbv2-listener-notfound-and-validation-error-paths elbv2-op-contract-describe-listeners elbv2-rule-target-group-load-balancer-not-found-errors
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|ModifyListener|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyListener|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyListener|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|ModifyListenerAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyListenerAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyListenerAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeListenerAttributes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeListenerAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeListenerAttributes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|AddListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|AddListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|AddListenerCertificates|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|RemoveListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|RemoveListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|RemoveListenerCertificates|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeListenerCertificates|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeListenerCertificates|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DeleteListener|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DeleteListener|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DeleteListener|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeListeners|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeListeners|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeListeners|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeRules|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeRules|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeRules|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elasticloadbalancing ELBv2 listener, certificate and rule actions answer 400 InvalidAction

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyListener

## Description
9 causes, one mechanism: the shared 'elasticloadbalancing' front door is the Classic ELB crate, which serves only Version=2012-06-01 and refuses every 2015-12-01 envelope with 400 InvalidAction 'Could not find operation <Action> for version <Version>' (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs); the ELBv2 model answers only under its private route key 'elasticloadbalancingv2'. Related OPEN issues that do NOT cover these fixture steps and carry no wire replay key: 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md, 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md, 20261022T220446Z-elbv2-elastic-load-balancing-v2-is-unreachable-every-request.md. Every cause below is a listener/rule operation whose modeled refusal is replaced by the routing refusal; each needs the version dispatch fixed and then that operation's own ARN lookup and error rendering. The recorded not-found messages are request-derived (they echo the listener ARN), so the rule — ListenerNotFound naming the ARN asked for — is the requirement; the quoted strings are examples.

Cause 1 of 9: ModifyListener on an absent listener.
AWS: Smithy ModifyListener declares ListenerNotFoundException, awsQueryError 'ListenerNotFound', HTTP 400 (example message "Listener 'arn:aws:elasticloadbalancing:us-west-2:000000000000:listener/app/probelb/0123456789abcdef/fedcba9876543210' not found").
Emulator: 400 Code 'InvalidAction', ErrorResponse.RequestId missing.
Why: listener reconfiguration cannot run and its not-found branch never fires.
Fix: version dispatch, then the ModifyListener handler/validation.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#12 (cluster of 8).

Cause 2 of 9: ModifyListenerAttributes on an absent listener.
AWS: ListenerNotFound / 400.
Emulator: 400 InvalidAction.
Why: listener attribute writes are unreachable.
Fix: version dispatch, then ModifyListenerAttributes handler.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#13 (cluster of 7).

Cause 3 of 9: DescribeListenerAttributes on an absent listener.
AWS: ListenerNotFound / 400.
Emulator: 400 InvalidAction.
Why: listener attribute reads are unreachable.
Fix: version dispatch, then DescribeListenerAttributes handler.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#14 (cluster of 7).

Cause 4 of 9: AddListenerCertificates on an absent listener.
AWS: Smithy AddListenerCertificates declares ListenerNotFound, CertificateNotFound, TooManyCertificates — all awsQuery 400; an absent listener is ListenerNotFound.
Emulator: 400 InvalidAction.
Why: SNI certificate management cannot be exercised.
Fix: version dispatch, then AddListenerCertificates handler.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#9 (cluster of 7).

Cause 5 of 9: RemoveListenerCertificates on an absent listener.
AWS: ListenerNotFound / 400.
Emulator: 400 InvalidAction.
Why: certificate removal is unreachable.
Fix: version dispatch, then RemoveListenerCertificates handler.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#10 (cluster of 10).

Cause 6 of 9: DescribeListenerCertificates on an absent listener.
AWS: ListenerNotFound / 400 (recorded message example 'One or more listeners not found').
Emulator: 400 InvalidAction.
Why: certificate listing is unreachable.
Fix: version dispatch, then DescribeListenerCertificates handler.
Traces: elbv2-listener-attribute-and-certificate-error-paths#task#11 (cluster of 8).

Cause 7 of 9: DeleteListener on an absent listener.
AWS: ListenerNotFound / 400, message naming the ARN.
Emulator: 400 InvalidAction.
Why: listener teardown cannot be exercised.
Fix: version dispatch, then DeleteListener handler.
Traces: elbv2-listener-not-found-error-paths#task#4.

Cause 8 of 9: DescribeListeners for an unknown listener ARN.
AWS: Smithy DescribeListeners declares ListenerNotFound, LoadBalancerNotFound, UnsupportedProtocol — awsQuery 400; an unknown ListenerArns entry is ListenerNotFound (recorded message example 'One or more listeners not found').
Emulator: 400 InvalidAction.
Why: listener discovery and its not-found branch are both unreachable.
Fix: version dispatch, then DescribeListeners handler.
Traces: elbv2-op-contract-describe-listeners#task#6.

Cause 9 of 9: DescribeRules for an absent listener.
AWS: Smithy DescribeRules declares ListenerNotFound, RuleNotFound, UnsupportedProtocol — awsQuery 400; an absent ListenerArn is ListenerNotFound ('One or more listeners not found').
Emulator: 400 InvalidAction.
Why: routing-rule inspection is unreachable, so no rule-priority or condition workflow can run.
Fix: version dispatch, then DescribeRules handler.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#1.

## Evidence
All actual responses are HTTP 400 with $.ErrorResponse.Error.Code 'InvalidAction', Message 'Could not find operation <Action> for version 2015-12-01', and no $.ErrorResponse.RequestId.
- elbv2-listener-attribute-and-certificate-error-paths#task#12 ModifyListener, #task#13 ModifyListenerAttributes, #task#14 DescribeListenerAttributes, #task#9 AddListenerCertificates, #task#10 RemoveListenerCertificates, #task#11 DescribeListenerCertificates: expected Code 'ListenerNotFound' in every case.
- elbv2-listener-not-found-error-paths#task#4 DeleteListener: expected 'ListenerNotFound' (message example "Listener 'arn:...:listener/app/sim2real-probe-nf-lb-5fb6cc52/0123456789abcdef/0123456789abcdef' not found").
- elbv2-op-contract-describe-listeners#task#6 DescribeListeners and elbv2-rule-target-group-load-balancer-not-found-errors#task#1 DescribeRules: expected 'ListenerNotFound' with message example 'One or more listeners not found'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Signing for service 'elasticloadbalancing', POST 'Action=ModifyListener&Version=2015-12-01&ListenerArn=arn:aws:elasticloadbalancing:us-west-2:000000000000:listener/app/probelb/0123456789abcdef/fedcba9876543210&Port=8080'. Observe 400 InvalidAction instead of 400 ListenerNotFound.
3. Repeat with Action = ModifyListenerAttributes, DescribeListenerAttributes, AddListenerCertificates, RemoveListenerCertificates, DescribeListenerCertificates, DeleteListener, DescribeListeners, DescribeRules at Version=2015-12-01.
4. Re-replay the fixture steps named in issue_step and trace_ids.
