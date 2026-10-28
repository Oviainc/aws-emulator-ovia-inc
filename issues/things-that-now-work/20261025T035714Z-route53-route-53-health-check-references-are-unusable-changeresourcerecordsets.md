Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.health-check-delete-allowed-while-referenced route53.deploy-health-check-cfn-status-behavior
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|ChangeResourceRecordSets|NoSuchHealthCheck
Issue-Key: route-53|GetHealthCheck|NoSuchHealthCheck
Issue-Commit: 2db38af76302cd70705574627196925040cef364

# Route 53 health-check references are unusable: ChangeResourceRecordSets refuses every HealthCheckId with NoSuchHealthCheck, and CFN AWS::Route53::HealthCheck is never provisioned (2 causes)

> _Note: this issue was identified against the code base at commit `2db38af76302cd70705574627196925040cef364`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeResourceRecordSets

## Description
Two net-new root causes in the route53 emulator, both about a health check that EXISTS being unreachable from the places that reference it. CreateHealthCheck is implemented in this build (it returns a HealthCheck.Id and stores it in `store.health_checks`), so neither cause is covered by the open stub issue 20261024T114159Z-route53-createhealthcheck-stub.md.

Cause 1 of 2: ChangeResourceRecordSets refuses ANY record set that carries a HealthCheckId, without ever consulting the health-check store.
AWS: ResourceRecordSet.HealthCheckId is the documented way to make a record set conditional on a health check ("include the HealthCheckId element and specify the ID of the applicable health check"); Smithy: ChangeResourceRecordSets models NoSuchHealthCheck (HTTP 404) only for an ID that names no health check of the account, and the operation otherwise returns 200 with ChangeInfo PENDING. A HealthCheckId freshly returned by CreateHealthCheck is a valid reference and the change must be accepted.
Emulator: for every batch containing a non-empty HealthCheckId the handler returns HTTP 404 NoSuchHealthCheck unconditionally — the store is never looked up. services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs refuses on the first non-empty `health_check_id` in the batch with the comment "this emulator mints none (CreateHealthCheck is not implemented)", which is stale now that CreateHealthCheck is implemented. Recorded example message: "no health check exists with the ID 6de0bd49-2070-7764-6e56-4bf63dd5ce8a" for an id CreateHealthCheck had just minted in the same run.
Why: health-check-backed DNS is impossible on this emulator. Every weighted/failover/latency record set with a health check — the whole DNS-failover pattern — is rejected with a 404 the client reads as "your health check does not exist", so callers retry or recreate the health check forever. It also hides the behavior the check is about (Route 53 deliberately permits deleting a referenced health check and leaves the record set with a dangling HealthCheckId).
Fix: the HealthCheckId validation in ChangeResourceRecordSets' handler (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/change_resource_record_sets.rs) — resolve each referenced id against the health-check store and refuse with NoSuchHealthCheck only when it is absent; persist the HealthCheckId on the stored record set so ListResourceRecordSets echoes it, and leave an existing reference intact when the health check is later deleted.
Traces: route53.health-check-delete-allowed-while-referenced.

Cause 2 of 2: CloudFormation does not provision AWS::Route53::HealthCheck into Route 53 state; Ref yields a fabricated CFN physical id.
AWS: Ref on an AWS::Route53::HealthCheck resource returns the Route 53 health check ID, which is why templates export `Value: !Ref HealthCheck` and feed it to route53 GetHealthCheck / GetHealthCheckStatus. Smithy: GetHealthCheck.HealthCheckId targets HealthCheckId (@length max 64) and the created health check must be readable there (HTTP 200) with the HealthCheckConfig the template declared; NoSuchHealthCheck (HTTP 404) is reserved for an id that names nothing.
Emulator: the stack reaches CREATE_COMPLETE but the Ref value is a synthesized CloudFormation-style physical name of the form "<stack-name>-<logical-id>-<random>" (recorded example: sim2real-r53-cfn-e9c58310712b-HealthCheck-51481AACD40C), and no health check exists in route53 state: GetHealthCheck on that value answers HTTP 404 NoSuchHealthCheck (recorded message "no health check exists with the ID sim2real-r53-cfn-e9c58310712b-HealthCheck-51481AACD40C"). This is the same class of defect as open issue 20261025T023940Z-route53-cloudformation-ref-on-aws-route53-hostedzone-still-yields.md but for a different resource type (that issue covers AWS::Route53::HostedZone only and does not create or Ref a health check).
Why: a CREATE_COMPLETE stack reports success while no health check was created and nothing downstream can use the Ref: any template that attaches a health check to a record set, alarms on it, or exports its id hands consumers a value every route53 call 404s. The stack looks healthy, so the failure only surfaces at first use.
Fix: the AWS::Route53::HealthCheck resource provider in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src) — provision the resource by calling Route 53 CreateHealthCheck with the template's HealthCheckConfig, record the returned Id as the resource's physical id / Ref value, apply stack-update property changes through UpdateHealthCheck, and delete it on stack delete.
Traces: route53.deploy-health-check-cfn-status-behavior.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (23/118 checks passed overall).

Cause 1 — route53.health-check-delete-allowed-while-referenced [error]:
Sequence: CreateHealthCheck (HTTP/HTTPS check, Port 80, RequestInterval 30, FailureThreshold 3) -> returns HealthCheck.Id 6de0bd49-2070-7764-6e56-4bf63dd5ce8a; CreateHostedZone; ChangeResourceRecordSets CREATE of a weighted A record with HealthCheckId = that id.
Expected (AWS): HTTP 200 with ChangeInfo (PENDING -> INSYNC); the record set keeps the HealthCheckId, and DeleteHealthCheck on a referenced health check is permitted, leaving the record set with a dangling reference.
Actual (emulator): botocore.errorfactory.NoSuchHealthCheck: An error occurred (NoSuchHealthCheck) when calling the ChangeResourceRecordSets operation: no health check exists with the ID 6de0bd49-2070-7764-6e56-4bf63dd5ce8a.
Code evidence: change_resource_record_sets.rs returns NoSuchHealthCheck for the first non-empty `change.resource_record_set.health_check_id` in the batch, with no store lookup.
Model evidence (smithy_query on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json): ChangeResourceRecordSets errors = InvalidChangeBatch 400, InvalidInput 400, NoSuchHealthCheck 404, NoSuchHostedZone 404, PriorRequestNotComplete 400; success 200. ResourceRecordSet.HealthCheckId documentation describes it as a reference to an existing health check.

Cause 2 — route53.deploy-health-check-cfn-status-behavior [error]:
Template: an API Gateway REST API with a MOCK 200 integration on /prod/healthy plus HealthCheck = AWS::Route53::HealthCheck {HealthCheckConfig: {Type: HTTPS, FullyQualifiedDomainName: !Sub '${Api}.execute-api...', Port: 443, ResourcePath: !Ref ResourcePath, RequestInterval: 10, FailureThreshold: 1}}; Outputs.HealthCheckId.Value = !Ref HealthCheck.
Expected (AWS): the output is a Route 53 health check ID; GetHealthCheck returns 200 with that config and a HealthCheckVersion, and the version advances after a stack update that changes ResourcePath.
Actual (emulator): stack reaches CREATE_COMPLETE; output value is 'sim2real-r53-cfn-e9c58310712b-HealthCheck-51481AACD40C' and botocore.errorfactory.NoSuchHealthCheck: An error occurred (NoSuchHealthCheck) when calling the GetHealthCheck operation: no health check exists with the ID sim2real-r53-cfn-e9c58310712b-HealthCheck-51481AACD40C.
Model evidence: GetHealthCheck GET /2013-04-01/healthcheck/{HealthCheckId}, HealthCheckId @length max 64, errors IncompatibleVersion 400 / InvalidInput 400 / NoSuchHealthCheck 404, success 200.

## Steps to Reproduce
Cause 1:
1. route53.create_health_check(CallerReference=<uuid>, HealthCheckConfig={FullyQualifiedDomainName: <fqdn>, Port: 80, Type: 'HTTP', ResourcePath: '/', RequestInterval: 30, FailureThreshold: 3}) and keep HealthCheck.Id.
2. route53.create_hosted_zone for any zone name.
3. route53.change_resource_record_sets(HostedZoneId=<zone>, ChangeBatch={'Changes': [{'Action': 'CREATE', 'ResourceRecordSet': {'Name': 'weighted.<zone>', 'Type': 'A', 'SetIdentifier': 'primary', 'Weight': 100, 'TTL': 60, 'ResourceRecords': [{'Value': '192.0.2.47'}], 'HealthCheckId': <id from step 1>}}]}).
   Observed: HTTP 404 NoSuchHealthCheck. Expected: HTTP 200 with ChangeInfo.
4. Then delete_health_check(<id>) and list_resource_record_sets: AWS keeps the record set with the now-dangling HealthCheckId.

Cause 2:
1. cloudformation.create_stack with a template whose only required resource is HealthCheck: AWS::Route53::HealthCheck {HealthCheckConfig: {Type: HTTPS, FullyQualifiedDomainName: <any resolvable name>, Port: 443, ResourcePath: '/prod/healthy', RequestInterval: 10, FailureThreshold: 1}} and Outputs.HealthCheckId.Value = {'Ref': 'HealthCheck'}.
2. Wait for CREATE_COMPLETE and read the output via describe_stacks — the value is '<stack>-HealthCheck-<random>', not a health check id.
3. route53.get_health_check(HealthCheckId=<that output>) — observe HTTP 404 NoSuchHealthCheck instead of the provisioned health check.
4. update_stack changing ResourcePath and re-read: on AWS HealthCheckVersion advances and HealthCheckConfig.ResourcePath reflects the new value.
