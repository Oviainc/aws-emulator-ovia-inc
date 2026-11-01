// SPDX-License-Identifier: CC0-1.0

//! The store's single application-write path is ALL-OR-NOTHING.
//!
//! The rule under test: storing a record and storing the indexes that NAME that record are
//! one step, taken only once the container the record lives in is known to be there — so
//! the store either holds the record and every index entry for it, or holds none of them,
//! and no index of this store ever names a record the store does not hold.
//!
//! For this service the record is a benefit application, its container is the catalog, and
//! the indexes are the ARN index (`arns`) and the spent-`ClientToken` index
//! (`client_tokens`), both store-wide. A write that left a token entry behind without the
//! application would poison that token: the token index is what
//! `CreateBenefitApplication` reads FIRST, so a later create reusing it would be refused
//! `ConflictException` for an application that was never created, while a create repeating
//! the stranded request exactly would mint a second application under the same token.
//!
//! The tables' rows differ along what the rule generalises over: whether the catalog is one
//! the store holds (both of them) or one it does not, the FORM of the catalog name (several
//! the modelled `CatalogName` pattern `^[A-Za-z0-9_-]+$` admits, a seeded name in another
//! case, and the empty name an in-process caller can pass), the form and length of the
//! `ClientToken` (`^[!-~]{1,64}$`: the shortest, a uuid, punctuation, the longest), whether
//! that token is one a create has already spent, and whether the record carries the
//! identity of an application the store already holds.
//!
//! The write path is reached in process — as `tests/all/benefit_allocations.rs` reaches the
//! out-of-API grant — because no request can name a catalog this store does not hold: the
//! create refuses one under the same lock. What the rule protects is still asserted over
//! the wire: the application is read back through `GetBenefitApplication` (by id and by
//! ARN), the ARN of a record that was not stored names no resource for
//! `ListTagsForResource`, and a `ClientToken` the refused write carried is still spendable
//! by a real `CreateBenefitApplication` exactly once.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    application_arn, service_state, BenefitApplication, APPLICATION_ID_PREFIX, ID_BODY_LEN,
    IDEMPOTENT_PARAMETER_MISMATCH, MAX_CLIENT_TOKEN_LEN, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG, REGION};
use crate::service_error_rules::message_of;

/// The `ClientToken` the one application every table starts from was created with, so a row
/// can carry a token a create has ALREADY spent.
const SPENT_TOKEN: &str = "a-create-that-really-happened";

/// One row of the unheld-catalog table.
struct Orphan {
    case: &'static str,
    /// The `Catalog` the record names: a catalog this store holds none of.
    catalog: String,
    /// The `ClientToken` the write carries.
    token: String,
    /// Whether that token is one a create already spent, so the row also pins that a
    /// refused write does not overwrite a live index entry.
    token_already_spent: bool,
    /// Whether the record carries the id (and so the ARN) of an application the store
    /// already holds, so a leaked write would clobber a stored record rather than add one.
    reuses_stored_identity: bool,
}

/// Every unheld-catalog row: the catalog-name forms crossed with the `ClientToken` forms
/// the modelled `^[!-~]{1,64}$` admits.
fn orphan_rows() -> Vec<Orphan> {
    let longest = "t".repeat(MAX_CLIENT_TOKEN_LEN);
    vec![
        Orphan {
            case: "a plausible catalog the service does not serve",
            catalog: "Production".to_string(),
            token: "orphan-production".to_string(),
            token_already_spent: false,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "a seeded catalog's name in another case, under the shortest token",
            catalog: "aws".to_string(),
            token: "o".to_string(),
            token_already_spent: false,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "a hyphenated name, under a uuid token",
            catalog: "held-by-nothing-20261029".to_string(),
            token: "9ec1f0e4-2f6b-4a7d-8f0e-5b3c1d2a4e6f".to_string(),
            token_already_spent: false,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "digits only, under a token of punctuation the pattern admits",
            catalog: "20261029".to_string(),
            token: "orphan!2026-10-29T12:52:13Z/1".to_string(),
            token_already_spent: false,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "an underscored name, under the longest token the model admits",
            catalog: "held_by_nothing".to_string(),
            token: longest,
            token_already_spent: false,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "the empty catalog name",
            catalog: String::new(),
            token: "orphan-empty".to_string(),
            token_already_spent: false,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "a token a create already spent",
            catalog: "Production".to_string(),
            token: SPENT_TOKEN.to_string(),
            token_already_spent: true,
            reuses_stored_identity: false,
        },
        Orphan {
            case: "the identity of an application the store already holds",
            catalog: "Production".to_string(),
            token: "orphan-reusing-a-stored-identity".to_string(),
            token_already_spent: false,
            reuses_stored_identity: true,
        },
    ]
}

/// A `benappl-...` id of the modelled form (`^benappl-[0-9a-z]{14}$`, from
/// `smithy_query(query="shape",shape="BenefitApplicationId")`), distinct per `seed` and
/// produced by no mint of this store.
fn probe_id(seed: usize) -> String {
    format!("{APPLICATION_ID_PREFIX}{seed:0>width$}", width = ID_BODY_LEN)
}

/// What this store holds, rendered whole: every catalog with the applications in it, the
/// ARN index and the spent-`ClientToken` index. Two instants of it compare as ONE value, so
/// an assertion says "nothing was stored" over the whole store rather than field by field.
///
/// `arns` is a `HashMap`, so it is re-keyed through a `BTreeMap` for one stable rendering.
fn store_snapshot(harness: &Harness) -> String {
    let state = service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state");
    let store = state.lock();
    let arns: BTreeMap<&String, &(String, String)> = store.arns.iter().collect();
    format!(
        "catalogs={:?} arns={arns:?} client_tokens={:?}",
        store.catalogs, store.client_tokens
    )
}

/// How many applications the whole store holds, across every catalog.
fn application_count(harness: &Harness) -> usize {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .catalogs
        .values()
        .map(|catalog| catalog.applications.len())
        .sum()
}

/// Take the store's single application-write path directly, as the create's own call site
/// does.
fn insert(
    harness: &Harness,
    client_token: &str,
    fingerprint: &str,
    application: BenefitApplication,
) {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .insert_application(client_token.to_string(), fingerprint.to_string(), application);
}

/// What the token index answers for `client_token` on a request of identity `fingerprint`:
/// the id of the application the create holding it minted, `None` when no create holds it,
/// or the message of the refusal it is a mismatch with.
fn replayed(
    harness: &Harness,
    client_token: &str,
    fingerprint: &str,
) -> Result<Option<String>, String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .replayed_create(client_token, fingerprint)
        .map(|found| found.map(|application| application.id.clone()))
        .map_err(|refusal| refusal.message)
}

/// The `(catalog, id)` the ARN index resolves `arn` to, read straight from the store.
fn indexed_by_arn(harness: &Harness, catalog: &str, arn: &str) -> Option<String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .resolve_application_id(catalog, arn)
}

/// Whether the ARNs of `catalog` are ARNs the modelled `resourceArn` member admits at all:
/// its pattern spells the catalog segment `([a-zA-Z]+)`, so a catalog named with digits,
/// punctuation or nothing has no ARN that member accepts.
fn arn_catalog_is_modelled(catalog: &str) -> bool {
    !catalog.is_empty() && catalog.chars().all(|segment| segment.is_ascii_alphabetic())
}

/// One candidate record for the write under test, built from an application this service
/// itself created, so every member but the identity is a real stored one.
fn candidate(stored: &BenefitApplication, catalog: &str, id: &str) -> BenefitApplication {
    BenefitApplication {
        id: id.to_string(),
        arn: application_arn(REGION, catalog, id),
        catalog: catalog.to_string(),
        ..stored.clone()
    }
}

/// Create one application over the wire and hand back the record the store kept for it,
/// which every row below builds its candidate from.
async fn stored_application(harness: &Harness, case: &str) -> BenefitApplication {
    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": SPENT_TOKEN,
                "BenefitIdentifier": benefit_id,
                "Name": "an application the store really holds",
                "Description": "the record every candidate below is built from",
                "BenefitApplicationDetails": { "CampaignName": "spring" },
            }),
        )
        .await;
    let id = text(case, &created, "Id");
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .application(CATALOG, &id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the create must have stored {id:?}"))
}

/// A write naming a catalog this store does not hold stores NOTHING: not the application,
/// not its ARN index entry, not its `ClientToken` — so the whole store is left exactly as
/// it was, and the token the write carried is neither a replay nor a mismatch afterwards.
#[tokio::test]
async fn a_write_naming_a_catalog_the_store_does_not_hold_stores_nothing() {
    let harness = Harness::new();
    let stored = stored_application(&harness, "the application every row starts from").await;

    for (index, row) in orphan_rows().iter().enumerate() {
        let case = row.case;
        assert!(
            !SEEDED_CATALOGS.contains(&row.catalog.as_str()),
            "case {case:?}: {:?} must name no catalog this service holds, got {SEEDED_CATALOGS:?}",
            row.catalog
        );

        let id = if row.reuses_stored_identity {
            stored.id.clone()
        } else {
            probe_id(index)
        };
        let application = candidate(&stored, &row.catalog, &id);
        let arn = application.arn.clone();
        let fingerprint = format!("a-request-that-stored-nothing-{index}");

        let before = store_snapshot(&harness);
        insert(&harness, &row.token, &fingerprint, application);
        let after = store_snapshot(&harness);
        assert_eq!(
            after, before,
            "case {case:?}: a write naming the unheld catalog {:?} must store nothing, expected \
             the store to stay {before}, got {after}",
            row.catalog
        );

        // The token index is what a create reads first, so what it answers after the
        // refused write is the whole point: a fresh token is still unspent, and a token a
        // create spent still answers for THAT create (a different request on it is the
        // mismatch it always was).
        let same = replayed(&harness, &row.token, &fingerprint);
        let other = replayed(&harness, &row.token, "a-different-request-entirely");
        if row.token_already_spent {
            assert_eq!(
                same,
                Err(IDEMPOTENT_PARAMETER_MISMATCH.to_string()),
                "case {case:?}: the token is spent on another request, so {fingerprint:?} must \
                 still be the mismatch it was, got {same:?}"
            );
        } else {
            assert_eq!(
                same,
                Ok(None),
                "case {case:?}: no create holds {:?}, so it must read as a new request, got \
                 {same:?}",
                row.token
            );
            assert_eq!(
                other,
                Ok(None),
                "case {case:?}: no create holds {:?}, so a differing request on it must not be \
                 refused as a mismatch, got {other:?}",
                row.token
            );
        }

        // The ARN of a record that was not stored names nothing — in the index, and over
        // the wire through the one operation that resolves a resource by ARN alone.
        assert_eq!(
            indexed_by_arn(&harness, &row.catalog, &arn),
            None,
            "case {case:?}: {arn:?} must resolve to no application"
        );
        let (status, out) = harness
            .call("ListTagsForResource", &json!({ "resourceArn": arn }))
            .await;
        // The modelled `resourceArn` pattern admits only an alphabetic catalog segment
        // (`^arn:[\w+=/,.@-]+:partnercentral:[\w+=/,.@-]*:[0-9]{12}:catalog/([a-zA-Z]+)/...`,
        // from `smithy_query(query="operation",operation="ListTagsForResource")`), so for a
        // row whose catalog name is not one the member refuses the ARN ahead of any lookup;
        // either way the ARN reaches no resource.
        let (expected_status, expected_code) = if arn_catalog_is_modelled(&row.catalog) {
            (404, "ResourceNotFoundException")
        } else {
            (400, "ValidationException")
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: {arn:?} names no resource, so the tag read must answer \
             {expected_status}, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            expected_code,
            "case {case:?}: expected {expected_code} for {arn:?}, got {out:?}"
        );
    }
}

/// A write naming a catalog this store DOES hold stores all three: the application in that
/// catalog, its ARN in the ARN index and its `ClientToken` in the token index — so the
/// record reads back over the wire by either identifier form, and the token replays to it.
#[tokio::test]
async fn a_write_naming_a_held_catalog_stores_the_record_and_both_indexes() {
    // (case, the `ClientToken` the write carries) — one row per catalog this store holds,
    // crossed with the token forms the modelled `^[!-~]{1,64}$` admits.
    let tokens: [&str; 2] = ["w", "7b1c0d9e-3a4f-4b5c-8d6e-2f1a0b9c8d7e"];

    let harness = Harness::new();
    let stored = stored_application(&harness, "the application every row starts from").await;

    for (index, catalog) in SEEDED_CATALOGS.iter().enumerate() {
        let case = format!("the held catalog {catalog:?}");
        let token = tokens[index % tokens.len()];
        let id = probe_id(1_000 + index);
        let application = candidate(&stored, catalog, &id);
        let arn = application.arn.clone();
        let fingerprint = format!("a-request-that-stored-{id}");

        let before = application_count(&harness);
        insert(&harness, token, &fingerprint, application);
        assert_eq!(
            application_count(&harness),
            before + 1,
            "case {case:?}: the write must store exactly one application"
        );

        // The ARN index names the record the write stored, and the token index replays to
        // it — the same request answers with the application, a different one is the
        // modelled idempotency mismatch.
        assert_eq!(
            indexed_by_arn(&harness, catalog, &arn),
            Some(id.clone()),
            "case {case:?}: {arn:?} must resolve to {id:?}"
        );
        assert_eq!(
            replayed(&harness, token, &fingerprint),
            Ok(Some(id.clone())),
            "case {case:?}: the token must replay to {id:?}"
        );
        assert_eq!(
            replayed(&harness, token, "a-different-request-entirely"),
            Err(IDEMPOTENT_PARAMETER_MISMATCH.to_string()),
            "case {case:?}: a different request carrying the spent token must be refused"
        );

        // The round trip: the stored record reads back over the wire by its id and by its
        // ARN, which is what the two indexes exist for.
        for identifier in [id.as_str(), arn.as_str()] {
            let out = harness.get_in(&case, catalog, identifier).await;
            assert_eq!(
                text(&case, &out, "Id"),
                id,
                "case {case:?}: reading {identifier:?} back must report {id:?}, got {out:?}"
            );
            assert_eq!(
                text(&case, &out, "Arn"),
                arn,
                "case {case:?}: reading {identifier:?} back must report {arn:?}, got {out:?}"
            );
            assert_eq!(
                text(&case, &out, "Catalog"),
                *catalog,
                "case {case:?}: reading {identifier:?} back must report the catalog it was \
                 stored in, got {out:?}"
            );
        }
    }
}

/// End to end, the damage the rule prevents: a `ClientToken` a write that stored nothing
/// carried is still a token no create holds, so a later `CreateBenefitApplication` reusing
/// it is served — once. The create that spends it answers 200, a retry of that create
/// replays it, and a DIFFERENT create carrying it is then the refusal the spent token
/// deserves.
#[tokio::test]
async fn a_token_a_refused_write_carried_is_still_spendable_by_a_create() {
    let harness = Harness::new();
    let case = "a token carried by a write that stored nothing";
    let stored = stored_application(&harness, case).await;

    let token = "a-token-no-create-holds-yet";
    let application = candidate(&stored, "Production", &probe_id(2_000));
    insert(&harness, token, "a-request-that-stored-nothing", application);

    let benefit_id = text(case, &harness.active_benefit().await, "Id");
    let before = application_count(&harness);
    let first_body = json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "BenefitIdentifier": benefit_id,
        "Name": "the create that spends the token",
    });

    // The create is a new request on that token, so it is served rather than refused for an
    // application nothing ever stored.
    let (status, created) = harness.call("CreateBenefitApplication", &first_body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must be served, got {status} {created:?}"
    );
    assert_eq!(
        application_count(&harness),
        before + 1,
        "case {case:?}: the create must store exactly one application"
    );

    // A retry of that same create is a replay of it: the same answer, and no second
    // application under the one token.
    let replay = harness
        .ok(case, "CreateBenefitApplication", &first_body)
        .await;
    assert_eq!(
        replay, created,
        "case {case:?}: the retry must answer the first create's output {created:?}, got {replay:?}"
    );
    assert_eq!(
        application_count(&harness),
        before + 1,
        "case {case:?}: the retry must store nothing"
    );

    // And now the token really is spent, so a different create carrying it is refused.
    let mut other_body = first_body.clone();
    other_body["Name"] = json!("a different create under the same token");
    let (status, refused) = harness.call("CreateBenefitApplication", &other_body).await;
    assert_eq!(
        status, 409,
        "case {case:?}: a different create on the spent token must be refused, got {status} \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {refused:?}"
    );
    assert_eq!(
        message_of(&refused).as_deref(),
        Some(IDEMPOTENT_PARAMETER_MISMATCH),
        "case {case:?}: expected the idempotency-mismatch message, got {refused:?}"
    );
    assert_eq!(
        application_count(&harness),
        before + 1,
        "case {case:?}: the refused create must store nothing"
    );
}
