Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# ListTagsForResource answers 404 for a domain that is still readable in DELETING state

_Filed: 2026-10-24T07:51:22Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

While a domain is in the non-terminal `DELETING` state the emulator reports it as an existing resource on every other read path — `GetDomain` answers 200 with `status: DELETING` and its `tags` map, `ListDomains?status=DELETING` lists it, and a repeated `DeleteDomain` is accepted idempotently — but `ListTagsForResource` on that same domain ARN answers `ResourceNotFoundException` (HTTP 404) and the resource's tags become unreadable for the duration of the deletion window.

Expected: `ListTagsForResource` resolves any resource ARN the service still holds and reports its tag map; it should stop resolving only once the domain record is actually purged (at which point 404 is correct and is already covered by the existing tagging tests).

Actual: the handler gates on `state::holds_resource_arn`, which requires `domain.status == DomainStatus::Active`, so a domain whose deletion is in flight (and every subscription under it) is treated as nonexistent by the tag reader while the domain readers still return it. The two readers disagree about whether the same resource exists.

## Evidence

Spec/doc: `ListTagsForResource` is documented "Lists the tags associated with the specified resource" (smithy_query digest, `GET /tags/{resourceArn}`), and `DomainStatus` models `DELETING` as a distinct, non-terminal state from `DELETED` (smithy_query shape=DomainStatus), i.e. a DELETING domain is still a resource of the service — which the emulator itself asserts by answering `GetDomain` 200 for it.

Handler: `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_tags_for_resource.rs:170` refuses the request when `!store.holds_resource_arn(&input.resource_arn)`; `src/state.rs:518-527` implements `holds_resource_arn` as `domain.status == DomainStatus::Active && (...)`. By contrast `src/ops/get_domain.rs:202` looks the domain up with `store.domains.get(..)` (no liveness filter) and returns 200 plus `tags` for a DELETING domain.

Test coverage: the DELETING window is exercised for the domain readers (`tests/all/domain_subscription_lifecycle.rs:1073,1097` assert `DeleteDomain` reports `DELETING` and that `GET /domain?status=DELETING` lists it; `tests/all/domain_response_projection.rs:259,357`), but no test drives `ListTagsForResource` (or `GetDomain`'s `tags`) during that window — `tests/all/list_tags_for_resource.rs` / `tests/all/resource_tagging.rs` only cover a live domain and a fully purged one, so this state is unasserted.

## Steps to Reproduce

1. CreateDomain with a `webAppSetupConfiguration` and `tags: {"env": "prod"}` — a web-app domain is the one whose deletion cannot finish inside the call, so it passes through `DELETING` (see `Domain::deletion_completes_in_call`).
2. Note the returned `domainId` and build its ARN (or read `arn` from the create response).
3. Call `DELETE /domain/{domainId}` and observe the 200 reporting `status: DELETING`.
4. Immediately (inside the `state::DOMAIN_DELETION_SPAN` window, before the record is purged) call `GET /domain/{domainId}`: 200, `status: DELETING`, `tags` reported.
5. Immediately call `GET /tags/{domainArn}`: the emulator answers 404 `ResourceNotFoundException` (`Resource not found: <arn>`) for the resource the previous call just reported as existing — expected is 200 with the stored tag map until the domain is actually purged.
