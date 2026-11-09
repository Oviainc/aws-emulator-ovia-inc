Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b81c65908eae7ec5556d37c8b81c80a06cb22b3f

# The create's ClientToken fingerprint covers the raw FulfillmentTypes/AssociatedResources lists, so a retry of a request the handler stores identically is refused 409

_Filed: 2026-11-07T23:08:56Z_
_Operation: CreateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b81c65908eae7ec5556d37c8b81c80a06cb22b3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBenefitApplication` stores `FulfillmentTypes` and `AssociatedResources` as the SETS they denote (`crate::state::distinct_values`), so a request naming an entry twice and a request naming it once produce the byte-identical stored record. The `ClientToken` fingerprint, however, is computed over the RAW input lists, so those two requests hash differently.

Consequence: a client whose first create succeeded and whose retry spells an identity list without its duplicate entry (or with one) is answered with the modelled `ConflictException` (HTTP 409, `IDEMPOTENT_PARAMETER_MISMATCH`) instead of a replay of the first call's `Id`/`Arn`/`Revision` — even though the handler itself treats the two spellings as the same request everywhere else it reads those members.

This is the same defect shape the crate already corrected for the identifier members: `Catalog`/`BenefitIdentifier`/`Identifier` are fingerprinted by the RECORD they name rather than by the spelling they were written with (`store.benefit_identity(..)`, `store.application_identity(..)`), while the two identity LISTS whose duplicate spelling the same handler explicitly discards are still fingerprinted verbatim. Order is genuinely part of the stored record (first-appearance order), so only the duplicate-entry spelling is at issue.

## Evidence

The handler states the storage rule for these two members:

`src/ops/create_benefit_application.rs:48-51`:
```
//! `AssociatedResources` and `FulfillmentTypes` are lists of IDENTITIES, so each is stored
//! as the SET it denotes: a resource ARN or a fulfillment type the request names twice is
//! stored once, in the order the request first named it.
```
and applies it when storing:
`src/ops/create_benefit_application.rs:482`: `fulfillment_types: crate::state::distinct_values(input.fulfillment_types),`
`src/ops/create_benefit_application.rs:499`: `associated_resources: crate::state::distinct_values(input.associated_resources),`

`src/state.rs:3203-3205` (the helper's own doc) says the two spellings carry the same information:
```
/// is the whole of what one carries: naming the same resource or the same fulfillment type
/// twice says nothing a single entry does not.
```

But the request fingerprint is taken over the RAW lists — `src/ops/create_benefit_application.rs:336-341` and `:353-359`:
```
            (
                "FulfillmentTypes",
                input
                    .fulfillment_types
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),
...
            (
                "AssociatedResources",
                input
                    .associated_resources
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),
```
while the SAME list of fingerprint members resolves the identifier members to the record they name (`src/ops/create_benefit_application.rs:313-323`): `Some(crate::state::fingerprint_value(&store.benefit_identity(&input.catalog, &region, &input.benefit_identifier)))`, justified at `src/ops/create_benefit_application.rs:308-312`: "the `BenefitIdentifier` is compared as the benefit it NAMES rather than the spelling it was written with".

`src/state.rs:2342-2345` states the fingerprint's own contract, which this breaks:
```
/// when the request omitted it. An omitted member is left out of the digest, so omitting
/// a member and supplying one never fingerprint alike, and the rendering is canonical
/// ([`document_json`] sorts an object's members), so two spellings of one request agree.
```
A mismatch then becomes the 409: `src/state.rs:2382-2383` (`replayed_write`) -> `Some(_) => Err(idempotency_conflict())`, reached from `src/ops/create_benefit_application.rs:383-388`.

Test coverage: `tests/all/create_identity_list_sets.rs:85-135` pins that a create naming an ARN two or three times stores it once (each row uses a distinct `ClientToken`, e.g. `format!("tok-create-set-{row}")` at line 142), so nothing there exercises the replay. `tests/all/idempotent_parameter_mismatch.rs:208-221` varies `AssociatedResources` only by MEMBERSHIP ("fewer associated resources") and by ORDER ("the same associated resources in another order") — both genuinely different stored records — and no row varies a repeated entry, so this spelling is unpinned by any test.

## Steps to Reproduce

1. Call `CreateBenefitApplication` with `{"Catalog":"AWS","ClientToken":"tok-dup","BenefitIdentifier":"<an ACTIVE benefit of AWS>","AssociatedResources":["arn:aws:s3:::partner-proposals","arn:aws:s3:::partner-proposals"]}` — it answers 200 with an `Id` and `Revision` `1`.
2. Call `GetBenefitApplication` for that `Id`: `AssociatedResources` carries the ARN ONCE (the set the create stored).
3. Retry the create with the SAME `ClientToken` and the list the record now holds: `"AssociatedResources":["arn:aws:s3:::partner-proposals"]`.
4. Expected, by the crate's own rules (`distinct_values` says the two spellings say the same thing; `request_fingerprint` says two spellings of one request agree): the retry is replayed and answers the first call's `Id`, `Arn` and `Revision`.
5. Actual: HTTP 409 `ConflictException` with `crate::state::IDEMPOTENT_PARAMETER_MISMATCH`, because the fingerprint covered `["arn","arn"]` for the first call and `["arn"]` for the retry.
6. The same holds for `FulfillmentTypes`: first call with `["CASH","CASH"]`, retry with `["CASH"]` under the same token.
