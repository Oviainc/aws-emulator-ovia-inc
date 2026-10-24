Issue-Severity: HIGH
Issue-Gap-Class: pagination
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# An unasked ListServices page holds 10 services where AWS returns the whole integrated-service list

_Filed: 2026-10-24T05:43:53Z_
_Operation: ListServices | Class: pagination_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServices` with an empty body returns only the first 10 `ServiceInfo` entries plus a `NextToken`.

Expected (AWS): `ListServices {}` returns the integrated-service list in one response — the recorded AWS answer carries entries at index 15 (`app-integrations`), 22 (`appstream2`), 25 (`aps`) and on for ~130 further indices, so AWS's default page for this operation is at least ~130 items, not 10.

Actual (emulator): `src/ops/list_services.rs` lines 116-130 pages the catalog through `crate::state::paginate` with `input.max_results = None`, which substitutes `crate::state::UNASKED_PAGE_SIZE` = 10 (`src/state.rs` line 202). A client that calls `ListServices` once — the documented way to discover a `ServiceCode` before any other call — sees 10 services and must follow dozens of tokens to find the rest, where AWS answered in one call.

## Evidence

Model: `ListServices` is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Services)`, `MaxResults` `@range(1..100)` with no declared default (OPERATIONS.md lines 128-133).

Handler: `src/ops/list_services.rs` lines 115-130 (`state.lock().services()` then `paginate(Self::NAME, services, input.max_results, input.next_token)`); the unasked size is `UNASKED_PAGE_SIZE` = 10 in `src/state.rs` lines 182-202 and 1452-1454, documented there as a CHOSEN number.

Recorded AWS behaviour, quoted in the still-open issue `20261023T182924Z-service-quotas-default-quota-catalog-is-incomplete-and`, cause 10 and its repro step 6: the fixture POSTs `ListServices` with `{}` and the recorded (expected) body carries `$.Services[15] = app-integrations`, `$.Services[22] = appstream2`, `$.Services[25] = aps` with "the diff continu[ing] for ~130 indices" — a single unasked AWS response of well over 10 entries. That issue covers only the CONTENT of the inventory (ServiceCode set and ServiceName strings), not the page size, so this gap is net-new.

Test coverage: no test asserts the AWS page size; `tests/all/default_quota_catalog.rs` lines 361-371 pin the 10-item unasked page for the sibling listings instead.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.ListServices` with body `{}`.
2. Count `$.Services`: exactly 10 entries come back with a non-null `$.NextToken`.
3. Compare with the recorded AWS response to the same request, which carries the integrated-service list (entries at index 15/22/25 and ~130 further indices) in one body.
