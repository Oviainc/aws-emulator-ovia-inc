Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-5xxklnd75r355zqy"].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-5xxklnd75r355zqy"].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-5xxklnd75r355zqy"].ResourceOwner
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-ol7s87fd94g6mq2g"].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-ol7s87fd94g6mq2g"].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-ol7s87fd94g6mq2g"].ResourceOwner
Issue-Commit: e8d34fd8b87a640bf972b7f1f05015aa932d31e1

# ListServices multi-service page drops ResourceOwner, CreatedByAccount and DnsConfig on every ServiceSummary

> _Note: this issue was identified against the code base at commit `e8d34fd8b87a640bf972b7f1f05015aa932d31e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
ONE net-new wire divergence in the servicediscovery (Cloud Map) emulator. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Cause 1 of 1: ListServices' ServiceSummary entries omit ResourceOwner, CreatedByAccount and DnsConfig when the page contains more than one service (the Arn-keyed page projection).
AWS: Smithy ListServices{NextToken, MaxResults, Filters: ServiceFilters} -> {Services: list of ServiceSummary, NextToken}; error InvalidInput (400). ServiceSummary members are Id, Arn, ResourceOwner: AWSAccountId (@length min 12 max 12, 'The ID of the Amazon Web Services account that created the namespace with which the service is associated'), Name, Type, Description, InstanceCount, DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, CreateDate, CreatedByAccount: AWSAccountId (@length min 12 max 12, 'The ID of the Amazon Web Services account that created the service'). Recorded behavior: every summary in the page carries ResourceOwner and CreatedByAccount set to the caller's own account id (the emulator's configured account in a single-account call, NOT a literal copied from the recording), and carries a DnsConfig member - present as an empty structure even for a service created in an HTTP namespace with no DnsConfig, i.e. AWS emits the member rather than omitting it.
Emulator: HTTP 200 and the status and remaining summary fields match, but for EVERY entry of the page (both services in the recorded page, selected by their service Arns) there is no ResourceOwner member, no CreatedByAccount member and no DnsConfig member.
Why: a client that groups, filters, sorts or displays a service listing by owning account reads absent members and gets a KeyError or an empty column against the emulator where AWS always supplies them; a client that reads summary['DnsConfig'] unconditionally to inspect the record configuration breaks the same way, and re-reading each service individually does not recover the data because the per-service read drops the same members too.
Fix: the ServiceSummary projection in the ListServices handler, plus the service record in the service state it reads the account and DnsConfig fields from - the member must be emitted for each entry of a multi-entry page, not only for the first.
Traces: servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5.

Relation to already-open issues: this cluster's replay key (servicediscovery|ListServices|status=200->200|keys=$.Services[Arn="..."].CreatedByAccount,.DnsConfig,.ResourceOwner) is reported by the replay with owned_by EMPTY, i.e. no open issue carries it. Two open issues look similar but do not own it: 20261029T203804Z-servicediscovery-cloud-map-namespace-service-and-operation-responses-omit.md (its ListServices Issue-Key is the truncated 'servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn', which does not resolve to this key) and 20261103T202622Z-servicediscovery-cloud-map-list-pages-omit-resourceowner-createdbyaccount-dnsconfig.md (its ListServices key is the index-keyed $.Services[*]/[0] single-entry page from servicediscovery-ecscm-traj-cloudmap-service-create-mistakes#task#6). Neither one's replay pointer exercises this step, so this cause stays unverified unless it is closed against the trace named above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 183 steps total, 130 passed, 53 mismatched, 0 errored, 0 blocked, score 71.0%, 27 divergence clusters, emulator_up true (bootstrap_signal false).

Cluster (divergence 19 of 27), fixture servicediscovery-multi-service-instance-isolation-and-revision-tracking, step task#5, operation ListServices, verdict mismatch, expected status 200 / actual status 200, affected_count 1, stub_path empty, owned_by empty.
Detail reported by the replay (expected = the recorded AWS response; the recorded 12-digit account id is the caller's own account, not a requirement in itself):
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/<service-id-1>"].CreatedByAccount: missing (expected the caller account id, got None)
  $.Services[Arn="...service/<service-id-1>"].DnsConfig: missing (expected {}, got None)
  $.Services[Arn="...service/<service-id-1>"].ResourceOwner: missing (expected the caller account id, got None)
  $.Services[Arn="...service/<service-id-2>"].CreatedByAccount: missing (expected the caller account id, got None)
  $.Services[Arn="...service/<service-id-2>"].DnsConfig: missing (expected {}, got None)
  $.Services[Arn="...service/<service-id-2>"].ResourceOwner: missing (expected the caller account id, got None)
No status or error-code divergence: the response is a 200 whose body is structurally short of three modelled members on each entry. Generated service ids, arns, region and the account value differ per environment and are not part of the requirement; the requirement is that each ServiceSummary carries ResourceOwner and CreatedByAccount (the caller's account id) and a DnsConfig structure (empty when the service has no DNS records).

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture step in Issue-Trace/Issue-Step (servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5), or reproduce with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. op = sd.create_http_namespace(Name='<unique>')['OperationId']; poll sd.get_operation(OperationId=op) until Status == 'SUCCESS'; ns = <namespace id from the operation Targets>.
2. Create TWO services in that namespace: sd.create_service(Name='<unique-a>', NamespaceId=ns) and sd.create_service(Name='<unique-b>', NamespaceId=ns).
3. page = sd.list_services() -> for EVERY entry in page['Services'], 'ResourceOwner', 'CreatedByAccount' and 'DnsConfig' are absent. AWS returns all three on each summary (both account members set to the caller's account id, DnsConfig an empty structure for an HTTP-namespace service).
