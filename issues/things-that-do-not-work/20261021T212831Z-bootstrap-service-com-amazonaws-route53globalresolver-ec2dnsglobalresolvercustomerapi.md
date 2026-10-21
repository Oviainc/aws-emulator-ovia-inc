Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the Route53GlobalResolver (EC2 DNS Global Resolver Customer API) emulator

_Filed: 2026-10-21T21:28:31Z_
_Operation: service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 48 operations in `src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state at all. Implement a FIRST, SMALL, stateful happy-path slice around the service's primary resources — the Global Resolver and the DNS View it contains (restJson1, sigv4 signing name `route53globalresolver`).

### Happy path to implement (in order)

1. **CreateGlobalResolver** — `POST /global-resolver`. Input: `name` (required, pattern `^(?!^[0-9]+$)([a-zA-Z0-9-_/' ']+)$`, 1..64), `regions` (required list<string>), optional `description`, `ipAddressType` (`IPV4` | `DUAL_STACK`), `observabilityRegion`, `tags`, and `clientToken` (`@idempotencyToken`). Must mint and PERSIST a new resolver and return the stored record: `id` (matching `^[-.a-zA-Z0-9]+$`), `arn` (`arn:aws:route53globalresolver:<region>:<account>:global-resolver/<id>`), `clientToken` (required in output — echo the supplied one or a generated UUID), `dnsName`, `ipv4Addresses` (required, non-empty, synthesized anycast addresses), `ipv6Addresses` only when `ipAddressType=DUAL_STACK`, `name`, `description`, `regions`, `observabilityRegion`, `ipAddressType`, `status` (`CRResourceStatus`: start at `CREATING` or go straight to `OPERATIONAL`), `createdAt`, `updatedAt`. Because the op is `@idempotent` with an `@idempotencyToken`, a repeat call with the same `clientToken` must return the SAME resolver, not a second one.
2. **GetGlobalResolver** — `GET /global-resolver/{globalResolverId}`. Must look the id up in state and return every field exactly as stored by step 1; unknown id → `ResourceNotFoundException` (HTTP 404), never a canned 200.
3. **ListGlobalResolvers** — `GET /global-resolver`. Returns `globalResolvers` (required list<GlobalResolversItem>) built from the stored map; honor `maxResults` / `nextToken` (`@paginated` inputToken=nextToken, outputToken=nextToken, items=globalResolvers) by returning at most `maxResults` items plus a `nextToken` that resumes where it left off, and omit `nextToken` on the last page.
4. **CreateDNSView** — `POST /dns-views/{globalResolverId}`. Input: `name` (required), `dnssecValidation`/`ednsClientSubnet`/`firewallRulesFailOpen` (each `DISABLED` default, required in output), optional `description`, `tags`, `clientToken`. Must reject an unknown `globalResolverId` with `ResourceNotFoundException` (404), persist the view under its parent resolver, and return `id`, `arn`, `globalResolverId`, `name`, `description`, the three toggles as resolved (defaults applied), `status` (`ProfileResourceStatus`), `createdAt`, `updatedAt`, `clientToken`.
5. **GetDNSView** / **ListDNSViews** — `GET /dns-views/{dnsViewId}` returns the persisted view (404 for unknown id); `GET /dns-views/resolver/{globalResolverId}` returns `dnsViews` (required) containing exactly the views created under that resolver, with `maxResults`/`nextToken` honored and 404 for an unknown resolver id.
6. **DeleteGlobalResolver** — `DELETE /global-resolver/{globalResolverId}`. Returns the final record of the deleted resolver (same shape as Get, with `status` = `DELETING`), then REMOVES it from state so a subsequent GetGlobalResolver/ListGlobalResolvers no longer sees it (404 / absent from the list). A second delete of the same id → `ResourceNotFoundException` (404).

Every one of these must compute its response from persisted state held in the single shared engine store reached via `crate::state::service_state(&*self.engine)` — NO canned bodies, NO echoing the input back without storing it, NO random id without a backing record. A create followed by a get/list must return what was created; a delete must be observable by the next reader. `DeleteDNSView`, `EnableDNSView`/`DisableDNSView`, access sources/tokens, hosted-zone associations, the firewall domain-list/rule families and tagging operations stay 501 in this slice and will be filed as individual gap issues later.

## State Model

In `src/state.rs` (`ComAmazonawsRoute53globalresolverEc2dnsglobalresolvercustomerapiState`):

- `global_resolvers: BTreeMap<String /* id */, GlobalResolver>` — insertion-ordered-by-id map so list output is deterministic.
  - `GlobalResolver { id: String, arn: String, name: String, description: Option<String>, client_token: String, dns_name: String, regions: Vec<String>, observability_region: Option<String>, ip_address_type: GlobalResolverIpAddressType /* IPV4 | DUAL_STACK */, ipv4_addresses: Vec<String>, ipv6_addresses: Vec<String>, status: CRResourceStatus /* CREATING | UPDATING | OPERATIONAL | DELETING */, created_at: SystemTime, updated_at: SystemTime, tags: BTreeMap<String,String>, dns_view_ids: Vec<String> }`
- `dns_views: BTreeMap<String /* id */, DnsView>`
  - `DnsView { id: String, arn: String, global_resolver_id: String, name: String, description: Option<String>, client_token: Option<String>, dnssec_validation: DnsSecValidationType, edns_client_subnet: EdnsClientSubnetType, firewall_rules_fail_open: FirewallRulesFailOpenType, status: ProfileResourceStatus, created_at: SystemTime, updated_at: SystemTime, tags: BTreeMap<String,String> }`
- `idempotency: HashMap<String /* clientToken */, String /* created resource id */>` — makes the `@idempotent` creates return the same resource on replay.
- `next_id: u64` (or UUID source) — ids must match `^[-.a-zA-Z0-9]+$` and be unique across the store.

Done = a client script can run CreateGlobalResolver → GetGlobalResolver → ListGlobalResolvers → CreateDNSView → GetDNSView/ListDNSViews → DeleteGlobalResolver against the emulator with every response body reflecting the state the previous calls established.

## Evidence

Spec: `services/service-com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi.json`) documents all 48 operations, protocol `restJson1`, SDK id `Route53GlobalResolver`, and the exact routes/members cited above: CreateGlobalResolver `POST /global-resolver` (@idempotent; required `name`, `regions`; output requires `id`, `arn`, `clientToken`, `dnsName`, `ipv4Addresses`, `regions`, `status`, `createdAt`, `updatedAt`), GetGlobalResolver `GET /global-resolver/{globalResolverId}` (@readonly, `ResourceNotFoundException` 404), ListGlobalResolvers `GET /global-resolver` (@paginated inputToken=nextToken/outputToken=nextToken/items=globalResolvers), CreateDNSView `POST /dns-views/{globalResolverId}` (required `name`, defaults `DISABLED` for `dnssecValidation`/`ednsClientSubnet`/`firewallRulesFailOpen`, 404 modeled), GetDNSView `GET /dns-views/{dnsViewId}`, ListDNSViews `GET /dns-views/resolver/{globalResolverId}` (@paginated, items=dnsViews), DeleteGlobalResolver `DELETE /global-resolver/{globalResolverId}` (@idempotent, 404/409 modeled).

Handler proof: `src/ops/create_global_resolver.rs` lines 4 and 223 — header `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented` and body `Err(crate::scaffolded::Scaffolded::not_implemented("CreateGlobalResolver"))`. All 49 files in `src/ops/` are the same generated scaffold, so no state is created or read anywhere in the crate. No tests assert any of this behaviour (`tests/state_roundtrip.rs` is only a compiled example of the state API).

## Steps to Reproduce

1. Start the emulator and point an AWS client at it with signing name `route53globalresolver`, region `us-east-2`.
2. `POST /global-resolver` with `{"name":"demo-resolver","regions":["us-east-2","eu-west-1"],"ipAddressType":"DUAL_STACK","clientToken":"tok-1"}` — expect 200 with a generated `id`, `arn`, `dnsName`, non-empty `ipv4Addresses` and `ipv6Addresses`, `status`, `createdAt`/`updatedAt`. Actual: HTTP 501 NotImplemented.
3. Repeat the same POST with `clientToken: "tok-1"` — expect the SAME `id` back (idempotent create), not a second resolver.
4. `GET /global-resolver/{id}` — expect 200 echoing exactly the persisted resolver (name `demo-resolver`, both regions, DUAL_STACK addresses).
5. `GET /global-resolver?max_results=1` — expect exactly one item plus a `nextToken`; follow the token to get the remaining resolvers and no token on the final page.
6. `POST /dns-views/{id}` with `{"name":"demo-view","dnssecValidation":"ENABLED"}` — expect 200 with a new view `id`, `globalResolverId` = the resolver id, `dnssecValidation` `ENABLED`, and the other two toggles defaulted to `DISABLED`.
7. `GET /dns-views/{dnsViewId}` and `GET /dns-views/resolver/{id}` — expect the stored view returned, and the list containing exactly that one view.
8. `POST /dns-views/does-not-exist` and `GET /global-resolver/does-not-exist` — expect `ResourceNotFoundException` with HTTP 404.
9. `DELETE /global-resolver/{id}` — expect 200 with the final record (`status` `DELETING`); then `GET /global-resolver/{id}` must return 404 and `GET /global-resolver` must no longer list it; a second DELETE must return 404.
