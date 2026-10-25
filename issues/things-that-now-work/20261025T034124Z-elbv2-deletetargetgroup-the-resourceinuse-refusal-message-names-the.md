Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.e2e-target-group-deletion-while-referenced-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.e2e-target-group-deletion-while-referenced-rejected|run#21|AssertionError|assert message == expected_in_use, f"expected message {expected_in_use!r}, got {message!r}"
Issue-Commit: 95f850a4c04866a54d7ccecba673fc8b3153145d

# ELBv2 DeleteTargetGroup: the ResourceInUse refusal message names the holding listener ARN instead of AWS's generic "in use by a listener or a rule"

> _Note: this issue was identified against the code base at commit `95f850a4c04866a54d7ccecba673fc8b3153145d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTargetGroup

## Description
One root cause.

Cause 1 of 1 — the ResourceInUse message emitted when DeleteTargetGroup is refused for a still-referenced target group is built on a different rule than AWS's.

AWS: Smithy DeleteTargetGroup(TargetGroupArn required) -> 200 empty; its ONE modeled error is ResourceInUseException (errorKind client, httpError 400, awsQueryError code ResourceInUse, member Message: ErrorDescription; shape doc "A specified resource is in use."). The model states only the code and status, so the message is characterised from the recorded behavior: AWS composes it from the REQUEST — the constant prefix "Target group '", the request's own TargetGroupArn, then the invariant phrase "' is currently in use by a listener or a rule". The phrase is deliberately non-specific: AWS names NO listener, rule or other server-side identifier, whichever reference is holding the group. Recorded example (ARNs are environment-specific): "Target group 'arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-inuse-tg-.../8be12a...' is currently in use by a listener or a rule".

Emulator: the status and the error code are correct — 400 / ResourceInUse — but the message substitutes the ARN of the specific listener whose default action holds the reference for the invariant phrase: "Target group '<tg-arn>' is currently in use by listener '<listener-arn>'". So the emulator's message rule is "prefix + request ARN + the matched listener's generated ARN", disclosing a server-generated id that AWS's message never contains, and dropping the "...or a rule" wording that tells a caller the reference may be a rule rather than a listener.

Why: a client that asserts on, logs or parses the refusal text for this very common guard (delete a target group that is still wired to a listener) sees a message of a different shape than AWS ever returns, and one that varies with a generated listener ARN rather than being a fixed function of the request. Tooling that diffs expected-vs-actual error text, or that treats the message as stable for a given request, diverges; and the emulator reveals which listener holds the reference where AWS deliberately does not, so code written against the emulator can come to depend on an id AWS will not supply.

Fix: the DeleteTargetGroup handler's in-use reference check in the elasticloadbalancingv2 service crate — the branch that formats the ResourceInUseException message after finding a referencing listener/rule. It must emit the request-derived generic form and not interpolate the matched listener's ARN.

Note on overlap: open issue 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md (Cause 3 of 10) carries this check's id for the EARLIER symptom — DeleteTargetGroup answering 501 NotImplemented, keyed on the `assert code == "ResourceInUse"` step. That step now passes: the operation is implemented and refuses with the modeled code. The failure reported here is the next assertion in the same check (the message), which that issue's replay key does not cover.

Traces: elbv2.e2e-target-group-deletion-while-referenced-rejected (the check fails at the message assertion; the remaining steps of that check — repointing the listener's default action with ModifyListener, the positive-control delete and the TargetGroupNotFound follow-up — are not reached and are not part of this report).

## Evidence
Check elbv2.e2e-target-group-deletion-while-referenced-rejected, verdict mismatch.

Sequence: CreateVpc + 2 subnets -> CreateLoadBalancer (internal, application) -> wait State.Code=active -> CreateTargetGroup(Protocol=HTTP, Port=80, TargetType=ip) -> CreateListener(Protocol=HTTP, Port=80, DefaultActions=[{Type: forward, TargetGroupArn: tg}]) -> DeleteTargetGroup(TargetGroupArn=tg).

Status/code: AWS 400 ResourceInUse; emulator 400 ResourceInUse — these AGREE and are not the divergence.

Message:
Expected (AWS rule: constant + request TargetGroupArn + invariant phrase):
  "Target group '<TargetGroupArn from the request>' is currently in use by a listener or a rule"
  recorded example: Target group 'arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-inuse-tg-a71c66235502/8be12a75a7b9e06d' is currently in use by a listener or a rule
Actual (emulator):
  "Target group '<TargetGroupArn from the request>' is currently in use by listener '<ARN of the referencing listener>'"
  observed: Target group 'arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-inuse-tg-a71c66235502/8be12a75a7b9e06d' is currently in use by listener 'arn:aws:elasticloadbalancing:us-west-2:000000000000:listener/app/sim2real-inuse-alb-59aebf0a3a7a/d1873f1fbbc3fb28/b98c1bd4970cf8e1'

Replay excerpt:
  AssertionError: expected message "Target group '...' is currently in use by a listener or a rule", got "Target group '...' is currently in use by listener 'arn:aws:elasticloadbalancing:...:listener/app/...'"

(Account id, region, generated names and the listener/target-group id suffixes above are environment-specific and are not requirements; the message TEMPLATE is.)

Model basis: DeleteTargetGroup errors = [ResourceInUseException] only; ResourceInUseException = client error, httpError 400, awsQueryError ResourceInUse, members {Message: ErrorDescription}.

## Steps to Reproduce
1. ec2.create_vpc(CidrBlock="10.95.0.0/16"); create two subnets in two available AZs.
2. elbv2.create_load_balancer(Name=<unique>, Subnets=[both subnets], Scheme="internal", Type="application"); poll describe_load_balancers until State.Code == "active".
3. elbv2.create_target_group(Name=<unique>, Protocol="HTTP", Port=80, VpcId=<vpc>, TargetType="ip") -> tg_arn.
4. elbv2.create_listener(LoadBalancerArn=<lb>, Protocol="HTTP", Port=80, DefaultActions=[{"Type": "forward", "TargetGroupArn": tg_arn}]).
5. elbv2.delete_target_group(TargetGroupArn=tg_arn) and read Error.Code and Error.Message from the ClientError.
   - Expected: Code "ResourceInUse" (HTTP 400) and Message == "Target group '" + tg_arn + "' is currently in use by a listener or a rule".
   - Observed: Code "ResourceInUse" (correct) but Message == "Target group '" + tg_arn + "' is currently in use by listener '<listener arn>'" — the generic "a listener or a rule" phrase is replaced by the referencing listener's generated ARN.
