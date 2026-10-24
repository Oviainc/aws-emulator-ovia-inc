Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Domains and subscriptions are not Region-scoped: a resource created in one Region is listed, read and deleted from another

_Filed: 2026-10-24T08:24:53Z_
_Operation: ListDomains | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ConnectHealth (`health-agent`) is a regional service: its endpoint is regional (`endpointPrefix: health-agent`, every modelled ARN carries a `[a-z0-9-]+` region segment), so a domain created against the us-east-1 endpoint does not exist for a caller addressing eu-west-1.

The emulator keeps ONE global, region-less store. `crate::state::Domain` has no region field at all (`domain_id`, `arn`, `account_id`, `onboarding_generation`, `name`, `kms_key_arn`, `encryption_type`, `status`, `purge_due_at`, `deleted_at`, `web_app_url`, `web_app_configuration`, `created_at`, `subscriptions`); the only place the request's region is used is at mint time, where `crate::state::region_of(ctx)` (read from the SigV4 credential scope) is baked into the ARN and the web-app URL. Lookups key on `domain_id` alone (`store.domains.get(..)`, `store.live_domain(..)`), so region is never part of resolution.

Observable divergences, all on the same root cause:
- `ListDomains` signed for region B enumerates every domain created in region A, each reported with its region-A `arn` (a `DomainSummary.arn` whose region segment contradicts the endpoint the caller used).
- `GetDomain` / `DeleteDomain` / `CreateSubscription` / `ListSubscriptions` / `GetSubscription` / `ActivateSubscription` / `DeactivateSubscription` all resolve a cross-region `domainId` and answer 200 where the real regional endpoint answers `ResourceNotFoundException` (404) — a caller can mutate and delete a resource that does not live in the Region it is talking to.
- The account-wide `CreateDomain` cap (`MAX_DOMAINS_PER_ACCOUNT = 10`, surfaced as a `ValidationException` naming the account) is counted across ALL regions, so 10 domains spread over two regions wrongly exhaust the quota the real service applies per account per Region.

Expected: a domain (and everything under it, including its tag entries) is visible only to requests signed for the Region it was created in; a lookup from another Region answers the modelled `ResourceNotFoundException`, and quotas are counted per Region.

## Evidence

Spec: the model's `DomainArn`/`SubscriptionArn` patterns embed a region segment (`^arn:aws:health-agent:[a-z0-9-]+:[0-9]{12}:domain/...`), and `smithy_query(query="service_info")` gives endpointPrefix/arnNamespace `health-agent` — i.e. a per-Region endpoint, which is well-established AWS behavior for a regional control-plane service.

Handler/state: `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs` — `Domain` carries no region member; `region_of(ctx)` (state.rs:723) is consumed only by `domain_arn`/`web_app_url` at creation (`src/ops/create_domain.rs:217,270,294`). Every reader ignores region entirely: `src/ops/get_domain.rs:202` (`store.domains.get(&input.domain_id)`), `src/ops/list_domains.rs:207-215` (iterates `store.domains.values()` with only a `status`/token filter), `src/ops/delete_domain.rs:327`, `state.rs:564-576` (`live_domain`/`live_domain_mut`). `domains_held_by(&account_id)` is likewise region-blind, so `MAX_DOMAINS_PER_ACCOUNT` is a cross-region cap.

Test coverage: none. The suite builds a fresh engine per test (`tests/all/domain_subscription_lifecycle.rs:57-67`, `Emulator::new(region)`) and signs every request of that test for the one region, so no test ever reads a resource from a second Region. `minted_ids_and_arns_match_the_model_patterns_in_every_signed_region` (line 441) only asserts the ARN carries the signing region; isolation across regions is unasserted.

## Steps to Reproduce

1. Sign a `POST /domain` request for us-east-1 (Authorization credential scope `.../us-east-1/health-agent/aws4_request`) with body {"name": "clinic"}; note the returned `domainId` and the `arn` (which carries `us-east-1`).
2. Sign `GET /domain` (ListDomains) for eu-west-1 against the same emulator: the us-east-1 domain is listed, with its `arn` naming us-east-1 — expected is an empty `domains` list for a Region that holds no domain.
3. Sign `GET /domain/{domainId}` for eu-west-1: 200 with the full domain and its us-east-1 ARN, instead of `ResourceNotFoundException` (404).
4. Sign `POST /domains/{domainId}/subscriptions` for eu-west-1: 201, minting a subscription (with a us-east-1-rooted ARN) under a domain that does not exist in eu-west-1.
5. Sign `DELETE /domain/{domainId}` for ap-south-1: the domain created in us-east-1 is deleted from an unrelated Region.
6. Create 10 domains spread over two Regions (e.g. 5 signed for us-east-1 and 5 for us-west-2), then attempt an 11th in either Region: the request is refused with `ValidationException` ("Account <id> has reached the maximum limit of 10 domains.") even though neither Region holds 10.
