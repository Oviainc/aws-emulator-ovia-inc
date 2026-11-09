Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__vpc-lattice.in-vpc-http-get-through-service-network-reaches-lambda-target lambda__vpc-lattice.auth-policy-deny-overrides-a-working-in-vpc-invoke
Issue-Key: lambda__vpc-lattice|RegisterTargets|NotImplemented
Issue-Commit: 22941ee6d43df66d29bcadd7a62b7906e5868e84

# lambda__vpc-lattice: vpc-lattice RegisterTargets answers NotImplemented, so no Lambda target can be attached to a target group and the whole in-VPC service-network data plane (and its auth-policy enforcement) is unreachable

> _Note: this issue was identified against the code base at commit `22941ee6d43df66d29bcadd7a62b7906e5868e84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTargets

## Description
Cause 1 of 1: vpc-lattice RegisterTargets is a scaffolded stub that answers an error instead of registering targets, so both cross-service Lambda + VPC Lattice data-plane checks abort at their first register_targets call. Failing check ids: lambda__vpc-lattice.in-vpc-http-get-through-service-network-reaches-lambda-target, lambda__vpc-lattice.auth-policy-deny-overrides-a-working-in-vpc-invoke. Note: the open issue 20261030T191436Z-vpc-lattice-registertargets-stub.md names the same operation, but the replay report scoped it to the single vpc-lattice-suite check it blocks and did not mark these two lambda__vpc-lattice checks as covered; they are recorded here so a fixer re-replays them too, since they exercise the data-plane consequences (Lambda invocation through a service network and SigV4 auth-policy enforcement) that the control-plane check does not.

AWS: Smithy (services/service-com-amazonaws-vpclattice-mercurycontrolplane/smithy-model/com-amazonaws-vpclattice-mercurycontrolplane.json) models RegisterTargets as POST /targetgroups/{targetGroupIdentifier}/registertargets returning HTTP 200 with output members `successful` (TargetList) and `unsuccessful` (TargetFailureList); input targetGroupIdentifier is required with @length 17..2048 and @pattern ^((tg-[0-9a-z]{17})|(arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:targetgroup/tg-[0-9a-z]{17}))$, and `targets` is required with @length 1..100. Its only modeled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ServiceQuotaExceededException (402), ThrottlingException (429) and ValidationException (400); NotImplemented is not among them. For a LAMBDA target group, registering the function ARN must return it under `successful`, after which the recordings show the service network routing an in-VPC HTTP GET through the HTTP listener to the Lambda target and returning the target's own body unchanged, and — with AWS_IAM auth and a service auth policy whose Effect is Deny for the caller's principal — the same request answering HTTP 403 with body 'AccessDeniedException: User: <assumed-role-arn> is not authorized to perform: vpc-lattice-svcs:Invoke on resource: <service-arn><path> with an explicit deny in a service-based policy' (fixed text 'AccessDeniedException: User: ', ' is not authorized to perform: vpc-lattice-svcs:Invoke on resource: ' and ' with an explicit deny in a service-based policy'; the principal ARN, service ARN and request path are values from the caller and the request).

Emulator: the call is refused — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterTargets operation: operation RegisterTargets is scaffolded but not implemented in this emulator. The handler is the scaffold stub services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/register_targets.rs.

Why: a client cannot attach any target to a VPC Lattice target group, so nothing is reachable through a service network: an in-VPC caller gets no response from the service's DNS name, and auth-policy allow/deny decisions can never be observed. NotImplemented is not a modeled error for this operation, so an SDK retry/error-handling path keyed on the modeled errors mishandles it outright.

Fix: implement the RegisterTargets handler (services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/register_targets.rs) against target-group state so an accepted target is returned under `successful` and persisted for ListTargets/DeregisterTargets, and so the data plane routes a request through the listener to the registered Lambda target, applying the service's auth policy to the SigV4 principal.

Traces: lambda__vpc-lattice.in-vpc-http-get-through-service-network-reaches-lambda-target, lambda__vpc-lattice.auth-policy-deny-overrides-a-working-in-vpc-invoke.

## Evidence
Check lambda__vpc-lattice.in-vpc-http-get-through-service-network-reaches-lambda-target
Expected (AWS): vl.register_targets(targetGroupIdentifier=<tg-id>, targets=[{'id': <lambda-arn>}]) returns HTTP 200 with {t['id'] for t in response['successful']} == {<lambda-arn>}; the in-VPC driver Lambda's HTTP GET of the service's DNS name then returns status 200 with the target's own body '<nonce>:<path>'.
Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterTargets operation: operation RegisterTargets is scaffolded but not implemented in this emulator

Check lambda__vpc-lattice.auth-policy-deny-overrides-a-working-in-vpc-invoke
Expected (AWS): the same register_targets call returns 200 with the ARN under `successful`; with an Allow auth policy the SigV4-signed in-VPC GET returns 200 and the target's body, and after PutAuthPolicy with an explicit Deny for the driver role the same GET returns 403 with body 'AccessDeniedException: User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: vpc-lattice-svcs:Invoke on resource: <service-arn><path> with an explicit deny in a service-based policy'.
Actual (emulator): ClientError (NotImplemented) on RegisterTargets: operation RegisterTargets is scaffolded but not implemented in this emulator

Modeled contract contradicted: RegisterTargets output members `successful`/`unsuccessful` with HTTP 200; modeled errors are AccessDeniedException, ConflictException, InternalServerException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException and ValidationException — NotImplemented is not one of them.

## Steps to Reproduce
1. Point a boto3/botocore vpc-lattice client at the emulator.
2. Create a LAMBDA target group: tg = vl.create_target_group(name='tg1', type='LAMBDA', config={'lambdaEventStructureVersion': 'V1'}).
3. Call vl.register_targets(targetGroupIdentifier=tg['id'], targets=[{'id': '<lambda-function-arn>'}]).
   Observe ClientError NotImplemented 'operation RegisterTargets is scaffolded but not implemented in this emulator' instead of a 200 whose `successful` list carries the function ARN.
4. Replay the two behavioral checks listed in Issue-Trace-Ids; both abort at this call, leaving the in-VPC data-plane GET and the auth-policy Allow/Deny flip unexercised.
