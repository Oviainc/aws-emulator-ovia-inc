Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-control-config-create-control-panel-nonexistent-cluster-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-control-config-create-control-panel-nonexistent-cluster-not-found route53-recovery-control-config-validation-error-paths
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config|CreateControlPanel|4xx->4xx|$.message
Issue-Key: route53-recovery-control-config|CreateControlPanel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|CreateControlPanel|4xx->4xx|status
Issue-Key: route53-recovery-control-config|CreateCluster|4xx->4xx|$.message
Issue-Key: route53-recovery-control-config|CreateCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|DescribeCluster|4xx->4xx|$.message
Issue-Key: route53-recovery-control-config|DescribeCluster|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|DescribeCluster|4xx->4xx|status
Issue-Commit: e2d4bffd2fe68cddce7016612710a92983157233

# route53-recovery-control-config wire error class and status divergences on cluster rejection paths

> _Note: this issue was identified against the code base at commit `e2d4bffd2fe68cddce7016612710a92983157233`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateControlPanel

## Description
Three distinct root causes in the route53-recovery-control-config emulator, all on rejection paths of the cluster / control-panel surface, all found by the wire corpus replay (score 16.7%, 5 of 6 steps mismatched). They share one service and one general theme — the rejection path selects the wrong error class, and therefore the wrong HTTP status, for three different kinds of refusal — but each needs its own fix and must be closed separately. THREE causes, numbered below.

=== Cause 1 of 3: CreateControlPanel naming a nonexistent ClusterArn answers HTTP 404 instead of HTTP 400 ===
AWS: recorded behavior only. CreateControlPanel with a ClusterArn that does not exist is refused at HTTP 400 on the create path (the fixture asserts the status and the request-derived message and deliberately does not assert the error code, so the status 400 is the requirement here, not a particular code). The message is built from the request and names the referenced cluster; recorded example: 'Cluster arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000 does not exist.' Smithy: CreateControlPanel models both ValidationException (httpError 400) and ResourceNotFoundException (httpError 404), so the model alone does not pick one; the recording does, and it picks the 400 class. Note the read path differs — DescribeCluster on a nonexistent cluster is the 404 class — so the emulator cannot share one lookup error between the two paths.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message of the form 'no cluster is stored under <arn>'.
Why: a client distinguishing a bad request parameter (4xx that retry never fixes, and which SDK code catches as the validation class) from a missing resource takes the wrong branch; CloudFormation/Terraform-style create logic that treats 404 on a parent reference as "create the parent and retry" loops instead of failing fast, and code written against AWS that catches the 400 class sees an uncaught ResourceNotFoundException.
Fix: in the CreateControlPanel handler's parent-cluster lookup, return the 400 validation error class instead of ResourceNotFoundException for an unknown ClusterArn, keeping the request-derived 'Cluster <arn> does not exist.' wording shape.
Traces: route53-recovery-control-config-create-control-panel-nonexistent-cluster-not-found#task#1.

=== Cause 2 of 3: CreateCluster enum violation on NetworkType answers ValidationException instead of BadRequestException ===
AWS: recorded behavior only. A CreateCluster request whose NetworkType is not a member of the enum is refused at HTTP 400 with error code BadRequestException — a front-door request-body validation class — not with the modeled ValidationException. The message is built from the request and quotes the offending value and the permitted set; recorded example: 'Invalid request body : [instance value ("NOTAVALIDTYPE") not found in enum (possible values: ["IPV4","DUALSTACK"])]'. Smithy: CreateCluster.NetworkType -> NetworkType enum with values [IPV4, DUALSTACK]; CreateCluster's modeled error list is AccessDenied/Conflict/InternalServer/ResourceNotFound/ServiceQuotaExceeded/Throttling/ValidationException — BadRequestException is NOT modeled, which is why the emulator reaches for the generic modeled class.
Emulator: answers HTTP 400 (status is correct) but with x-amzn-errortype: ValidationException and a constraint-style message, e.g. "1 validation error detected: Value 'NOTAVALIDTYPE' at 'networkType' failed to satisfy constraint: Member must satisfy enum value set: [IPV4, DUALSTACK]".
Why: botocore exposes BadRequestException as its own exception class, so every client that branches on it (negative tests, retry classifiers, error-mapping layers) fails to match and either falls through to a generic handler or crashes; the operation is indistinguishable on the wire from a modeled validation failure.
Fix: the service's input-constraint validation layer must map request-body enum (and other shape-constraint) violations for this service to a BadRequestException response at 400 carrying the 'Invalid request body : [...]' form, instead of emitting the generic modeled ValidationException. Related but NOT the same input path as Cause 2 of open issue 20261022T140811Z-route53-recovery-control-config-wrong-error-classes-on.md, which covers the ClusterName @pattern violation; that issue carries no wire replay key for this enum step, so this cause is filed here.
Traces: route53-recovery-control-config-validation-error-paths#task#2.

=== Cause 3 of 3: DescribeCluster on a cluster ARN in another account answers 404 ResourceNotFoundException instead of 403 AccessDeniedException ===
AWS: recorded behavior only. DescribeCluster against a cluster ARN whose account id is not the caller's account is refused at HTTP 403 with error code AccessDeniedException — the authorization check runs before (and independently of) the existence check, so a cross-account ARN never reveals whether the resource exists. The message is request-derived and names the principal, the action and the resource; recorded example: 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: route53-recovery-control-config:DescribeCluster on resource: arn:aws:route53-recovery-control::123456789012:cluster/00000000-0000-0000-0000-000000000000 because no resource-based policy allows the route53-recovery-control-config:DescribeCluster action'. Smithy: DescribeCluster models AccessDeniedException (httpError 403, doc '403 response - You do not have sufficient access to perform this action') alongside ResourceNotFoundException (404); ClusterArn is an @httpLabel on GET /cluster/{ClusterArn}.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message 'no cluster is stored under <arn>' — it looks the ARN up in its own store and never inspects the ARN's account id.
Why: a client that classifies 403 as a permissions problem (stop, surface a credentials/policy error) and 404 as "resource gone" (treat as deleted, proceed with teardown, or create it) takes the opposite action; multi-account tooling that probes a peer account's cluster concludes the cluster was deleted rather than that it lacks access, and the emulator also cannot be used to exercise any AccessDeniedException handling for this service.
Fix: add an account-ownership check to the DescribeCluster handler (ideally the shared ARN-resolution step for cluster ARNs) that compares the ARN's account id against the caller's account and returns the 403 AccessDeniedException class — with the 'User: <principal> is not authorized to perform: <action> on resource: <arn>' message shape — before the existence lookup runs.
Traces: route53-recovery-control-config-validation-error-paths#task#3.

All three divergences were reported with empty stub_path and empty owned_by, so none is blocked by an unimplemented operation and none is owned by an existing issue's replay key. state_reset_failures was 0, so no cause here is contaminated by leftover fixture state. The only neighbouring open issue, 20261022T140811Z-route53-recovery-control-config-wrong-error-classes-on.md, is behavioral-corpus derived and does not carry the wire fixture/step keys for any of these three steps.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0; 6 steps, 1 passed, 5 mismatched, score 16.7%).

Cause 1 — fixture route53-recovery-control-config-create-control-panel-nonexistent-cluster-not-found, step task#1, operation CreateControlPanel:
  status: expected 400, got 404
  $.message: expected 'Cluster arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000 does not exist.' (example wording), got 'no cluster is stored under arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000'
  $headers.x-amzn-errortype: expected <shaped-not-asserted>, got 'ResourceNotFoundException'

Cause 2 — fixture route53-recovery-control-config-validation-error-paths, step task#2, operation CreateCluster:
  status: 400 expected and 400 observed (status matches)
  $headers.x-amzn-errortype: expected 'BadRequestException', got 'ValidationException'
  $.message: expected 'Invalid request body : [instance value ("NOTAVALIDTYPE") not found in enum (possible values: ["IPV4","DUALSTACK"])]' (example wording), got "1 validation error detected: Value 'NOTAVALIDTYPE' at 'networkType' failed to satisfy constraint: Member must satisfy enum value set: [IPV4, DUALSTACK]"

Cause 3 — fixture route53-recovery-control-config-validation-error-paths, step task#3, operation DescribeCluster:
  status: expected 403, got 404
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'ResourceNotFoundException'
  $.message: expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: route53-recovery-control-config:DescribeCluster on resource: arn:aws:route53-recovery-control::123456789012:cluster/00000000-0000-0000-0000-000000000000 because no resource-based policy allows the route53-recovery-control-config:DescribeCluster action' (example wording), got 'no cluster is stored under arn:aws:route53-recovery-control::123456789012:cluster/00000000-0000-0000-0000-000000000000'

Model references used (services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json):
  CreateControlPanel errors include ValidationException(400) and ResourceNotFoundException(404); ClusterArn required.
  CreateCluster.NetworkType -> NetworkType enum values [IPV4, DUALSTACK]; BadRequestException is not modeled for CreateCluster.
  DescribeCluster GET /cluster/{ClusterArn}, errors include AccessDeniedException(403, '403 response - You do not have sufficient access to perform this action') and ResourceNotFoundException(404).

In every cause the diverging HTTP status and error code are the requirements; the quoted messages are recorded examples except where the rule is request-derived (the enum message names the member value and the permitted set; the access-denied message names principal, action and resource).

## Steps to Reproduce
Replay the two wire fixtures against the emulator's route53-recovery-control-config endpoint:

1. Cause 1 — fixture route53-recovery-control-config-create-control-panel-nonexistent-cluster-not-found, step task#1: POST /controlpanel with ClusterArn='arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000' (a cluster that was never created) and any valid ControlPanelName. Observe HTTP 404 / x-amzn-errortype ResourceNotFoundException; expected HTTP 400 with a 'Cluster <arn> does not exist.'-style message.

2. Cause 2 — fixture route53-recovery-control-config-validation-error-paths, step task#2: POST /cluster with a valid ClusterName and NetworkType='NOTAVALIDTYPE'. Observe HTTP 400 with x-amzn-errortype ValidationException; expected HTTP 400 with x-amzn-errortype BadRequestException and an 'Invalid request body : [instance value ("NOTAVALIDTYPE") not found in enum (possible values: ["IPV4","DUALSTACK"])]' body-validation message.

3. Cause 3 — fixture route53-recovery-control-config-validation-error-paths, step task#3: GET /cluster/arn%3Aaws%3Aroute53-recovery-control%3A%3A123456789012%3Acluster%2F00000000-0000-0000-0000-000000000000 — i.e. DescribeCluster on a cluster ARN belonging to account 123456789012 while the caller is account 000000000000. Observe HTTP 404 / ResourceNotFoundException; expected HTTP 403 / AccessDeniedException with the 'User: <principal> is not authorized to perform: route53-recovery-control-config:DescribeCluster on resource: <arn>' message.

Then re-run replay_wire_corpus(): both fixtures must replay clean (all three steps above passing) before this issue is closed.
