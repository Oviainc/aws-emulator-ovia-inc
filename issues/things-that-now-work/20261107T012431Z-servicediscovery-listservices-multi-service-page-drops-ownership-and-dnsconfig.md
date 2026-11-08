Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-cbefifhfqz4vsv7m"].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-cbefifhfqz4vsv7m"].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-cbefifhfqz4vsv7m"].ResourceOwner
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-jv7mukqxwv9v5osz"].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-jv7mukqxwv9v5osz"].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-jv7mukqxwv9v5osz"].ResourceOwner
Issue-Commit: e9aa8ba1fdfe417f19b3a603b17e06ccf1308d5b

# ListServices multi-service page drops ownership and DnsConfig members

> _Note: this issue was identified against the code base at commit `e9aa8ba1fdfe417f19b3a603b17e06ccf1308d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
ONE net-new wire divergence in the servicediscovery (Cloud Map) emulator, filed because its replay key is not carried by any open issue's pointer (replay reported owned_by: none for this cluster).

Cause 1 of 1: a multi-entry ListServices page omits ResourceOwner, CreatedByAccount and DnsConfig on EVERY ServiceSummary it returns.

AWS: Smithy model services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json. ListServices{NextToken (@length 0..4096), MaxResults (@range 1..100), Filters: ServiceFilters} -> {Services: ServiceSummariesList, NextToken}; modelled error InvalidInput (400); paginated inputToken=NextToken/outputToken=NextToken/pageSize=MaxResults. ServiceSummary members: Id, Arn, ResourceOwner: AWSAccountId (@length min 12 max 12, doc 'The ID of the Amazon Web Services account that created the namespace with which the service is associated'), Name, Type, Description, InstanceCount, DnsConfig: DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, CreateDate, CreatedByAccount: AWSAccountId (@length min 12 max 12, doc 'The ID of the Amazon Web Services account that created the service'). Recorded behavior: in a page containing MORE THAN ONE service, every summary carries ResourceOwner and CreatedByAccount set to the caller's own account id (the emulator's configured account — not a literal from one recording) and carries a DnsConfig member, present as an EMPTY structure for a service created in an HTTP namespace with no DnsConfig; AWS emits the member rather than omitting it.

Emulator: HTTP 200 and the page itself matches (both services present, keyed by Arn, with matching Id/Name/Type/InstanceCount), but EACH summary in the page has no ResourceOwner member, no CreatedByAccount member and no DnsConfig member.

Why: a client that lists the services of a namespace and groups, filters or displays them by owning account, or that reads summary['DnsConfig'] unconditionally to inspect the record configuration, gets a KeyError or an empty column against the emulator where AWS always supplies the member — and re-reading each service individually does not recover it, because GetService drops the same three members.

Fix: the ServiceSummary projection in the ListServices handler, and the service record in state it reads the account/DnsConfig fields from — the page must populate ResourceOwner/CreatedByAccount from the configured account and always emit DnsConfig (empty structure when the service has no DNS records) for every entry, not just for single-entry pages.

Related open issues, neither of which carries this replay key: 20261029T203804Z-servicediscovery-cloud-map-namespace-service-and-operation-responses-omit.md (its ListServices Issue-Key is truncated at 'servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn', so this cluster's key does not resolve to it) and 20261103T202622Z-servicediscovery-cloud-map-list-pages-omit-resourceowner-createdbyaccount-dnsconfig.md (carries only the index-keyed $.Services[0].* page key). This cluster is therefore unexercised by both of their replay pointers and must be closed here.

Traces: servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: emulator_up=true, 183 steps total, 130 passed, 53 mismatched, 0 errored, 0 blocked, score 71.0%, 27 divergence clusters. The cluster below is the only one with stub_path empty AND owned_by empty.

Fixture servicediscovery-multi-service-instance-isolation-and-revision-tracking, step task#5, operation ListServices, expected status 200 / actual status 200, verdict mismatch, affected_count 1. Divergence detail, verbatim:
  $.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-cbefifhfqz4vsv7m"].CreatedByAccount: missing (expected the caller's account id, got None)
  $.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-cbefifhfqz4vsv7m"].DnsConfig: missing (expected {}, got None)
  $.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-cbefifhfqz4vsv7m"].ResourceOwner: missing (expected the caller's account id, got None)
  $.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-jv7mukqxwv9v5osz"].CreatedByAccount: missing (expected the caller's account id, got None)
  $.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-jv7mukqxwv9v5osz"].DnsConfig: missing (expected {}, got None)
  $.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-jv7mukqxwv9v5osz"].ResourceOwner: missing (expected the caller's account id, got None)

Divergence key: service=servicediscovery|ListServices|status=200->200|keys=$.Services[Arn=...].CreatedByAccount,$.Services[Arn=...].DnsConfig,$.Services[Arn=...].ResourceOwner (both Arn-keyed entries). The service ids, ARNs and the account id above are per-run values from the recording; the requirement is that each member is PRESENT on every entry, with the account members equal to the emulator's configured account and DnsConfig emitted (empty when the service has no DNS records). No status or error-code divergence is involved: both expected and actual are HTTP 200.

## Steps to Reproduce
Boot the emulator from this checkout and replay fixture servicediscovery-multi-service-instance-isolation-and-revision-tracking step task#5, or reproduce with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. op = sd.create_http_namespace(Name='<unique>')['OperationId']; poll sd.get_operation(OperationId=op) until Status == 'SUCCESS'; ns = <namespace id from Operation['Targets']['NAMESPACE']>.
2. Create TWO services in that namespace: sd.create_service(Name='<unique-a>', NamespaceId=ns) and sd.create_service(Name='<unique-b>', NamespaceId=ns).
3. page = sd.list_services(Filters=[{'Name': 'NAMESPACE_ID', 'Values': [ns], 'Condition': 'EQ'}]).
4. Observe: HTTP 200 and both services appear, but for every entry in page['Services'] the keys 'ResourceOwner', 'CreatedByAccount' and 'DnsConfig' are all absent. AWS returns ResourceOwner and CreatedByAccount set to the calling account and a DnsConfig member (an empty structure for these HTTP-namespace services) on each entry.
