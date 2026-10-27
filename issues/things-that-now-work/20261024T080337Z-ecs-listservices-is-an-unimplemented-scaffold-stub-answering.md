Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected
Issue-Key: ec2__ecs__elbv2|ListServices|NotImplemented
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ECS ListServices is an unimplemented scaffold stub answering 501 NotImplemented, blocking the ECS deployment-controller / load-balancer-binding negative checks

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
Cause 1 of 1 — ECS ListServices is still a scaffold stub, so every flow that enumerates the services of a cluster dies on it.
AWS: Smithy com.amazonaws.ecs#ListServices is a readonly, paginated operation (inputToken/outputToken nextToken, items serviceArns, pageSize maxResults). Input {cluster, nextToken, maxResults (1..100, default page 10), launchType, schedulingStrategy, resourceManagementType}; output {serviceArns: StringList of full service ARNs, nextToken}. It answers HTTP 200 with the ARNs of the services associated with the named cluster (the default cluster when `cluster` is omitted), and its only failure modes are the modeled ones: AccessDeniedException (403), ClientException (400), ClusterNotFoundException (400), InvalidParameterException (400), ServerException (500). A 501 is not among them.
Emulator: every call is refused with `An error occurred (NotImplemented) when calling the ListServices operation: operation ListServices is scaffolded but not implemented in this emulator`. services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_services.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("ListServices"))`.
Why: ListServices is how every client (the AWS CLI's `ecs list-services`, the console, CDK/Terraform refreshers, and any teardown that must find services before deleting a cluster) discovers what exists in a cluster. A 501 is not a modeled outcome, so SDK retry/error handling cannot classify it; the call does not fail the way any AWS failure fails. Here it also masks the three ECS CreateService negative behaviours the checks exist to grade (a CODE_DEPLOY deployment controller with no load balancer, a load-balancer binding naming a container the task definition does not declare, and a target group with no load balancer attached): those assertions are never reached, so the emulator's CreateService validation on those paths is unverified.
Fix: implement the handler in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_services.rs — enumerate the service records in the service state, resolve `cluster` (short name or ARN, defaulting to `default`) and raise ClusterNotFoundException for an unknown one, apply the launchType / schedulingStrategy / resourceManagementType filters, page on maxResults with an opaque nextToken, and delete the SCAFFOLDED marker line.
Traces: ec2__ecs__elbv2.ecsnt-code-deploy-service-without-a-load-balancer-rejected, ec2__ecs__elbv2.ecsnt-service-load-balancer-binding-outside-the-task-definition-rejected, ec2__ecs__elbv2.ecsnt-service-on-a-target-group-without-a-load-balancer-rejected.

## Evidence
Expected: HTTP 200 ListServicesResponse with serviceArns (and nextToken when paginating), or one of the modeled errors (ClusterNotFoundException 400, InvalidParameterException 400, AccessDeniedException 403, ClientException 400, ServerException 500).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListServices operation: operation ListServices is scaffolded but not implemented in this emulator — raised identically in all three checks (botocore client.py _make_api_call), so none of them reached its CreateService assertions.
Code citation: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_services.rs lines 3-12 (SCAFFOLDED marker) and line 190 (`Err(crate::scaffolded::Scaffolded::not_implemented("ListServices"))`).

## Steps to Reproduce
1. CreateCluster(clusterName='<unique>').
2. RegisterTaskDefinition for a trivial container, then CreateService in that cluster.
3. Call ListServices(cluster='<unique>') (e.g. `aws ecs list-services --cluster <unique>`) -> the emulator answers 501 NotImplemented instead of 200 with the service's ARN in serviceArns.
4. The same 501 is returned for ListServices with no arguments (default cluster) and for a cluster that does not exist, where AWS answers 200 with an empty list / 400 ClusterNotFoundException respectively.
