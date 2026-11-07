Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-multi-service-instance-isolation-and-revision-tracking
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-fmkgci0b9zzxaron"].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-fmkgci0b9zzxaron"].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-fmkgci0b9zzxaron"].ResourceOwner
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-nbj91zu6f105ebil"].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-nbj91zu6f105ebil"].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[Arn="arn:aws:servicediscovery:us-west-2:000000000000:service/srv-nbj91zu6f105ebil"].ResourceOwner
Issue-Commit: b9d2725659209c141c169cec7c41334c1c27aac5

# Cloud Map ListServices multi-service page omits ResourceOwner, CreatedByAccount and DnsConfig

> _Note: this issue was identified against the code base at commit `b9d2725659209c141c169cec7c41334c1c27aac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
ONE net-new wire divergence in the servicediscovery (Cloud Map) emulator.

Cause 1 of 1: the ServiceSummary projection returned by ListServices drops the modelled ownership members ResourceOwner and CreatedByAccount and the modelled structural member DnsConfig, for EVERY entry of a page that lists more than one service in the same namespace.

AWS: Smithy (services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json) ListServices{NextToken @length 0..4096, MaxResults @range 1..100, Filters: ServiceFilters} -> ListServicesResponse{Services: ServiceSummariesList, NextToken}; modelled error InvalidInput (400); paginated inputToken/outputToken NextToken, pageSize MaxResults. ServiceSummary members: Id, Arn, ResourceOwner: AWSAccountId (@length min 12 max 12, "The ID of the Amazon Web Services account that created the namespace with which the service is associated"), Name, Type, Description, InstanceCount, DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, CreateDate, CreatedByAccount: AWSAccountId (@length min 12 max 12, "The ID of the Amazon Web Services account that created the service"). Recorded behavior: in a single-account call every summary in the page carries ResourceOwner and CreatedByAccount set to the CALLER's own 12-digit account id (the emulator's configured account - never a literal copied from one recording), and carries a DnsConfig member, emitted as an EMPTY structure for a service created in an HTTP namespace with no DnsConfig rather than being omitted.

Emulator: HTTP 200 with the page and the rest of each summary matching, but EVERY entry in the multi-service page has no ResourceOwner, no CreatedByAccount and no DnsConfig member at all. The recorded page in this fixture holds two services in one namespace and both entries are missing all three members, so the omission is in the projection itself, not in one record.

Why: a client that groups, filters, displays or makes cross-account sharing decisions off a service listing reads absent members - summary['ResourceOwner'] / summary['CreatedByAccount'] raise KeyError and summary['DnsConfig'] is unavailable for inspecting the record configuration - where AWS always supplies them. The AWSAccountId @length 12..12 constraint means a consumer may also be validating the field's shape, so absence is not a benign omission.

Fix: the ServiceSummary projection in the ListServices handler, plus the service record in state it must read the owning/creating account and the DnsConfig structure from.

Related open issues that do NOT own this replay key: 20261103T202622Z-servicediscovery-cloud-map-list-pages-omit-resourceowner-createdbyaccount-dnsconfig.md (carries the index-keyed single-entry ListServices page and the ListNamespaces page) and 20261029T203804Z-servicediscovery-cloud-map-namespace-service-and-operation-responses-omit.md (carries the GetNamespace / GetOperation / CreateService / GetService / GetServiceAttributes keys). The replay reports this cluster with owned_by: none, i.e. neither issue's replay pointer exercises the Arn-keyed multi-service ListServices page, so fixing either one can be submitted while this page still diverges.

Traces: servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 183 steps, 130 passed, 53 mismatched, 0 blocked, 0 errored, score 71.0%, 27 divergence clusters, emulator_up true (/__admin/health answered 200).

This cluster (the only one in the run whose owned_by is empty):
fixture servicediscovery-multi-service-instance-isolation-and-revision-tracking, step task#5, ListServices, expected HTTP 200 / actual HTTP 200, verdict mismatch, affected_count 1, stub_path empty, owned_by empty.
Divergence key: servicediscovery|ListServices|status=200->200|keys=$.Services[Arn="...service/srv-<id-A>"].CreatedByAccount,...DnsConfig,...ResourceOwner,$.Services[Arn="...service/srv-<id-B>"].CreatedByAccount,...DnsConfig,...ResourceOwner
Detail, for BOTH services in the page (service ids and ARNs are per-run values, not requirements):
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/srv-<A>"].CreatedByAccount: missing (expected the caller's account id, got None)
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/srv-<A>"].DnsConfig: missing (expected {}, got None)
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/srv-<A>"].ResourceOwner: missing (expected the caller's account id, got None)
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/srv-<B>"].CreatedByAccount: missing (expected the caller's account id, got None)
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/srv-<B>"].DnsConfig: missing (expected {}, got None)
  $.Services[Arn="arn:aws:servicediscovery:<region>:<account>:service/srv-<B>"].ResourceOwner: missing (expected the caller's account id, got None)

Status and error code are not at issue here (200 expected, 200 served); the divergence is purely modelled members absent from each ServiceSummary.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture step named in Issue-Trace/Issue-Step (servicediscovery-multi-service-instance-isolation-and-revision-tracking#task#5), or reproduce with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. op = sd.create_http_namespace(Name='<unique>')['OperationId']; poll sd.get_operation(OperationId=op) until Operation.Status == 'SUCCESS'; take the namespace id ns from the operation's Targets, or from sd.list_namespaces().
2. Create TWO services in that one namespace: for n in ('<unique-a>', '<unique-b>'): sd.create_service(Name=n, NamespaceId=ns).
3. page = sd.list_services(Filters=[{'Name': 'NAMESPACE_ID', 'Values': [ns], 'Condition': 'EQ'}]).
4. Observe: HTTP 200, page['Services'] has both services, but EVERY entry lacks 'ResourceOwner', 'CreatedByAccount' and 'DnsConfig'. AWS returns both 12-digit account ids (the caller's own account in a single-account call) and a 'DnsConfig' member on every entry - present as an empty structure for an HTTP-namespace service created without DnsConfig.
