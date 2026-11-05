Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__servicediscovery.cloudmap-srv-service-defaults-to-multivalue-routing-and-type-dns-http
Issue-Key: ecs__servicediscovery.cloudmap-srv-service-defaults-to-multivalue-routing-and-type-dns-http|run#12|KeyError|assert got["ResourceOwner"] == account and got["CreatedByAccount"] == account, (
Issue-Commit: 57cca9dfb40cff43666727c7926823b091294572

# servicediscovery GetService omits Service.ResourceOwner and Service.CreatedByAccount

> _Note: this issue was identified against the code base at commit `57cca9dfb40cff43666727c7926823b091294572`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetService

## Description
Cause 1 of 1: the Cloud Map `Service` view the emulator serves hard-codes the two account-identity members to absent, so every read of a service omits them.

AWS: Smithy (com.amazonaws.servicediscovery) models `Service$ResourceOwner` -> `AWSAccountId` (@length min 12, max 12), documented as "The ID of the Amazon Web Services account that created the namespace with which the service is associated", and `Service$CreatedByAccount` -> `AWSAccountId` (@length 12..12), documented as "The ID of the Amazon Web Services account that created the service". `GetService` returns `Service` (output member `Service`), HTTP 200. Real AWS populates both on an own-account service: each is the 12-digit id of the account the request is signed for (the namespace owner for `ResourceOwner`, the service creator for `CreatedByAccount`); they differ from the caller's account only for a cross-account shared namespace. The value is taken from the signing identity of the request, not from any fixed id.

Emulator: `GetService` answers 200 with a `Service` object that has NO `ResourceOwner` and NO `CreatedByAccount` key at all, so a client indexing the modeled member raises `KeyError: 'ResourceOwner'`. The same omission comes from the single rendering helper, so it is not specific to this one read: `state::service_view` (used by `GetService`) and `state::service_summary_view` (used by the service listing) both set these members to absent unconditionally. All other representation assertions in the failing check passed (`Type` = DNS_HTTP, `DnsConfig.RoutingPolicy` = MULTIVALUE, the single SRV/TTL 60 record, `DnsConfig.NamespaceId` filled from the namespace, `HealthCheckCustomConfig` = FailureThreshold 1, no `HealthCheckConfig`), so the divergence is isolated to these two members.

Why: a client that reads `Service.ResourceOwner`/`Service.CreatedByAccount` — to decide whether a service belongs to its own account or to a shared namespace owner, to attribute a Cloud Map service to an account in inventory/tagging tooling, or merely to index the field as a modeled required-in-practice value — crashes (KeyError / missing-field deserialization) or wrongly concludes the resource has no owner. Ownership checks that gate cross-account behaviour silently take the "unknown owner" branch.

Fix: the servicediscovery `Service` response rendering — `state::service_view` (resource_owner / created_by_account currently literal `None`) and the parallel `state::service_summary_view` — must carry the account id of the identity the call was signed for, in the same way the region is already derived from the request's sigv4 credential scope; `GetService`'s handler then returns them for every stored service.

Traces: ecs__servicediscovery.cloudmap-srv-service-defaults-to-multivalue-routing-and-type-dns-http (error, KeyError: 'ResourceOwner' at the GetService assertion).

## Evidence
Failing check: ecs__servicediscovery.cloudmap-srv-service-defaults-to-multivalue-routing-and-type-dns-http

Replay output (tail):
  File "check_ecs__servicediscovery_cloudmap_srv_service_defaults_to_multivalue_routing_and_type_dns_http.py", line 154, in run
    assert got["ResourceOwner"] == account and got["CreatedByAccount"] == account, (
  KeyError: 'ResourceOwner'

Expected (real AWS): GetService -> 200, Service contains ResourceOwner = <caller's 12-digit account id> and CreatedByAccount = <caller's 12-digit account id> (both modeled AWSAccountId, @length 12..12). The ids are request/state values, not fixed text.
Actual (emulator): GetService -> 200, Service contains neither member; both are serialized as absent.

Source of the omission in the checkout:
  services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/state.rs:1187-1204 (`service_view`: `resource_owner: None`, `created_by_account: None`)
  services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/state.rs:1210-1228 (`service_summary_view` reuses the same absent values)
  services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/get_service.rs:159-161 (handler returns `service_view(service)`)

## Steps to Reproduce
1. CreatePrivateDnsNamespace(Name="<unique>.local", Vpc=<vpc>) and poll GetOperation until Status == SUCCESS; take the namespace id from Operation.Targets.NAMESPACE.
2. CreateService(Name="<unique>", NamespaceId=<ns-id>, DnsConfig={DnsRecords:[{Type:"SRV", TTL:60}]}, HealthCheckCustomConfig={FailureThreshold:1}).
3. GetService(Id=<service-id>) and read Service["ResourceOwner"] and Service["CreatedByAccount"].
4. Observed: both keys are absent (KeyError). Expected: both equal the 12-digit account id the request was signed for.
5. Re-replay the check: ecs__servicediscovery.cloudmap-srv-service-defaults-to-multivalue-routing-and-type-dns-http
