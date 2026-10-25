Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# visibleServices accepts (and persists) identifiers the service catalog does not name

_Filed: 2026-10-24T10:12:53Z_
_Operation: UpdateAccountCustomizations | Class: validation_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a `visibleServices` member that satisfies the modelled `Service` constraints (`@length(1,64)`, `@pattern ^[a-z0-9]+(-[a-z0-9]+)*$`) but is not a real Amazon Web Services console service identifier is refused with HTTP 400 `ValidationException` (recorded message example: "Invalid services provided") and the previously persisted `visibleServices` list is left intact — the model's own member doc makes catalog membership part of the value's validity ("For valid service identifiers, call ListServices"), and the recorded AWS behavior for a made-up-but-pattern-legal identifier is a rejection.

Actual: the handler performs no membership check at all. `services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs` lines 175-188 state in a comment that `visibleServices` is deliberately NOT checked against the catalog `ListServices` pages over, and the write path (lines 203-229) stores whatever the body supplied. A PATCH with `{"visibleServices": ["totally-made-up-service-xyz"]}` therefore answers HTTP 200, overwrites a previously stored list, and `GetAccountCustomizations` reports the bogus identifier afterwards. This is an over-permissive accept: a client that relies on the service to reject unknown identifiers persists garbage against the emulator and only discovers the rejection in production.

Note for the fixer: this was previously filed as Cause 3 of the now-`fixed` issue `20261022T041007Z-uxc-updateaccountcustomizations-wrong-error-code-for-body-validation-refusals.md`, but the behavior is back in the current checkout — the handler and `src/state.rs` (lines 117-120) now argue the catalog is a "DISCOVERY list, not an admission list", and the test suite actively pins the acceptance. The error-code half of that old issue (body shape violations answering `BadRequestException`) IS implemented and is not re-reported here; only the missing membership refusal is. Any fix must keep the membership refusal in the modelled `ValidationException` channel (400), distinct from the front door's `BadRequestException` for shape violations, and must not mutate state before refusing. The same code path stores `visibleRegions` with no check beyond the member `@pattern`, so a pattern-legal but nonexistent Region code (e.g. `us-abcdefghij-1`) is likewise persisted; that half is unverified against a recording and is secondary.

## Evidence

Spec: `smithy_query(query="operation", operation="UpdateAccountCustomizations")` — `visibleServices` targets `ServiceList` (max 500, member `Service` `@length(1,64)` `@pattern ^[a-z0-9]+(-[a-z0-9]+)*$`), documented as "The list of Amazon Web Services service identifiers to make visible ... For valid service identifiers, call ListServices"; the operation declares `ValidationException` (400). Recorded AWS behavior: behavioral check `uxc.arbitrary-pattern-valid-service-id-accepted-without-existence-check` (cited in the fixed issue 20261022T041007Z) shows AWS refusing `visibleServices=['totally-made-up-service-xyz']` with 400 `ValidationException` while the emulator answered 200 and persisted it.
Handler: `src/ops/update_account_customizations.rs` lines 175-188 (explicit "is NOT checked against the catalog" comment) and 203-229 (unconditional store), with the catalog itself in `src/state.rs` (`SEED_SERVICE_CATALOG`, lines 117-348).
Tests: coverage asserts the OPPOSITE of the recorded behavior — `tests/all/visible_services_round_trip.rs::an_identifier_the_catalog_omits_is_accepted_and_read_back` (lines 233-296) and `::neither_a_catalog_entry_nor_a_near_miss_of_one_is_refused` (lines 302-352) require HTTP 200 for `zzqq-no-such-service`, a 64-'a' identifier and every catalog entry with a digit appended; `tests/all/body_validation_refusals.rs` only covers shape-constraint refusals. No test asserts the catalog-membership refusal.

## Steps to Reproduce

1. Boot the emulator from this checkout.
2. PATCH /v1/account-customizations with body {"visibleServices": ["s3"]} and observe HTTP 200 storing ["s3"].
3. PATCH /v1/account-customizations with body {"visibleServices": ["totally-made-up-service-xyz"]} — an identifier that satisfies Service's @pattern/@length but that GET /v1/services never reports.
4. Observe HTTP 200 with the bogus identifier echoed, where AWS answers HTTP 400 ValidationException ("Invalid services provided") and leaves the stored list unchanged.
5. GET /v1/account-customizations and observe visibleServices = ["totally-made-up-service-xyz"] — the prior ["s3"] was overwritten by a call AWS rejects.
6. Optionally walk GET /v1/services to confirm the identifier is absent from the catalog the service itself publishes.

## Resolution

Fixed in `services/service-com-amazonaws-uxc-awsaccountuxsetting`.

- **Observable difference:** `PATCH /v1/account-customizations` with `visibleServices` naming an identifier that satisfies `Service`'s `@length(1, 64)` and `@pattern` but that `GET /v1/services` never reports answered HTTP 200, overwrote the previously stored list, and `GetAccountCustomizations` then reported the made-up identifier; expected HTTP 400 `ValidationException` ("Invalid services provided") with the stored list intact.
- **Rule:** where a setting's members are documented as identifiers of a catalog the service itself enumerates, membership in that catalog is part of a member's validity, so a member the catalog does not name is refused in the modelled validation channel before any state is written.
- **Root cause:** `src/ops/update_account_customizations.rs` deliberately performed no membership check (its own comment said so) and the write path stored whatever the body supplied; `src/state.rs` carried the catalog but exposed no membership predicate.

The handler now refuses, under the same state guard as the write and before reaching the account's record, any supplied `visibleServices` member for which the new `ComAmazonawsUxcAwsaccountuxsettingState::is_catalog_service` is false — HTTP 400, modelled `ValidationException`, message "Invalid services provided", no `fieldList` (no member's own modelled constraint was broken), and no setting of the refused request applied. Shape violations keep the front door's `BadRequestException`, so the two channels stay distinct.

`visibleRegions` is unchanged: this service publishes no catalog of Region codes (no operation enumerates them and the model offers only the member's `@pattern`), so there is nothing to decide membership against and a refusal there would be against a list no caller can discover.

Pinned by the new `tests/all/visible_service_catalog_membership.rs` (a table spanning the form, position, count and list size of non-members — generated identifiers, both `@length` bounds, invented names, near misses of catalog entries spread across the catalog, and endpoint prefixes the console namespace does not name — each asserted model-legal and absent from the catalog first; plus the entry-versus-near-miss contrast, the front-door-code contrast, and the reset case). The suites that pinned the old acceptance now draw their service identifiers from the catalog.

