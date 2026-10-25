Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.resolver-dnssec-config-lifecycle
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|GetResolverDnssecConfig|ResourceNotFoundException
Issue-Commit: cffd6e9555a4201fe1372378bca74d6750d9dc8c

# route53resolver DNSSEC config read path: GetResolverDnssecConfig refuses a VPC with no explicit config, and ListResolverDnssecConfigs is unimplemented

> _Note: this issue was identified against the code base at commit `cffd6e9555a4201fe1372378bca74d6750d9dc8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResolverDnssecConfig

## Description
Two net-new root causes in the route53resolver DNSSEC-validation read path, both grounded in the failing check `route53resolver.resolver-dnssec-config-lifecycle`. There are 2 causes; both must be closed before that check can pass.

Note on an existing issue: the open issue `20261024T062131Z-route53resolver-getresolverdnssecconfig-stub.md` carries the same check id, but its Issue-Key is `route53resolver|GetResolverDnssecConfig|NotImplemented` and that stub no longer exists — `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/get_resolver_dnssec_config.rs` has had its `//! SCAFFOLDED, NOT IMPLEMENTED:` header removed and now has a real handler. Cause 1 below is a fidelity defect IN that implemented handler, not that 501, so it is not covered by that issue. The fixed issue `20261024T150703Z-route53resolver-updateresolverdnssecconfig-stub.md` covers UpdateResolverDnssecConfig, which is NOT the refusing call here, so this is not a reopen of it.

=== Cause 1 of 2: GetResolverDnssecConfig answers ResourceNotFoundException for an existing VPC that has never been the target of UpdateResolverDnssecConfig, instead of the implicit DISABLED configuration ===
Failing check ids: route53resolver.resolver-dnssec-config-lifecycle

AWS: Smithy (com.amazonaws.route53resolver): `GetResolverDnssecConfig` input `ResourceId` (ResourceId, @length 1..64, required) — "The ID of the virtual private cloud (VPC) for the DNSSEC validation status"; output `ResolverDNSSECConfig` of shape `ResolverDnssecConfig {Id (ResourceId 1..64), OwnerId (AccountId 12..32), ResourceId, ValidationStatus}` where `ResolverDNSSECValidationStatus` = ENABLING|ENABLED|DISABLING|DISABLED|UPDATING_TO_USE_LOCAL_RESOURCE_SETTING|USE_LOCAL_RESOURCE_SETTING; modeled errors AccessDeniedException, InternalServiceErrorException, InvalidParameterException, InvalidRequestException, ResourceNotFoundException (HTTP 400, "The specified resource doesn't exist"), ThrottlingException. The model does not state what a VPC with no explicit configuration reads as, so for that: recorded behavior only — the recording creates a brand-new VPC (ec2 CreateVpc) and, with no UpdateResolverDnssecConfig ever issued for it, reads GetResolverDnssecConfig and gets HTTP 200 with a `ResolverDNSSECConfig` carrying an `Id`, `ResourceId` equal to that VPC id and a settled `ValidationStatus` (DISABLED for a VPC that has never had validation enabled). That implicit `Id` is stable: the UpdateResolverDnssecConfig that follows returns the same `Id`, and the config is visible in ListResolverDnssecConfigs. In other words every VPC in the account has a DNSSEC-validation configuration, defaulted to validation off; ResourceNotFoundException is for a `ResourceId` that is not a VPC of this account at all, not for one that merely has no stored override.

Emulator: refuses the read. HTTP 400, error code `ResourceNotFoundException`, message `Resolver DNSSEC configuration for resource with ID "<ResourceId>" does not exist.` — fixed text is `Resolver DNSSEC configuration for resource with ID "` ... `" does not exist.`; the only variable part is the VPC id taken verbatim from the request's `ResourceId` (the replay shows the freshly created VPC id in that slot). The cause is that the store only ever holds a DNSSEC record that UpdateResolverDnssecConfig inserted, and the Get handler treats an absent record as a missing resource; existence is keyed on "an update has run" rather than on "this VPC exists".

Why: a client doing the normal read-then-modify flow on DNSSEC validation (read the current status for a VPC, flip it, read back) gets a 400 error where AWS gives it DISABLED. Infrastructure tools that refresh state by reading the config (CloudFormation/Terraform-style drift reads, console status panes) see a hard failure instead of "validation off", and any caller must special-case ResourceNotFoundException as a synonym for DISABLED — which on real AWS means something else entirely (the resource id is not a VPC here). The stable implicit `Id` is also unobservable before the first update.

Fix: the GetResolverDnssecConfig handler plus the DNSSEC portion of the service state — on a read for a `ResourceId` that is a VPC of this account with no stored record, materialize (or synthesize) the default configuration with a stable minted `Id`, `OwnerId` = the emulator account, `ResourceId` = the request value and `ValidationStatus` = DISABLED, and keep ResourceNotFoundException for a `ResourceId` that is not a VPC of this account. The same default must be what ListResolverDnssecConfigs and UpdateResolverDnssecConfig see, so the `Id` does not change when the first update lands.

Traces: route53resolver.resolver-dnssec-config-lifecycle. Code: `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/get_resolver_dnssec_config.rs` lines 162-169 (the absent-record branch that raises the refusal via `crate::state::resolver_dnssec_config_not_found`); `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/update_resolver_dnssec_config.rs` lines 228-241 (the only place a record is ever created).

=== Cause 2 of 2: ListResolverDnssecConfigs is an unmodified scaffold stub and answers 501 NotImplemented ===
Failing check ids: route53resolver.resolver-dnssec-config-lifecycle

AWS: Smithy models `ListResolverDnssecConfigs` with a 200 response carrying `ResolverDnssecConfigs` (list of `ResolverDnssecConfig`) and `NextToken`; the recording enumerates the configs and requires the VPC's config `Id` to be present in that list after the enable.

Emulator: `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/list_resolver_dnssec_configs.rs` line 185 returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListResolverDnssecConfigs"))`, i.e. HTTP 501 with code `NotImplemented` and message `operation ListResolverDnssecConfigs is scaffolded but not implemented in this emulator`, and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` census header. This operation was NOT reached in this replay because Cause 1 aborted the check on its first call, and no issue exists for it (checked via the issue listing), so it is bundled here: the check stays red until it works.

Why: a client cannot enumerate DNSSEC-validation configurations at all — every listing-based read (console DNSSEC view, account-wide audit of which VPCs validate DNSSEC, paginating clients) fails, and a 501 is not a status any AWS SDK retry/error path models for this call.

Fix: implement the ListResolverDnssecConfigs handler over the same DNSSEC state the Get/Update handlers use (including the defaults from Cause 1), honouring `MaxResults`/`NextToken` paging, and delete that file's scaffold census header once implemented.

Traces: route53resolver.resolver-dnssec-config-lifecycle. Code: `services/service-com-amazonaws-route53resolver-route53resolver/src/ops/list_resolver_dnssec_configs.rs` line 185.

## Evidence
Check `route53resolver.resolver-dnssec-config-lifecycle` ([error] in suite simparity-behavioral-ga-prod-v1, neither ALREADY FILED nor OWNED).

Cause 1 — replay output:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResolverDnssecConfig operation: Resolver DNSSEC configuration for resource with ID "vpc-0000000000000001e" does not exist.
(the vpc- id is the VPC the check had just created with ec2 CreateVpc in this same emulator; it is a per-run value, the requirement is the behavior for any existing VPC)

  Expected (AWS, as recorded): HTTP 200, {"ResolverDNSSECConfig": {"Id": <stable minted id>, "OwnerId": <account>, "ResourceId": "<the VPC id>", "ValidationStatus": "DISABLED"}} — the check's first step polls GetResolverDnssecConfig for a settled status (ENABLED or DISABLED) BEFORE any UpdateResolverDnssecConfig call and asserts ResolverDNSSECConfig.ResourceId == the VPC id, then asserts the subsequent Update returns the same Id.
  Actual (emulator): HTTP 400, code ResourceNotFoundException, message `Resolver DNSSEC configuration for resource with ID "<ResourceId>" does not exist.`

  Emulator source: get_resolver_dnssec_config.rs:162-169 returns ResourceNotFoundException whenever `store.resolver_dnssec_configs.get(&input.resource_id)` is None; update_resolver_dnssec_config.rs:228-241 is the only writer of that map, so no VPC has a config until an update runs.

Cause 2 — list_resolver_dnssec_configs.rs:185 is still `Err(crate::scaffolded::Scaffolded::not_implemented("ListResolverDnssecConfigs"))` with the `//! SCAFFOLDED, NOT IMPLEMENTED:` header present.
  Expected (AWS): HTTP 200 with `ResolverDnssecConfigs` containing the VPC's config (the check asserts `any(c["Id"] == config_id for c in listed)`).
  Actual (emulator): HTTP 501, code NotImplemented, message `operation ListResolverDnssecConfigs is scaffolded but not implemented in this emulator`.

## Steps to Reproduce
1. Boot the emulator (route53resolver + ec2).
2. ec2 CreateVpc CidrBlock=10.90.0.0/16 -> note VpcId.
3. route53resolver GetResolverDnssecConfig ResourceId=<VpcId> with no prior UpdateResolverDnssecConfig for that VPC.
   Cause 1: observe HTTP 400 ResourceNotFoundException `Resolver DNSSEC configuration for resource with ID "<VpcId>" does not exist.` where AWS answers HTTP 200 with ValidationStatus DISABLED, ResourceId=<VpcId> and a stable Id.
4. route53resolver UpdateResolverDnssecConfig ResourceId=<VpcId> Validation=ENABLE, then ListResolverDnssecConfigs.
   Cause 2: observe HTTP 501 NotImplemented instead of a 200 listing that contains the config Id returned in step 4.
5. Replay `route53resolver.resolver-dnssec-config-lifecycle` to confirm both.
