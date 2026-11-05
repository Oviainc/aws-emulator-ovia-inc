// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone catalog listing lifecycle
//! (`CreateListingChangeSet`, `GetListing`, and the `listing` a published asset
//! reports through `GetAsset`).
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A PUBLISH change set records the named entity in its domain's catalog: the first
//!    one mints a listing at revision 1 and every later one records the NEXT revision of
//!    that same listing, and each recorded revision is a snapshot of the entity as it
//!    was when the change set ran (`GetListing` reports it back).
//! 2. Publishing is a step the service reports as in-progress: the listing is `CREATING`
//!    when the change set answers and `ACTIVE` once the clock makes the publish due.
//! 3. An UNPUBLISH change set takes the record out of the catalog — the listing becomes
//!    `INACTIVE`, keeping the revisions it had — and publishing again puts it back.
//! 4. The preconditions are refused with the operation's own modelled errors: a missing
//!    domain, an entity no store holds, or an entity revision the entity does not have
//!    with `ResourceNotFoundException`; an unsettled domain with `ConflictException`.
//! 5. A repeated `clientToken` applies ONE change set, and a read changes nothing a
//!    later caller sees.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// `ListingId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="ListingId").
fn is_listing_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `ListingName`'s modelled maximum length, 1..64 (smithy_query shape="ListingName") —
/// shorter than `AssetName`'s 1..256, so a longer entity name is reported clipped to it.
const LISTING_NAME_MAX_LEN: usize = 64;

/// The listing item's `forms` member, parsed back from the modelled `Forms` string.
fn item_forms(body: &Value) -> Option<Value> {
    let forms = body
        .get("item")?
        .get("assetListing")?
        .get("forms")?
        .as_str()?;
    Some(serde_json::from_str(forms).expect("the listing's forms are a JSON document"))
}

/// One row: an asset to create and publish. The rows differ along what the rule
/// generalises over — how many metadata forms the published entity carries, whether its
/// content is JSON, whether it has a description, how long its name is, and whether the
/// change set pins the entity's revision.
struct PublishCase {
    label: &'static str,
    /// The `CreateAsset` body, minus `owningProjectIdentifier` (filled by the test).
    asset: Value,
    /// The listing `name` the entity's name must be reported as.
    expect_name: String,
    /// The listing `description`, absent when the asset has none.
    expect_description: Option<String>,
    /// The listing item's `forms`, absent when the entity carries no form.
    expect_forms: Option<Value>,
    /// Whether the change set pins `entityRevision` to the asset's own revision.
    pin_entity_revision: bool,
}

fn publish_cases() -> Vec<PublishCase> {
    vec![
        PublishCase {
            label: "an asset with no form and no description",
            asset: json!({
                "name": "minimal-asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
            }),
            expect_name: "minimal-asset".to_string(),
            expect_description: None,
            expect_forms: None,
            pin_entity_revision: false,
        },
        PublishCase {
            label: "one form whose content is JSON",
            asset: json!({
                "name": "orders table",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "description": "the orders fact table",
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": "{\"columns\":[{\"columnName\":\"order_id\"}]}",
                }],
            }),
            expect_name: "orders table".to_string(),
            expect_description: Some("the orders fact table".to_string()),
            expect_forms: Some(json!({
                "RelationalTableForm": { "columns": [{ "columnName": "order_id" }] },
            })),
            pin_entity_revision: false,
        },
        PublishCase {
            label: "three forms: JSON content, no content, and content that is not JSON",
            asset: json!({
                "name": "customers.v2",
                "typeIdentifier": "CustomTable",
                "description": "d".repeat(2048),
                "glossaryTerms": ["term0000000000000001"],
                "formsInput": [
                    {
                        "formName": "RelationalTableForm",
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "content": "{\"columns\":[]}",
                    },
                    { "formName": "_private_notes" },
                    { "formName": "OwnershipForm", "content": "data-eng" },
                ],
            }),
            expect_name: "customers.v2".to_string(),
            expect_description: Some("d".repeat(2048)),
            expect_forms: Some(json!({
                "RelationalTableForm": { "columns": [] },
                "_private_notes": Value::Null,
                "OwnershipForm": "data-eng",
            })),
            pin_entity_revision: true,
        },
        PublishCase {
            label: "an entity name longer than ListingName's maximum",
            asset: json!({
                "name": "n".repeat(256),
                "typeIdentifier": "amazon.datazone.GlueTableAssetType",
                "formsInput": [],
            }),
            expect_name: "n".repeat(LISTING_NAME_MAX_LEN),
            expect_description: None,
            expect_forms: None,
            pin_entity_revision: true,
        },
    ]
}

#[tokio::test]
async fn publishing_an_entity_records_a_listing_get_listing_reports_back() {
    let dz = Emulator::new();
    let domain = dz.available_domain("listing-domain").await;
    let project = dz.project(&domain, "listing owner").await;
    let change_sets_uri = format!("/v2/domains/{domain}/listings/change-set");

    // (label, asset id, listing id) for the ACTIVE checks once the clock has made
    // every publish due.
    let mut published: Vec<(&'static str, String, String)> = Vec::new();
    for case in publish_cases() {
        let mut body = case.asset.clone();
        body["owningProjectIdentifier"] = json!(project);
        let asset = dz
            .call("POST", &format!("/v2/domains/{domain}/assets"), Some(body))
            .await;
        assert_eq!(
            asset.status, 201,
            "case {:?}: creating the asset: expected 201, got {} {:?}",
            case.label, asset.status, asset.body
        );
        let asset_id = asset.s("id");
        let description = asset.body.get("description").cloned();

        let mut change_set = json!({
            "entityIdentifier": asset_id,
            "entityType": "ASSET",
            "action": "PUBLISH",
        });
        if case.pin_entity_revision {
            change_set["entityRevision"] = json!(asset.s("revision"));
        }
        let published_answer = dz.call("POST", &change_sets_uri, Some(change_set)).await;
        assert_eq!(
            published_answer.status, 200,
            "case {:?}: publishing: expected 200, got {} {:?}",
            case.label, published_answer.status, published_answer.body
        );
        let listing_id = published_answer.s("listingId");
        assert!(
            is_listing_id(&listing_id),
            "case {:?}: expected a listingId matching ^[a-zA-Z0-9_-]{{1,36}}$, got {listing_id:?}",
            case.label
        );
        assert_eq!(
            published_answer.s("listingRevision"),
            "1",
            "case {:?}: a first publish records revision 1, got {:?}",
            case.label,
            published_answer.body
        );
        // Rule 2: the publish is in progress when the change set answers, so a caller
        // polling straight afterwards still sees that status.
        assert_eq!(
            published_answer.s("status"),
            "CREATING",
            "case {:?}: a publish that has just started, got {:?}",
            case.label,
            published_answer.body
        );
        let listing_uri = format!("/v2/domains/{domain}/listings/{listing_id}");
        let creating = dz.call("GET", &listing_uri, None).await;
        assert_eq!(
            creating.s("status"),
            "CREATING",
            "case {:?}: GetListing before the publish is due, got {:?}",
            case.label,
            creating.body
        );

        // Rule 1: the recorded revision is a snapshot of the published entity.
        assert_eq!(
            creating.s("listingRevision"),
            "1",
            "case {:?}: GetListing defaults to the latest revision, got {:?}",
            case.label,
            creating.body
        );
        assert_eq!(
            creating.s("id"),
            listing_id,
            "case {:?}: GetListing reports the listing asked for, got {:?}",
            case.label,
            creating.body
        );
        assert_eq!(
            creating.s("domainId"),
            domain,
            "case {:?}: the listing belongs to the domain it was published in, got {:?}",
            case.label,
            creating.body
        );
        assert_eq!(
            creating.s("name"),
            case.expect_name,
            "case {:?}: the listing's name, got {:?}",
            case.label,
            creating.body
        );
        assert_eq!(
            creating.body.get("description").and_then(Value::as_str),
            case.expect_description.as_deref(),
            "case {:?}: the listing's description, got {:?}",
            case.label,
            creating.body.get("description")
        );
        // A description the request supplied is reported by the entity and by the
        // listing alike, so the two cannot drift apart.
        if let Some(expected) = description.as_ref().and_then(Value::as_str) {
            assert_eq!(
                creating.body.get("description").and_then(Value::as_str),
                Some(expected),
                "case {:?}: the listing records the entity's description, got {:?}",
                case.label,
                creating.body.get("description")
            );
        }
        let item = creating
            .body
            .get("item")
            .and_then(|item| item.get("assetListing"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            item.get("assetId").and_then(Value::as_str),
            Some(asset_id.as_str()),
            "case {:?}: the item records the published asset, got {item:?}",
            case.label
        );
        assert_eq!(
            item.get("assetRevision").and_then(Value::as_str),
            Some(asset.s("revision").as_str()),
            "case {:?}: the item records the entity revision, got {item:?}",
            case.label
        );
        assert_eq!(
            item.get("assetType").and_then(Value::as_str),
            case.asset.get("typeIdentifier").and_then(Value::as_str),
            "case {:?}: the item records the entity's type, got {item:?}",
            case.label
        );
        assert_eq!(
            item.get("owningProjectId").and_then(Value::as_str),
            Some(project.as_str()),
            "case {:?}: the item records the owning project, got {item:?}",
            case.label
        );
        assert_eq!(
            item.get("createdAt").cloned(),
            asset.body.get("createdAt").cloned(),
            "case {:?}: the item records when the ENTITY was created, got {item:?}",
            case.label
        );
        assert_eq!(
            item_forms(&creating.body),
            case.expect_forms,
            "case {:?}: the item's forms, got {:?}",
            case.label,
            item.get("forms")
        );
        // A term's own record is a resource this emulator does not store, so the
        // detailed terms a listing would carry are absent rather than invented.
        assert!(
            item.get("glossaryTerms").is_none(),
            "case {:?}: expected no glossaryTerms on the item, got {item:?}",
            case.label
        );
        published.push((case.label, asset_id, listing_id));
    }

    // Rule 2: the publish the clock owed is due, so every listing is now in the
    // catalog — and the asset's own reader agrees about which listing it has.
    tokio::time::sleep(SETTLED).await;
    for (label, asset_id, listing_id) in &published {
        let active = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/listings/{listing_id}"),
                None,
            )
            .await;
        assert_eq!(
            active.s("status"),
            "ACTIVE",
            "case {label:?}: once the publish is due, got {:?}",
            active.body
        );
        let asset = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/assets/{asset_id}"),
                None,
            )
            .await;
        let listing = asset.body.get("listing").cloned().unwrap_or(Value::Null);
        assert_eq!(
            listing.get("listingId").and_then(Value::as_str),
            Some(listing_id.as_str()),
            "case {label:?}: GetAsset reports the asset's listing, got {listing:?}"
        );
        assert_eq!(
            listing.get("listingStatus").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {label:?}: GetAsset reports the listing's status, got {listing:?}"
        );
    }
}

#[tokio::test]
async fn republishing_records_the_next_revision_and_unpublishing_deactivates_the_listing() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-domain").await;
    let project = dz.project(&domain, "revision owner").await;
    let change_sets_uri = format!("/v2/domains/{domain}/listings/change-set");

    // The rule generalises over HOW MANY times an entity is published: each change set
    // records the next revision of the one listing that entity has.
    for publishes in [1_u32, 2, 5] {
        let asset = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/assets"),
                Some(json!({
                    "name": format!("asset-published-{publishes}-times"),
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                })),
            )
            .await;
        assert_eq!(
            asset.status, 201,
            "case {publishes} publishes: creating the asset, got {} {:?}",
            asset.status, asset.body
        );
        let asset_id = asset.s("id");
        let publish = json!({
            "entityIdentifier": asset_id,
            "entityType": "ASSET",
            "action": "PUBLISH",
        });

        let mut listing_id = String::new();
        for round in 1..=publishes {
            let answer = dz
                .call("POST", &change_sets_uri, Some(publish.clone()))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {publishes} publishes, round {round}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.s("listingRevision"),
                round.to_string(),
                "case {publishes} publishes, round {round}: the next revision, got {:?}",
                answer.body
            );
            if listing_id.is_empty() {
                listing_id = answer.s("listingId");
            }
            assert_eq!(
                answer.s("listingId"),
                listing_id,
                "case {publishes} publishes, round {round}: one entity has ONE listing, got {:?}",
                answer.body
            );
        }
        let listing_uri = format!("/v2/domains/{domain}/listings/{listing_id}");

        // Every recorded revision is still addressable, and the default is the newest.
        let latest = dz.call("GET", &listing_uri, None).await;
        assert_eq!(
            latest.s("listingRevision"),
            publishes.to_string(),
            "case {publishes} publishes: the default revision is the newest, got {:?}",
            latest.body
        );
        for round in 1..=publishes {
            let pinned = dz
                .call("GET", &format!("{listing_uri}?listingRevision={round}"), None)
                .await;
            assert_eq!(
                pinned.status, 200,
                "case {publishes} publishes: revision {round} is addressable, got {} {:?}",
                pinned.status, pinned.body
            );
            assert_eq!(
                pinned.s("listingRevision"),
                round.to_string(),
                "case {publishes} publishes: asked for revision {round}, got {:?}",
                pinned.body
            );
        }
        // A revision the listing does not have names no stored resource.
        for absent in [(publishes + 1).to_string(), "01".to_string(), "x".to_string()] {
            let missing = dz
                .call(
                    "GET",
                    &format!("{listing_uri}?listingRevision={absent}"),
                    None,
                )
                .await;
            assert_eq!(
                missing.status, 404,
                "case {publishes} publishes: revision {absent:?} is absent, got {} {:?}",
                missing.status, missing.body
            );
            assert_eq!(
                missing.error_code, "ResourceNotFoundException",
                "case {publishes} publishes: revision {absent:?} is absent, got {:?}",
                missing.body
            );
        }

        // Rule 3: unpublishing takes the record out of the catalog, keeping every
        // revision it recorded.
        let unpublish = json!({
            "entityIdentifier": asset_id,
            "entityType": "ASSET",
            "action": "UNPUBLISH",
        });
        let unpublished = dz
            .call("POST", &change_sets_uri, Some(unpublish.clone()))
            .await;
        assert_eq!(
            unpublished.status, 200,
            "case {publishes} publishes: unpublishing, expected 200, got {} {:?}",
            unpublished.status, unpublished.body
        );
        assert_eq!(
            unpublished.s("status"),
            "INACTIVE",
            "case {publishes} publishes: an unpublished listing, got {:?}",
            unpublished.body
        );
        assert_eq!(
            unpublished.s("listingId"),
            listing_id,
            "case {publishes} publishes: unpublishing names the same listing, got {:?}",
            unpublished.body
        );
        assert_eq!(
            unpublished.s("listingRevision"),
            publishes.to_string(),
            "case {publishes} publishes: an unpublish records no revision, got {:?}",
            unpublished.body
        );
        let inactive = dz.call("GET", &listing_uri, None).await;
        assert_eq!(
            inactive.s("status"),
            "INACTIVE",
            "case {publishes} publishes: GetListing after the unpublish, got {:?}",
            inactive.body
        );
        assert_eq!(
            inactive.s("listingRevision"),
            publishes.to_string(),
            "case {publishes} publishes: the revisions survive the unpublish, got {:?}",
            inactive.body
        );
        // Unpublishing again has nothing left to apply: the stored listing is reported
        // as it stands.
        let again = dz.call("POST", &change_sets_uri, Some(unpublish)).await;
        assert_eq!(
            again.s("status"),
            "INACTIVE",
            "case {publishes} publishes: a repeated unpublish, got {:?}",
            again.body
        );
        assert_eq!(
            again.s("listingRevision"),
            publishes.to_string(),
            "case {publishes} publishes: a repeated unpublish records nothing, got {:?}",
            again.body
        );

        // And publishing it again puts it back in the catalog, through the same
        // in-progress step, at the next revision.
        let back = dz.call("POST", &change_sets_uri, Some(publish)).await;
        assert_eq!(
            back.s("listingRevision"),
            (publishes + 1).to_string(),
            "case {publishes} publishes: a re-publish records the next revision, got {:?}",
            back.body
        );
        assert_eq!(
            back.s("status"),
            "CREATING",
            "case {publishes} publishes: a re-publish starts the publish again, got {:?}",
            back.body
        );
        tokio::time::sleep(SETTLED).await;
        let active = dz.call("GET", &listing_uri, None).await;
        assert_eq!(
            active.s("status"),
            "ACTIVE",
            "case {publishes} publishes: once the re-publish is due, got {:?}",
            active.body
        );
    }
}

#[tokio::test]
async fn a_change_set_needs_a_settled_domain_and_a_stored_entity() {
    let dz = Emulator::new();
    let domain = dz.available_domain("precondition-listing-domain").await;
    let project = dz.project(&domain, "owner").await;
    let asset = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "publishable",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(asset.status, 201, "creating the asset: got {:?}", asset.body);
    let asset_id = asset.s("id");

    // A domain that is still CREATING cannot take a change set yet.
    let creating = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "listing-still-creating" })),
        )
        .await;
    assert_eq!(
        creating.status, 201,
        "creating the unsettled domain: got {:?}",
        creating.body
    );
    let creating_domain = creating.s("id");

    // (label, domain, body, expected status, expected error code)
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the call
            // before any lookup.
            "a domain nothing created",
            "dzd_doesnotexist".to_string(),
            json!({ "entityIdentifier": asset_id, "entityType": "ASSET", "action": "PUBLISH" }),
            403,
            "AccessDeniedException",
        ),
        (
            "a domain that is still CREATING",
            creating_domain,
            json!({ "entityIdentifier": asset_id, "entityType": "ASSET", "action": "PUBLISH" }),
            409,
            "ConflictException",
        ),
        (
            "an asset id nothing created",
            domain.clone(),
            json!({ "entityIdentifier": "nothing", "entityType": "ASSET", "action": "PUBLISH" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a data product, which no store holds",
            domain.clone(),
            json!({
                "entityIdentifier": asset_id,
                "entityType": "DATA_PRODUCT",
                "action": "PUBLISH",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an entity revision the entity does not have",
            domain.clone(),
            json!({
                "entityIdentifier": asset_id,
                "entityType": "ASSET",
                "action": "PUBLISH",
                "entityRevision": "7",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unpublishing an entity that was never published",
            domain.clone(),
            json!({ "entityIdentifier": asset_id, "entityType": "ASSET", "action": "UNPUBLISH" }),
            404,
            "ResourceNotFoundException",
        ),
    ];
    for (label, change_set_domain, body, status, code) in rows {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{change_set_domain}/listings/change-set"),
                Some(body),
            )
            .await;
        assert_eq!(
            answer.status, status,
            "case {label:?}: expected {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, code,
            "case {label:?}: expected {code}, got {:?}",
            answer.body
        );
    }

    // A listing id nothing published is a 404, not an empty success.
    for absent in ["nothing", "0"] {
        let missing = dz
            .call("GET", &format!("/v2/domains/{domain}/listings/{absent}"), None)
            .await;
        assert_eq!(
            missing.status, 404,
            "case {absent:?}: expected 404, got {} {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.error_code, "ResourceNotFoundException",
            "case {absent:?}: expected ResourceNotFoundException, got {:?}",
            missing.body
        );
    }
    // Nothing above published anything, so the asset still has no listing: a refused
    // change set persisted nothing.
    let unpublished_asset = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/assets/{asset_id}"),
            None,
        )
        .await;
    assert!(
        unpublished_asset.body.get("listing").is_none(),
        "expected no listing on an asset no change set published, got {:?}",
        unpublished_asset.body
    );
}

#[tokio::test]
async fn a_repeated_client_token_applies_one_change_set() {
    let dz = Emulator::new();
    let domain = dz.available_domain("listing-idempotency-domain").await;
    let project = dz.project(&domain, "owner").await;
    let change_sets_uri = format!("/v2/domains/{domain}/listings/change-set");

    // `ClientToken` is 1..128 with pattern ^[\x21-\x7E]+$ (smithy_query
    // shape="ClientToken"), so the rows span that class and both bounds of its length.
    for token in ["token-1", "A!~", &"t".repeat(128)] {
        let asset = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/assets"),
                Some(json!({
                    "name": format!("asset-for-{}", &token[..3]),
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                })),
            )
            .await;
        assert_eq!(
            asset.status, 201,
            "case {token:?}: creating the asset, got {:?}",
            asset.body
        );
        let publish = json!({
            "entityIdentifier": asset.s("id"),
            "entityType": "ASSET",
            "action": "PUBLISH",
            "clientToken": token,
        });

        let first = dz
            .call("POST", &change_sets_uri, Some(publish.clone()))
            .await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: publishing, got {} {:?}",
            first.status, first.body
        );
        let replay = dz.call("POST", &change_sets_uri, Some(publish)).await;
        assert_eq!(
            replay.s("listingId"),
            first.s("listingId"),
            "case {token:?}: a replay answers with the same listing, got {:?}",
            replay.body
        );
        assert_eq!(
            replay.s("listingRevision"),
            first.s("listingRevision"),
            "case {token:?}: a replay records no second revision, got {:?}",
            replay.body
        );
        // The store itself is the evidence: the listing is still at the revision the
        // first change set recorded.
        let listing = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/listings/{}", first.s("listingId")),
                None,
            )
            .await;
        assert_eq!(
            listing.s("listingRevision"),
            first.s("listingRevision"),
            "case {token:?}: the stored listing after the replay, got {:?}",
            listing.body
        );

        // The same token against a DIFFERENT change set is not a replay of this one.
        let unpublish = json!({
            "entityIdentifier": asset.s("id"),
            "entityType": "ASSET",
            "action": "UNPUBLISH",
            "clientToken": token,
        });
        let unpublished = dz
            .call("POST", &change_sets_uri, Some(unpublish.clone()))
            .await;
        assert_eq!(
            unpublished.s("status"),
            "INACTIVE",
            "case {token:?}: the token's unpublish applies, got {:?}",
            unpublished.body
        );
        let unpublish_replay = dz.call("POST", &change_sets_uri, Some(unpublish)).await;
        assert_eq!(
            unpublish_replay.s("status"),
            "INACTIVE",
            "case {token:?}: a replayed unpublish, got {:?}",
            unpublish_replay.body
        );
        assert_eq!(
            unpublish_replay.s("listingRevision"),
            first.s("listingRevision"),
            "case {token:?}: a replayed unpublish records nothing, got {:?}",
            unpublish_replay.body
        );
    }
}
