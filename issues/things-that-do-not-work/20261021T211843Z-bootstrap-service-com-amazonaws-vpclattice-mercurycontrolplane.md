Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the VPC Lattice (mercurycontrolplane) emulator

_Filed: 2026-10-21T21:18:43Z_
_Operation: service-com-amazonaws-vpclattice-mercurycontrolplane | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 73 handlers in `services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and stores nothing, so no client can do anything with this service. Implement a FIRST, SMALL stateful slice: the service-network + service happy path, which is the core VPC Lattice workflow (protocol `restJson1`, sigv4 name `vpc-lattice`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateServiceNetwork** — `POST /servicenetworks` -> 201. Input: `name` (required, 3..63, `^(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$`), `authType` (AWS_IAM|NONE), `tags`, `sharingConfig`, `clientToken` (@idempotencyToken). Must mint a fresh `id` matching `^sn-[0-9a-z]{17}$` and an `arn` matching `arn:aws:vpc-lattice:<region>:<account>:servicenetwork/<id>`, persist the record, and echo back `id`/`name`/`arn`/`authType`/`sharingConfig` from what was stored.
2. **CreateService** — `POST /services` -> 201. Input: `name` (required, 3..40, `^(?!svc-)...$`), `customDomainName`, `certificateArn`, `authType`, `idleTimeoutSeconds` (60..600), `tags`, `clientToken`. Must mint `id` matching `^svc-[0-9a-z]{17}$` + matching service ARN, set `status` to `ACTIVE`, synthesize a `dnsEntry` (domainName + hostedZoneId) and persist all of it.
3. **CreateServiceNetworkServiceAssociation** — `POST /servicenetworkserviceassociations` -> 200. Input: `serviceIdentifier` (required), `serviceNetworkIdentifier` (required), `tags`. Must resolve BOTH identifiers (id or ARN form) against persisted records and return `ResourceNotFoundException` (404) when either is absent; mint `id` matching `^snsa-[0-9a-z]{17}$` + association ARN, `status` = ACTIVE, `createdBy` = account id, and link the two records so the service network's `numberOfAssociatedServices` reflects it.
4. **GetService** — `GET /services/{serviceIdentifier}` -> 200. Must look up by id OR ARN and return exactly what CreateService persisted: `id`, `name`, `arn`, `createdAt`, `lastUpdatedAt`, `dnsEntry`, `customDomainName`, `certificateArn`, `status`, `authType`, `idleTimeoutSeconds`. Unknown identifier -> `ResourceNotFoundException` (404).
5. **ListServices** — `GET /services` -> 200. Must return one `ServiceSummary` per persisted service (id, arn, name, status, createdAt, lastUpdatedAt, dnsEntry, customDomainName), honour `maxResults` (1..100) and round-trip `nextToken` so a second page continues where the first stopped. Newly created services must appear; deleted ones must not.
6. **DeleteServiceNetworkServiceAssociation** (`DELETE /servicenetworkserviceassociations/{id}` -> 200, returns `id`/`arn`/`status`), then **DeleteService** (`DELETE /services/{serviceIdentifier}` -> 200, returns `id`/`arn`/`name`/`status`) and **DeleteServiceNetwork** (`DELETE /servicenetworks/{serviceNetworkIdentifier}` -> 204, empty body). Deletes must actually remove state so a following Get returns 404 and List omits the resource. Deleting a service network that still has associations must return `ConflictException` (409).

Every response must be computed from persisted state — no canned bodies, no echoing the request, no minting an id that is never stored. Identifier inputs must be accepted in both bare-id and ARN form, and inputs violating the modeled length/pattern/range constraints must produce `ValidationException` (400) rather than being stored.

## State Model

In `src/state.rs` (`ComAmazonawsVpclatticeMercurycontrolplaneState`), reached only through `crate::state::service_state(&*self.engine)`:

- `service_networks: HashMap<String /* sn-id */, ServiceNetwork { id, arn, name, auth_type, sharing_config, tags: HashMap<String,String>, created_at, last_updated_at }>`
- `services: HashMap<String /* svc-id */, LatticeService { id, arn, name, custom_domain_name, certificate_arn, auth_type, idle_timeout_seconds, status, dns_entry: DnsEntry { domain_name, hosted_zone_id }, tags, created_at, last_updated_at }>`
- `service_associations: HashMap<String /* snsa-id */, ServiceNetworkServiceAssociation { id, arn, status, created_by, created_at, service_id, service_arn, service_name, service_network_id, service_network_arn, service_network_name, dns_entry, custom_domain_name, tags }>`
- `name_index: HashMap<String /* resource-type + name */, String /* id */>` so duplicate names can raise `ConflictException` (409)
- `idempotency_tokens: HashMap<String /* clientToken */, String /* created resource id */>` so a repeated `clientToken` on the @idempotent creates returns the first result instead of a second resource
- Derived, never stored stale: `numberOfAssociatedServices` / `numberOfAssociatedVPCs` on GetServiceNetwork are counted from `service_associations` / vpc associations at read time.

All other operations (listeners, rules, target groups, resource configurations/gateways, access log subscriptions, domain verification, auth/resource policies, tagging) stay at 501 for this slice and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-vpclattice-mercurycontrolplane/OPERATIONS.md` lists 73 operations with protocol `restJson1`, sdkId `VPC Lattice`; it defines the routes, members, constraints and errors cited above (CreateService lines 357-363, GetService 365-371, DeleteService 380-386, ListServices 388-394, CreateServiceNetwork 396-402, GetServiceNetwork 404-410, DeleteServiceNetwork 420-426, CreateServiceNetworkServiceAssociation 468-474, Get/Delete/List association 476-498). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/` matches all 73 handler files, including `create_service.rs`, `create_service_network.rs`, `create_service_network_service_association.rs`, `get_service.rs`, `list_services.rs`, `delete_service.rs`, `delete_service_network.rs` — each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). README.md confirms the scaffold contract and that state must live in the shared engine. No test exercises any of this today (only `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `vpc-lattice` at it.
2. `aws vpc-lattice create-service-network --name my-net` — expect 201 with `id` matching ^sn-[0-9a-z]{17}$ and a servicenetwork ARN; today HTTP 501 NotImplemented.
3. `aws vpc-lattice create-service --name my-svc` — expect 201 with `id` matching ^svc-[0-9a-z]{17}$, status ACTIVE and a dnsEntry.
4. `aws vpc-lattice create-service-network-service-association --service-identifier <svc-id> --service-network-identifier <sn-id>` — expect 200 with `id` matching ^snsa-[0-9a-z]{17}$ and status ACTIVE.
5. `aws vpc-lattice get-service --service-identifier <svc-id>` (and again with the service ARN) — expect the name/authType/idleTimeoutSeconds exactly as created, plus createdAt.
6. `aws vpc-lattice list-services --max-results 1` — expect one ServiceSummary plus a nextToken that pages to the rest.
7. `aws vpc-lattice delete-service-network --service-network-identifier <sn-id>` while the association exists — expect ConflictException (409).
8. Delete the association, then the service, then the service network; re-run get-service — expect ResourceNotFoundException (404) and list-services to no longer include it.
9. `aws vpc-lattice get-service --service-identifier svc-00000000000000000` for an unknown id — expect ResourceNotFoundException (404), not 501.
