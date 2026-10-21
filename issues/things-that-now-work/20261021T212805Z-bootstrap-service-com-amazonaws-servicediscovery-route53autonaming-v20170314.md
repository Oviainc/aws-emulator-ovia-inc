Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the servicediscovery (Route53AutoNaming) emulator

_Filed: 2026-10-21T21:28:05Z_
_Operation: service-com-amazonaws-servicediscovery-route53autonaming-v20170314 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 30 operations in this crate is an untouched scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no AWS Cloud Map / servicediscovery client can do anything at all. This issue defines the FIRST minimal, stateful slice: the Cloud Map core loop of "create an HTTP namespace -> resolve the async operation -> create a service in it -> register an instance -> discover the instance -> deregister it".

Wire protocol is `awsJson1_1`: `POST /` with `X-Amz-Target: Route53AutoNaming_v20170314.<OperationName>` (per OPERATIONS.md "Wire protocol"), sigv4 name `servicediscovery`.

## Happy path to implement (in order)

1. **CreateHttpNamespace** — input `Name` (required, len 1..1024, pattern `^(?!arn:)[!-~]{1,1024}$`), `CreatorRequestId` (@idempotencyToken), `Description`, `Tags`. Output: `OperationId` ONLY. So the handler must (a) mint a namespace id (`ns-<random>`) and persist a Namespace record with `Id`, `Arn` (`arn:aws:servicediscovery:<region>:<account>:namespace/<id>`), `Name`, `Type=HTTP`, `Description`, `ServiceCount=0`, `CreateDate`, `CreatorRequestId`, `Properties.HttpProperties.HttpName=<Name>`, and (b) persist an Operation record whose `Targets` map contains `NAMESPACE -> <namespace id>` and return that operation's id. Re-creating an existing name must fail `NamespaceAlreadyExists` (400) carrying `NamespaceId`/`CreatorRequestId`; replaying the same `CreatorRequestId` must be idempotent (not a second namespace).
2. **GetOperation** — input `OperationId` (required). Output `Operation` with `Id`, `Type` (`CREATE_NAMESPACE` / `REGISTER_INSTANCE` / `DEREGISTER_INSTANCE` / `DELETE_NAMESPACE`), `Status` (emulate as `SUCCESS` immediately), `CreateDate`, `UpdateDate`, and the `Targets` map. This op is load-bearing: it is the ONLY way a client learns the namespace id created in step 1. An unknown id must be `OperationNotFound` (404) — never a synthesized success.
3. **CreateService** — input `Name` (required), `NamespaceId`, `CreatorRequestId`, `Description`, `DnsConfig`, `HealthCheckConfig`, `HealthCheckCustomConfig`, `Tags`, `Type`. Output the full `Service` structure computed from what was stored: `Id` (`srv-<random>`), `Arn`, `Name`, `NamespaceId`, `Description`, `InstanceCount=0`, `DnsConfig`, `Type`, `CreateDate`, `CreatorRequestId`. Unknown `NamespaceId` -> `NamespaceNotFound` (404); duplicate name within the namespace -> `ServiceAlreadyExists` (400). Must also increment the owning namespace's `ServiceCount`.
4. **RegisterInstance** — input `ServiceId` (required), `InstanceId` (required, pattern `^[0-9a-zA-Z_/:.@-]+$`), `Attributes` (required map), `CreatorRequestId`. Output `OperationId`. Persist an Instance{InstanceId, Attributes, CreatorRequestId} under the service (re-registering the same `InstanceId` REPLACES the attributes, it does not duplicate), bump the service's `InstanceCount`, bump the namespace+service instances revision, and record a `REGISTER_INSTANCE` operation whose `Targets` carry `NAMESPACE`/`SERVICE`/`INSTANCE`. Unknown `ServiceId` -> `ServiceNotFound` (404).
5. **DiscoverInstances** — input `NamespaceName` (required), `ServiceName` (required), `MaxResults`, `QueryParameters`, `HealthStatus`. Output `Instances` (list<HttpInstanceSummary>: `InstanceId`, `NamespaceName`, `ServiceName`, `HealthStatus`, `Attributes`) plus `InstancesRevision`. The returned attributes MUST be the exact map persisted in step 4, looked up by namespace NAME + service NAME (not id). `QueryParameters` must filter: an instance matches only if every requested key/value pair is present in its attributes. Unknown namespace/service -> `NamespaceNotFound`/`ServiceNotFound` (404).
6. **DeregisterInstance** — input `ServiceId`, `InstanceId` (both required). Output `OperationId`. Remove the instance from the service, decrement `InstanceCount`, bump the revision, record a `DEREGISTER_INSTANCE` operation. A following DiscoverInstances must return an EMPTY `Instances` list. Unknown instance -> `InstanceNotFound` (404).

Every one of these six must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)`, per README.md "State"). No canned bodies, no echoing the input back, and no minting an id that is never stored — in particular `CreateHttpNamespace`/`RegisterInstance` returning a random `OperationId` that `GetOperation` cannot resolve is a failure of this issue, as is `DiscoverInstances` returning a hardcoded instance list.

## State Model

In `src/state.rs`, on the service state struct:

- `namespaces: HashMap<String /*namespace id*/, Namespace>` where `Namespace { id, arn, name, type_: HTTP|DNS_PRIVATE|DNS_PUBLIC, description, service_count: usize, create_date, creator_request_id, properties, tags: Vec<Tag> }`
- `namespace_ids_by_name: HashMap<String /*Name*/, String /*namespace id*/>` — the index DiscoverInstances needs to resolve `NamespaceName`, and the one that makes `NamespaceAlreadyExists` detectable
- `services: HashMap<String /*service id*/, Service>` where `Service { id, arn, name, namespace_id, description, instance_count: usize, dns_config, health_check_config, health_check_custom_config, type_, create_date, creator_request_id, tags, instances: BTreeMap<String /*InstanceId*/, Instance>, instances_revision: i64 }`
- `Instance { instance_id, attributes: HashMap<String,String>, creator_request_id, health_status: HealthStatus }`
- `service_ids_by_namespace_and_name: HashMap<(String /*namespace id*/, String /*service Name*/), String /*service id*/>` — resolves DiscoverInstances' `ServiceName` and enforces `ServiceAlreadyExists`
- `operations: HashMap<String /*operation id*/, Operation>` where `Operation { id, type_, status, create_date, update_date, error_code, error_message, targets: HashMap<String,String> }` — populated by every async op so `GetOperation` is a real read
- `idempotency: HashMap<String /*CreatorRequestId*/, String /*resource or operation id*/>` so a replayed `CreatorRequestId` returns the first result instead of creating a duplicate

Counters (`ServiceCount`, `InstanceCount`, `InstancesRevision`) must be derived/maintained on write, not hardcoded.

## Out of scope for this slice

CreatePrivateDnsNamespace, CreatePublicDnsNamespace, Update*/Delete* (beyond what is listed), ListNamespaces/ListServices/ListInstances/ListOperations pagination, Get/Update/DeleteServiceAttributes, GetInstancesHealthStatus, UpdateInstanceCustomHealthStatus, DiscoverInstancesRevision, Tag/Untag/ListTagsForResource, and Route 53 record propagation stay at 501 and will be filed as individual gap issues later.

## Evidence

All 30 files under `services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501; the service therefore holds zero state and no workflow can succeed. Operation signatures, required members, string patterns and modeled errors above are taken verbatim from the crate's generated `OPERATIONS.md` (CreateHttpNamespace, GetOperation, CreateService, RegisterInstance, DiscoverInstances, DeregisterInstance sections) which is rendered from `smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json`. The asynchronous create pattern (create returns only `OperationId`, whose `Targets` map carries the created resource id) and attribute-based `QueryParameters` filtering in DiscoverInstances are well-established AWS Cloud Map semantics. No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `servicediscovery` client at it.
2. CreateHttpNamespace Name="bootstrap-ns" -> expect 200 with an `OperationId`.
3. GetOperation OperationId=<that id> -> expect 200, Status=SUCCESS, Type=CREATE_NAMESPACE, Targets["NAMESPACE"]=<namespace id>.
4. CreateService Name="bootstrap-svc" NamespaceId=<that namespace id> -> expect 200 with Service.Id, Service.Arn, Service.NamespaceId echoing the stored namespace and InstanceCount=0.
5. RegisterInstance ServiceId=<service id> InstanceId="i-1" Attributes={"AWS_INSTANCE_IPV4":"10.0.0.1","stage":"prod"} -> expect 200 with an OperationId that GetOperation resolves to Type=REGISTER_INSTANCE.
6. DiscoverInstances NamespaceName="bootstrap-ns" ServiceName="bootstrap-svc" -> expect exactly one HttpInstanceSummary with InstanceId="i-1" and Attributes equal to the map registered above, plus an InstancesRevision.
7. DiscoverInstances with QueryParameters={"stage":"prod"} -> expect the same single instance; with QueryParameters={"stage":"dev"} -> expect an empty Instances list.
8. DeregisterInstance ServiceId=<service id> InstanceId="i-1" -> expect 200 with an OperationId; then DiscoverInstances again -> expect an empty Instances list.
9. Negative checks: GetOperation with a bogus id -> OperationNotFound 404; CreateService with a bogus NamespaceId -> NamespaceNotFound 404; RegisterInstance with a bogus ServiceId -> ServiceNotFound 404; CreateHttpNamespace repeating Name="bootstrap-ns" -> NamespaceAlreadyExists 400.
