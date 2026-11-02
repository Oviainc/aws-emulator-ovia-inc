Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.albos-health-check-matcher-code-above-499-rejected ec2__elbv2.albos-ip-targets-a-listener-stops-routing-to-revert-to-unused-not-in-use ec2__elbv2.albos-stale-ip-targets-behind-a-listener-fail-health-checks-while-an-unrouted-twin-stays-unused ec2__elbv2.albos-listener-rule-with-a-taken-priority-rejected ec2__elbv2.albos-listener-rule-with-six-path-patterns-rejected ec2__elbv2.albos-register-an-instance-id-into-an-ip-target-group-rejected ec2__elbv2.asghc-internal-nlb-round-trips-scheme-subnet-and-forwards-to-its-group ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered ec2__elbv2.ecsnt-internal-alb-listeners-forward-each-port-to-its-own-target-group ec2__elbv2.ecsnt-modify-listener-onto-an-occupied-port-rejected ec2__elbv2.ecsnt-target-group-of-one-alb-rejected-as-another-albs-listener-target
Issue-Key: ec2__elbv2.albos-health-check-matcher-code-above-499-rejected|_error#6|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2__elbv2.albos-ip-targets-a-listener-stops-routing-to-revert-to-unused-not-in-use|run#13|AssertionError|assert {ip: s for ip, (s, _r) in routed.items()} == want_unhealthy, (
Issue-Key: ec2__elbv2.albos-stale-ip-targets-behind-a-listener-fail-health-checks-while-an-unrouted-twin-stays-unused|run#16|AssertionError|assert states == (failed, failed), (
Issue-Key: ec2__elbv2.albos-listener-rule-with-a-taken-priority-rejected|run#10|AssertionError|assert got == ("PriorityInUse", "Priority '10' is currently in use"), (
Issue-Key: ec2__elbv2.albos-listener-rule-with-six-path-patterns-rejected|run#13|AssertionError|assert (code, message) == ("ValidationError", _VALUES_MSG), (
Issue-Key: ec2__elbv2.albos-register-an-instance-id-into-an-ip-target-group-rejected|_error#10|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: ec2__elbv2.asghc-internal-nlb-round-trips-scheme-subnet-and-forwards-to-its-group|run#13|AssertionError|assert "SecurityGroups" not in d, f"an NLB created without SGs reports some: {d.get('SecurityGroups')!r}"
Issue-Key: ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered|run#7|AssertionError|assert code == "ValidationError", f"registering {ghost} was answered {code!r}: {msg!r}"
Issue-Key: ec2__elbv2.ecsnt-internal-alb-listeners-forward-each-port-to-its-own-target-group|run#8|KeyError|fwd = act["ForwardConfig"]
Issue-Key: ec2__elbv2.ecsnt-modify-listener-onto-an-occupied-port-rejected|run#14|AssertionError|assert got == want, f"moving 8080 onto the occupied port 80 was answered {got!r}, expected {want!r}"
Issue-Key: ec2__elbv2.ecsnt-target-group-of-one-alb-rejected-as-another-albs-listener-target|_error#14|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# ec2__elbv2: 10 net-new ELBv2 root causes — four requests AWS refuses are accepted with HTTP 200, four refusal messages/codes are emulator-authored, routed IP targets report healthy, and forward actions/NLB security groups mis-shape

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
Ten distinct ELBv2 root causes surfaced by the ec2__elbv2 behavioral checks. None is covered by an open issue: 20261029T185946Z-elasticloadbalancing-elbv2-accepts-invalid-and-prohibited-requests-with-http lists nine OTHER operations/preconditions (DeleteLoadBalancer deletion protection, AddListenerCertificates, lambda-target validation, ModifyIpPools, CreateLoadBalancer SG, CreateListener protocol, source-ip legacy Values, redirect self-loop), 20261029T185946Z-elasticloadbalancing-elbv2-error-replies-carry-emulator-authored-messages-instead and 20261029T185946Z-elasticloadbalancing-elbv2-forward-actions-and-listener-mutual-authentication-omit and 20261029T185946Z-elasticloadbalancing-elbv2-load-balancer-and-target-health-payloads-drop are wire issues keyed to other replay units and do not carry any of the check ids below.

Cause 1 of 10: CreateTargetGroup accepts an HTTP matcher code above 499.
AWS: HTTP 400, Code 'ValidationError', Message "Health check matcher HTTP code '<the offending code>' must be within '200-499' inclusive" — fixed text, the quoted code taken from the request's Matcher.HttpCode; no target group is created (a later DescribeTargetGroups by that name answers TargetGroupNotFound, 'One or more target groups not found'). Smithy: CreateTargetGroup input member Matcher -> Matcher, whose HttpCode doc states "For Application Load Balancers, you can specify values between 200 and 499"; CreateTargetGroup models InvalidConfigurationRequestException/DuplicateTargetGroupNameException/TooManyTargetGroupsException (the recorded refusal is ValidationError).
Emulator: HTTP 200; the group is created and echoes Matcher {'HttpCode': '200,600'}, i.e. the out-of-range code is persisted.
Why: a client that relies on AWS validating the matcher ships a target group whose health check can never match, and sees it accepted in test and refused in production.
Fix: CreateTargetGroup input validation (matcher range check) in the operation's handler/validation layer.
Traces: ec2__elbv2.albos-health-check-matcher-code-above-499-rejected.

Cause 2 of 10: registered IP targets of a group a listener routes to are reported healthy, with no health check ever having succeeded.
AWS: DescribeTargetHealth for IP targets that answer nothing on the health-check port settles TargetHealth.State 'unhealthy' (Reason one of Target.Timeout / Target.FailedHealthChecks / Target.ResponseCodeMismatch — the state is the requirement, the reason varies), and the same addresses in a target group no listener references read 'unused' with Reason 'Target.NotInUse'. Smithy: TargetHealth.State -> TargetHealthStateEnum, Reason -> TargetHealthReasonEnum documenting Target.NotInUse for unused and Target.Timeout/Target.FailedHealthChecks for unhealthy.
Emulator: both routed groups' targets read ('healthy', no reason) forever — the emulator marks a registered target healthy on registration and never grades it against reachability or listener wiring.
Why: every health-driven workflow (diagnosis, replacement, alarming) is inverted: a broken estate looks healthy, so a client's remediation never triggers.
Fix: target-health evaluation in the target-group state/health component (grade registered targets, and drive 'unused'/Target.NotInUse off current listener references).
Traces: ec2__elbv2.albos-ip-targets-a-listener-stops-routing-to-revert-to-unused-not-in-use, ec2__elbv2.albos-stale-ip-targets-behind-a-listener-fail-health-checks-while-an-unrouted-twin-stays-unused.

Cause 3 of 10: the PriorityInUse refusal message is emulator-authored.
AWS: HTTP 400, Code 'PriorityInUse' (Smithy CreateRule errors include PriorityInUseException, awsQueryError PriorityInUse/400), Message "Priority '<priority>' is currently in use" — fixed text; the only variable part is the priority value from the request. The listener keeps exactly its prior rules.
Emulator: correct code, wrong text: "Priority '<priority>' is in use on listener '<listener ARN>'" — it appends a listener ARN AWS never names.
Why: clients and tooling that match on the refusal sentence (log scrapers, idempotency wrappers) do not recognise the refusal.
Fix: CreateRule's priority-collision error construction (message template).
Traces: ec2__elbv2.albos-listener-rule-with-a-taken-priority-rejected.

Cause 4 of 10: the rule condition-value limit message is emulator-authored.
AWS: HTTP 400, Code 'ValidationError', Message "A rule can only have '5' condition values and regex values" — entirely fixed text (the limit is quoted inside it); the rule's prior conditions are left untouched.
Emulator: correct code, wrong text: "A rule's conditions carry at most 5 values in total, got 6" — it states the submitted count, which AWS's message does not contain.
Fix: shared condition-value-limit validation used by CreateRule/ModifyRule (message template).
Traces: ec2__elbv2.albos-listener-rule-with-six-path-patterns-rejected.

Cause 5 of 10: DeregisterTargets accepts a malformed target id on an ip target group.
AWS: HTTP 400, Code 'ValidationError', Message "The IP address '<the submitted Id>' is not a valid IPv4 address" — fixed text with the id from the request. AWS applies the same shape validation to DeregisterTargets as to RegisterTargets: a never-valid id is refused, not treated as the no-op it grants a well-formed target that was simply never registered.
Emulator: HTTP 200 with an empty DeregisterTargetsResult — the instance id 'i-0123456789abcdef0' passed to an ip group is silently accepted.
Why: a client's cleanup path believes it removed a target it never could have, and never learns its id form is wrong.
Fix: DeregisterTargets target validation keyed on the group's TargetType, shared with RegisterTargets.
Traces: ec2__elbv2.albos-register-an-instance-id-into-an-ip-target-group-rejected.

Cause 6 of 10: an NLB created without SecurityGroups reports an empty SecurityGroups list.
AWS: DescribeLoadBalancers omits the SecurityGroups member entirely for a Network Load Balancer created with no security groups (Smithy LoadBalancer.SecurityGroups is an optional list, 'The IDs of the security groups for the load balancer'); an absent list and an empty list are different answers to 'does this NLB have security groups'.
Emulator: the member is present as [].
Why: SDK callers test membership ('SecurityGroups' in lb) to decide whether the NLB supports security groups at all; an empty list makes a group-less NLB look group-capable, and the complementary wire issue 20261029T185946Z-elasticloadbalancing-elbv2-load-balancer-and-target-health-payloads-drop reports the mirror-image defect on other members.
Fix: the DescribeLoadBalancers/CreateLoadBalancer serializer (omit, do not default, an unset optional list).
Traces: ec2__elbv2.asghc-internal-nlb-round-trips-scheme-subnet-and-forwards-to-its-group.

Cause 7 of 10: RegisterTargets' refusal for a nonexistent instance id uses the wrong error code and an emulator-authored message.
AWS: HTTP 400, Code 'ValidationError', Message "The following targets are not valid instances: '<instance id>'" — fixed prefix with the id(s) from the request; nothing is registered and DescribeTargetHealth for that id still answers (unused / Target.NotRegistered) rather than erroring. Smithy RegisterTargets models InvalidTargetException (awsQueryError InvalidTarget/400) among others, but the recorded refusal for an unknown instance is ValidationError.
Emulator: Code 'InvalidTarget', Message "The target 'i-0123456789abcdef0' is not valid: this account holds no instance with that id" — both the code and the sentence are the emulator's own.
Fix: RegisterTargets instance-resolution error classification and message (handler validation).
Traces: ec2__elbv2.asghc-register-nonexistent-instance-rejected-while-describe-reports-not-registered.

Cause 8 of 10: a forward action given only TargetGroupArn is not normalized into ForwardConfig.
AWS: CreateListener/DescribeListeners expand the shorthand into the full action: Type 'forward', TargetGroupArn echoed, plus ForwardConfig.TargetGroups = [{TargetGroupArn, Weight 1}] and ForwardConfig.TargetGroupStickinessConfig.Enabled false. Smithy: Action.ForwardConfig -> ForwardActionConfig {TargetGroups, TargetGroupStickinessConfig}, documented as the multi-target form of a forward action.
Emulator: the described action carries no ForwardConfig key at all (the check raises KeyError 'ForwardConfig' on the describe result).
Why: clients that read weights or stickiness off the normalized block (and every read-modify-write of a listener action) crash or silently drop the configuration.
Fix: forward-action normalization in the listener/rule action serializer, shared by Create/Modify/Describe.
Traces: ec2__elbv2.ecsnt-internal-alb-listeners-forward-each-port-to-its-own-target-group.

Cause 9 of 10: ModifyListener onto an occupied port answers the wrong code and message.
AWS: HTTP 400, Code 'ValidationError', Message "Listener port '<port>' is already in use" — fixed text with the port from the request; both listeners are left unchanged. (ModifyListener does model DuplicateListenerException/awsQueryError DuplicateListener, but the recorded refusal for moving a listener onto a port another listener of the same load balancer holds is ValidationError with that sentence.)
Emulator: Code 'DuplicateListener', Message "A listener already exists on port 80 of load balancer '<lb ARN>'" — a different code and an emulator-authored sentence naming the load balancer ARN.
Fix: ModifyListener port-collision validation: error classification + message template.
Traces: ec2__elbv2.ecsnt-modify-listener-onto-an-occupied-port-rejected.

Cause 10 of 10: CreateListener accepts a target group already forwarded to by another load balancer.
AWS: HTTP 400, Code 'TargetGroupAssociationLimit' (Smithy CreateListener errors include TargetGroupAssociationLimitException, awsQueryError TargetGroupAssociationLimit/400), Message 'The following target groups cannot be associated with more than one load balancer: <target group ARN(s) from the request>' — fixed prefix with the ARNs from the request. The second load balancer gains no listener and the group's LoadBalancerArns stays the first load balancer alone; after the first listener is deleted the identical CreateListener succeeds.
Emulator: HTTP 200 — the second ALB's listener is created against the already-associated group.
Why: the one-load-balancer-per-target-group rule is a hard AWS constraint; automation validated against the emulator builds topologies AWS rejects.
Fix: CreateListener (and ModifyListener) default-action validation against the target group's existing load-balancer association.
Traces: ec2__elbv2.ecsnt-target-group-of-one-alb-rejected-as-another-albs-listener-target.

## Evidence
Cause 1: expected ('ValidationError', "Health check matcher HTTP code '600' must be within '200-499' inclusive"); got HTTP 200 and a created group whose Matcher is {'HttpCode': '200,600'} ("real AWS rejects this call, but it SUCCEEDED and returned {'TargetGroups': [... 'Matcher': {'HttpCode': '200,600'} ...]}").
Cause 2: expected {'10.80.4.37': 'unhealthy', '10.80.8.112': 'unhealthy'} for both routed groups and (unused, Target.NotInUse) for the unrouted twin; got {'10.80.4.37': ('healthy', None), '10.80.8.112': ('healthy', None)} in both routed groups ("routed stale targets should settle unhealthy", "every routed stale target should be unhealthy, got dashboards {...healthy...}, api {...healthy...}").
Cause 3: expected ('PriorityInUse', "Priority '10' is currently in use"); got ('PriorityInUse', "Priority '10' is in use on listener 'arn:aws:elasticloadbalancing:...:listener/app/<name>/<id>/<id>'"). The quoted 10 is the request's Priority; the listener ARN is not part of AWS's message at all.
Cause 4: expected ('ValidationError', "A rule can only have '5' condition values and regex values"); got ('ValidationError', "A rule's conditions carry at most 5 values in total, got 6").
Cause 5: expected ('ValidationError', "The IP address 'i-0123456789abcdef0' is not a valid IPv4 address"); got HTTP 200 ("the call was expected to be REFUSED but it succeeded: {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}").
Cause 6: expected the SecurityGroups member to be ABSENT from the described NLB; got SecurityGroups = [] ("an NLB created without SGs reports some: []").
Cause 7: expected ('ValidationError', "The following targets are not valid instances: 'i-0123456789abcdef0'"); got ('InvalidTarget', "The target 'i-0123456789abcdef0' is not valid: this account holds no instance with that id").
Cause 8: expected DefaultActions[0].ForwardConfig = {'TargetGroups': [{'TargetGroupArn': <tg>, 'Weight': 1}], 'TargetGroupStickinessConfig': {'Enabled': False}}; got no ForwardConfig key (KeyError: 'ForwardConfig').
Cause 9: expected ('ValidationError', "Listener port '80' is already in use"); got ('DuplicateListener', "A listener already exists on port 80 of load balancer 'arn:aws:elasticloadbalancing:...:loadbalancer/app/<name>/<id>'").
Cause 10: expected ('TargetGroupAssociationLimit', 'The following target groups cannot be associated with more than one load balancer: <tg arn>'); got a 200 success ("the call succeeded; real AWS rejects it, so there is no error code").
ARNs, ids, names and ports above are the values of one replay; the fixed text of each message is the requirement.

## Steps to Reproduce
Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the ec2__elbv2 checks listed in trace_ids. Per cause: (1) CreateTargetGroup Protocol=HTTPS Port=443 TargetType=ip Matcher={'HttpCode':'200,600'}; (2) register two unreachable in-VPC IP targets into a group, attach an ALB listener forwarding to it, poll DescribeTargetHealth; (3) CreateRule twice at Priority=10 on one listener; (4) ModifyRule growing a path-pattern condition to six values; (5) DeregisterTargets {'Id':'i-0123456789abcdef0'} on an ip target group; (6) CreateLoadBalancer Type=network with no SecurityGroups, then DescribeLoadBalancers; (7) RegisterTargets {'Id':'i-0123456789abcdef0'} on an instance group in an account with no such instance; (8) CreateListener with DefaultActions=[{'Type':'forward','TargetGroupArn':<tg>}] then DescribeListeners; (9) two listeners on ports 80 and 8080 of one ALB, then ModifyListener(8080 -> Port=80); (10) CreateListener on a second ALB forwarding to a target group already forwarded to by the first ALB's listener.
