Issue-Source: trace-sweep-divergence
Issue-Trace: servicediscovery-ecscm-traj-cloudmap-service-create-mistakes
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicediscovery-ecscm-traj-cloudmap-service-create-mistakes servicediscovery-ecscm-traj-rerun-create-namespace
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[*].CreatedByAccount
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[*].DnsConfig
Issue-Key: servicediscovery|ListServices|2xx->2xx|$.Services[*].ResourceOwner
Issue-Key: servicediscovery|ListNamespaces|2xx->2xx|$.Namespaces[*].Properties.DnsProperties
Issue-Key: servicediscovery|ListNamespaces|2xx->2xx|$.Namespaces[*].ResourceOwner
Issue-Commit: 7395066f6d0f65e771980acfc36b289985512441

# Cloud Map list pages omit ResourceOwner, CreatedByAccount, DnsConfig and DnsProperties

> _Note: this issue was identified against the code base at commit `7395066f6d0f65e771980acfc36b289985512441`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
TWO independent, net-new wire divergences in the servicediscovery (Cloud Map) emulator: the paged LIST projections (ListServices' ServiceSummary entries and ListNamespaces' NamespaceSummary entries) drop modelled members that AWS always populates - the ownership members ResourceOwner / CreatedByAccount and the structural members DnsConfig (service summaries) and Properties.DnsProperties (namespace summaries). Both causes below must be closed before this issue is submitted. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Shared AWS fact: AWSAccountId is @length min 12 max 12 and the ownership members are documented as 'The ID of the Amazon Web Services account that created ...'. In a single-account call they all carry the CALLER'S own account id - i.e. the emulator's configured account - not a literal copied from one recording. Both of these clusters are distinct replay keys from the already-open issue 20261029T203804Z-servicediscovery-cloud-map-namespace-service-and-operation-responses-omit.md, which carries the GetNamespace / GetOperation / CreateService / GetService / GetServiceAttributes keys and only the Arn-keyed ListServices page; it does NOT carry these two keys, so neither cause is exercised by its replay pointer.

Cause 1 of 2: ListServices' summaries omit ResourceOwner, CreatedByAccount and DnsConfig.
AWS: Smithy ListServices{NextToken (@length 0..4096), MaxResults (@range 1..100), Filters: ServiceFilters} -> {Services: ServiceSummariesList, NextToken}; error InvalidInput (400). ServiceSummary members: Id, Arn, ResourceOwner: AWSAccountId, Name, Type, Description, InstanceCount, DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, CreateDate, CreatedByAccount: AWSAccountId. Recorded: every summary in the page carries ResourceOwner and CreatedByAccount set to the owning account, and carries a DnsConfig member - present as an EMPTY structure even for a service in an HTTP namespace that was created with no DnsConfig, i.e. AWS emits the member rather than omitting it.
Emulator: HTTP 200 with the page (status and the rest of the summary match), but each entry has no ResourceOwner, no CreatedByAccount and no DnsConfig member.
Why: a client that groups, filters or displays a service list by owner, or that reads summary['DnsConfig'] unconditionally to inspect the record configuration, gets a KeyError / empty column against the emulator where AWS always supplies the member - and re-reading each service individually does not help, because the per-service read drops the same members.
Fix: the ServiceSummary projection in ListServices' handler (and the service record in state that it reads the account/DnsConfig fields from).
Traces: servicediscovery-ecscm-traj-cloudmap-service-create-mistakes#task#6.

Cause 2 of 2: ListNamespaces' summaries omit ResourceOwner and Properties.DnsProperties.
AWS: Smithy ListNamespaces{NextToken (@length 0..4096), MaxResults (@range 1..100), Filters: NamespaceFilters} -> {Namespaces: NamespaceSummariesList, NextToken}; error InvalidInput (400). NamespaceSummary members: Id, Arn, ResourceOwner: AWSAccountId, Name, Type, Description, ServiceCount, Properties: NamespaceProperties{DnsProperties{HostedZoneId, SOA}, HttpProperties}, CreateDate. Recorded: each summary carries ResourceOwner = the owning account id, and Properties.DnsProperties present with an SOA sub-structure.
Emulator: HTTP 200 with the page but each entry missing ResourceOwner, and its Properties carrying no DnsProperties member at all.
Why: hosted-zone / SOA discovery and cross-account sharing decisions made off a namespace listing read absent members; a caller cannot tell from the list which account owns a namespace or whether it has DNS properties.
Fix: the NamespaceSummary projection in ListNamespaces' handler (same namespace state fields the GetNamespace projection needs).
Traces: servicediscovery-ecscm-traj-rerun-create-namespace#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 183 steps, 130 passed, 53 mismatched, score 71.0%, 27 divergence clusters. Both clusters below are status-matched HTTP 200 responses with modelled members missing (expected = the recorded AWS response; the recorded account id is the caller's own account):

Cause 1 - servicediscovery-ecscm-traj-cloudmap-service-create-mistakes#task#6, ListServices, expected 200 / actual 200: '$.Services[0].CreatedByAccount: missing (expected the caller account id, got None)'; '$.Services[0].DnsConfig: missing (expected {}, got None)'; '$.Services[0].ResourceOwner: missing (expected the caller account id, got None)'. Divergence key servicediscovery|ListServices|status=200->200|keys=$.Services[0].CreatedByAccount,$.Services[0].DnsConfig,$.Services[0].ResourceOwner. owned_by: none.

Cause 2 - servicediscovery-ecscm-traj-rerun-create-namespace#task#5, ListNamespaces, expected 200 / actual 200: '$.Namespaces[0].Properties.DnsProperties: missing (expected {"SOA": {}}, got None)'; '$.Namespaces[0].ResourceOwner: missing (expected the caller account id, got None)'. Divergence key servicediscovery|ListNamespaces|status=200->200|keys=$.Namespaces[0].Properties.DnsProperties,$.Namespaces[0].ResourceOwner. owned_by: none.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture steps in Issue-Trace/Issue-Step and Issue-Trace-Ids, or with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)):
1. Cause 2: op = sd.create_http_namespace(Name='<unique>')['OperationId']; poll sd.get_operation(OperationId=op) to SUCCESS; sd.list_namespaces() -> each entry in ['Namespaces'] has no 'ResourceOwner' and its ['Properties'] has no 'DnsProperties'. Repeat after creating a public DNS namespace.
2. Cause 1: ns = <namespace id from step 1>; sd.create_service(Name='<unique>', NamespaceId=ns); sd.list_services() -> each entry in ['Services'] has no 'ResourceOwner', no 'CreatedByAccount' and no 'DnsConfig' (AWS returns both account ids and a DnsConfig structure, empty for an HTTP-namespace service).
