// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreateAssetRevision`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateAssetRevision` records the NEXT revision number of a stored asset from the
//!    metadata the request carries (`name`, `description`, `formsInput`, `glossaryTerms`,
//!    `predictionConfiguration`), while the asset's identity — `id`, `domainId`,
//!    `owningProjectId`, `typeIdentifier`, `externalIdentifier` — and its
//!    `firstRevisionCreatedBy`/`firstRevisionCreatedAt` stay exactly as they were.
//! 2. The revisions already recorded are left alone: `GetAsset` reads each earlier
//!    revision back by number, as it was recorded, and a `GetAsset` that names no
//!    revision reports the newest.
//! 3. `typeRevision` follows the rule `CreateAsset` applies, except that a request that
//!    pins none keeps the revision the asset is already bound to.
//! 4. A repeated `clientToken` answers with the revision it recorded instead of
//!    recording a second one.
//! 5. The preconditions are refused with the operation's own modelled errors: an absent
//!    domain or asset with `ResourceNotFoundException`, a domain that is no longer
//!    available with `ConflictException`, a `formsInput` naming one form twice or a
//!    `typeRevision` the named asset type does not have with `ValidationException`.
//! 6. Revising an asset neither publishes it nor withdraws it: a published asset's
//!    `listing` is reported unchanged by the revision.

use crate::asset_lifecycle::{expected_forms, reported_forms};
use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};

/// The principal the harness's SigV4 scope names, which is what every `createdBy` this
/// service writes must be.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY";

/// A domain, a project in it and one asset of a service-defined type: the precondition
/// every revision case needs. Answers `(domain_id, project_id, asset)`.
async fn asset_to_revise(dz: &Emulator, domain_name: &str, body: Value) -> (String, String, Answer) {
    let domain = dz.available_domain(domain_name).await;
    let project = dz.project(&domain, "revision owner").await;
    let mut body = body;
    body["owningProjectIdentifier"] = json!(project);
    let created = dz
        .call("POST", &format!("/v2/domains/{domain}/assets"), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "creating the asset to revise: expected 201, got {} {:?}",
        created.status, created.body
    );
    (domain, project, created)
}

/// One `CreateAssetRevision` request. The rows differ along what the rule generalises
/// over: which optional members the revision carries, how many forms it attaches (the
/// modelled `FormInputList` is 0..10), how many glossary terms, and whether it pins a
/// `typeRevision`.
struct RevisionCase {
    label: &'static str,
    body: Value,
}

fn revision_cases() -> Vec<RevisionCase> {
    vec![
        RevisionCase {
            label: "only the required name, dropping every optional member",
            body: json!({ "name": "orders" }),
        },
        RevisionCase {
            label: "a description and two forms",
            body: json!({
                "name": "orders table",
                "description": "the orders fact table, re-documented",
                "formsInput": [
                    {
                        "formName": "RelationalTableForm",
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "content": "{\"columns\":[{\"columnName\":\"order_id\",\"dataType\":\"bigint\"}]}",
                    },
                    { "formName": "_private_notes" },
                ],
            }),
        },
        RevisionCase {
            label: "glossary terms, a prediction configuration and an empty form list",
            body: json!({
                "name": "orders.v3",
                "glossaryTerms": ["term0000000000000001", "term0000000000000002"],
                "predictionConfiguration": { "businessNameGeneration": { "enabled": true } },
                "formsInput": [],
            }),
        },
        RevisionCase {
            label: "a pinned type revision on a type no store holds, and a long description",
            body: json!({
                "name": "a".repeat(256),
                "description": "d".repeat(2048),
                "typeRevision": "4",
                "formsInput": [
                    { "formName": "OwnershipForm", "typeIdentifier": "CustomOwnership", "typeRevision": "2", "content": "{\"owner\":\"data-eng\"}" },
                    { "formName": "SecondForm", "content": "not json" },
                    { "formName": "ThirdForm", "typeIdentifier": "amazon.datazone.RelationalTableFormType" },
                ],
            }),
        },
        RevisionCase {
            label: "a single glossary term and a prediction configuration that sets no flag",
            body: json!({
                "name": "orders.v5",
                "glossaryTerms": ["term0000000000000003"],
                "predictionConfiguration": { "businessNameGeneration": {} },
            }),
        },
    ]
}

/// The members of a revision response that must match the request that recorded it.
///
/// `predictionConfiguration` is NOT among them: `GetAssetOutput` has no such member
/// (smithy_query operation="GetAsset"), so this is the set both the revision's own
/// response and a later `GetAsset` report, and the revision response is checked for that
/// member separately.
fn assert_revision_matches_request(label: &str, request: &Value, answer: &Answer) {
    assert_eq!(
        answer.s("name"),
        request.get("name").and_then(Value::as_str).unwrap_or("<absent>"),
        "case {label:?}: the revision must report the name the request set"
    );
    assert_eq!(
        answer.body.get("description").and_then(Value::as_str),
        request.get("description").and_then(Value::as_str),
        "case {label:?}: the revision must report the description the request set, got {:?}",
        answer.body.get("description")
    );
    assert_eq!(
        reported_forms(&answer.body),
        expected_forms(request),
        "case {label:?}: the revision must report the forms the request listed, got {:?}",
        answer.body.get("formsOutput")
    );
    assert_eq!(
        answer.body.get("glossaryTerms"),
        request.get("glossaryTerms"),
        "case {label:?}: the revision must report the glossary terms the request listed, got {:?}",
        answer.body.get("glossaryTerms")
    );
}

#[tokio::test]
async fn each_revision_records_the_next_number_and_leaves_the_earlier_ones_readable() {
    let dz = Emulator::new();
    let (domain, project, created) = asset_to_revise(
        &dz,
        "asset-revision-domain",
        json!({
            "name": "orders",
            "typeIdentifier": "amazon.datazone.RelationalTable",
            "description": "the first revision",
            "externalIdentifier": "arn:aws:glue:us-west-2:123456789012:table/sales/orders",
            "formsInput": [{
                "formName": "RelationalTableForm",
                "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                "content": "{\"columns\":[]}",
            }],
        }),
    )
    .await;
    let id = created.s("id");
    let asset_uri = format!("/v2/domains/{domain}/assets/{id}");
    let revisions_uri = format!("{asset_uri}/revisions");
    // What revision one reported, which no later revision may change.
    let first_created_at = created.body.get("createdAt").cloned();
    let first_created_by = created.s("createdBy");
    // Every revision recorded so far: its number and the request that recorded it.
    let mut recorded: Vec<(&'static str, String, Value)> =
        vec![("the created asset", "1".to_string(), created.body.clone())];

    for (index, case) in revision_cases().into_iter().enumerate() {
        let expected_number = (index + 2).to_string();
        let answer = dz
            .call("POST", &revisions_uri, Some(case.body.clone()))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.label, answer.status, answer.body
        );
        // The revision number is the next one: revisions are numbered from one and
        // never go backwards.
        assert_eq!(
            answer.s("revision"),
            expected_number,
            "case {:?}: expected revision {expected_number}, got {:?}",
            case.label,
            answer.body
        );
        // The asset's identity is untouched by a revision of it.
        assert_eq!(
            (
                answer.s("id"),
                answer.s("domainId"),
                answer.s("owningProjectId"),
                answer.s("typeIdentifier"),
                answer.s("externalIdentifier"),
            ),
            (
                id.clone(),
                domain.clone(),
                project.clone(),
                "amazon.datazone.RelationalTable".to_string(),
                "arn:aws:glue:us-west-2:123456789012:table/sales/orders".to_string(),
            ),
            "case {:?}: a revision must not change the asset's identity, got {:?}",
            case.label,
            answer.body
        );
        // The revision was made by the CALLER, now; the FIRST revision still points at
        // the create.
        assert_eq!(
            answer.s("createdBy"),
            CALLER,
            "case {:?}: createdBy must name the calling principal",
            case.label
        );
        assert!(
            answer.body.get("createdAt").is_some(),
            "case {:?}: createdAt must be reported, got {:?}",
            case.label,
            answer.body
        );
        assert_eq!(
            (
                answer.s("firstRevisionCreatedBy"),
                answer.body.get("firstRevisionCreatedAt").cloned()
            ),
            (first_created_by.clone(), first_created_at.clone()),
            "case {:?}: the first revision is the creation itself, got {:?}",
            case.label,
            answer.body
        );
        assert_revision_matches_request(case.label, &case.body, &answer);
        // The prediction configuration is the one the request set, mirrored member for
        // member, and none is invented for a request that set none.
        assert_eq!(
            answer.body.get("predictionConfiguration"),
            case.body.get("predictionConfiguration"),
            "case {:?}: unexpected predictionConfiguration, got {:?}",
            case.label,
            answer.body.get("predictionConfiguration")
        );
        // A revision does not publish the asset, so an unpublished one still has no
        // catalog record.
        assert_eq!(
            answer.body.get("listing"),
            None,
            "case {:?}: an unpublished asset has no listing, got {:?}",
            case.label,
            answer.body.get("listing")
        );

        // The asset now reads back AT the new revision when no revision is named.
        let latest = dz.call("GET", &asset_uri, None).await;
        assert_eq!(
            (latest.status, latest.s("revision")),
            (200, expected_number.clone()),
            "case {:?}: GetAsset must report the newest revision, got {:?}",
            case.label,
            latest.body
        );
        assert_revision_matches_request(case.label, &case.body, &latest);

        recorded.push((case.label, expected_number, answer.body.clone()));
        // Every revision recorded so far still reads back exactly as it was recorded:
        // a later revision rewrites none of them.
        for (earlier_label, number, body) in &recorded {
            let read = dz
                .call("GET", &format!("{asset_uri}?revision={number}"), None)
                .await;
            assert_eq!(
                read.status, 200,
                "case {earlier_label:?}: revision {number} must still be readable, got {} {:?}",
                read.status, read.body
            );
            for member in [
                "id",
                "revision",
                "name",
                "description",
                "createdAt",
                "createdBy",
                "firstRevisionCreatedAt",
                "firstRevisionCreatedBy",
                "typeIdentifier",
                "typeRevision",
                "externalIdentifier",
                "owningProjectId",
                "domainId",
                "glossaryTerms",
                "formsOutput",
            ] {
                assert_eq!(
                    read.body.get(member),
                    body.get(member),
                    "case {earlier_label:?}: revision {number} must still report the {member} it \
                     was recorded with, got {:?}",
                    read.body.get(member)
                );
            }
        }
    }

    // One past the newest names no revision, so it is a 404 rather than an empty answer.
    let past_the_end = recorded.len() + 1;
    let absent = dz
        .call("GET", &format!("{asset_uri}?revision={past_the_end}"), None)
        .await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a revision the asset does not have must be 404, got {:?}",
        absent.body
    );
}

#[tokio::test]
async fn a_revision_resolves_its_type_revision_against_the_stored_asset_type() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-type-domain").await;
    let project = dz.project(&domain, "owner").await;

    // A custom asset type, created through the operation that owns that store.
    let asset_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/asset-types"),
            Some(json!({
                "name": "SalesTable",
                "owningProjectIdentifier": project,
                "formsInput": {
                    "SalesForm": {
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "typeRevision": "1",
                    },
                },
            })),
        )
        .await;
    assert_eq!(
        asset_type.status, 201,
        "creating the asset type: expected 201, got {} {:?}",
        asset_type.status, asset_type.body
    );
    let stored_revision = asset_type.s("revision");

    // Each row: the type the asset names, the `typeRevision` the asset was created
    // with, the one its revision pins, and either the `typeRevision` the revision must
    // report or the refusal the pin earns.
    enum Expect {
        TypeRevision(String),
        Refused(u16, &'static str),
    }
    let rows = vec![
        (
            "a stored type, revised without pinning: the asset keeps the revision it is bound to",
            "SalesTable",
            None,
            None,
            Expect::TypeRevision(stored_revision.clone()),
        ),
        (
            "a stored type, pinning the revision it is at",
            "SalesTable",
            None,
            Some(stored_revision.clone()),
            Expect::TypeRevision(stored_revision.clone()),
        ),
        (
            "a stored type, pinning a revision it does not have",
            "SalesTable",
            None,
            Some("9".to_string()),
            Expect::Refused(400, "ValidationException"),
        ),
        (
            "a service-defined type, revised without pinning: the created asset's pin stands",
            "amazon.datazone.RelationalTable",
            Some("3".to_string()),
            None,
            Expect::TypeRevision("3".to_string()),
        ),
        (
            "a service-defined type, pinning a revision no store can check",
            "amazon.datazone.RelationalTable",
            Some("3".to_string()),
            Some("8".to_string()),
            Expect::TypeRevision("8".to_string()),
        ),
    ];

    for (label, type_identifier, created_pin, revision_pin, expect) in rows {
        let mut create = json!({
            "name": format!("asset for {label}"),
            "typeIdentifier": type_identifier,
            "owningProjectIdentifier": project,
        });
        if let Some(pinned) = &created_pin {
            create["typeRevision"] = json!(pinned);
        }
        let created = dz
            .call("POST", &format!("/v2/domains/{domain}/assets"), Some(create))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: creating the asset: expected 201, got {} {:?}",
            created.status, created.body
        );
        let mut body = json!({ "name": "revised" });
        if let Some(pinned) = &revision_pin {
            body["typeRevision"] = json!(pinned);
        }
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/assets/{}/revisions", created.s("id")),
                Some(body),
            )
            .await;
        let expected = match expect {
            Expect::Refused(status, code) => {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (status, code),
                    "case {label:?}: expected {status} {code}, got {} {:?}",
                    answer.status,
                    answer.body
                );
                continue;
            }
            Expect::TypeRevision(revision) => revision,
        };
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.s("typeRevision"),
            expected,
            "case {label:?}: unexpected typeRevision, got {:?}",
            answer.body
        );
        // And the stored asset reports the same type revision on the way back out.
        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/assets/{}", created.s("id")),
                None,
            )
            .await;
        assert_eq!(
            (got.s("revision"), got.s("typeRevision")),
            ("2".to_string(), expected),
            "case {label:?}: GetAsset must report the revision this call recorded, got {:?}",
            got.body
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_answers_with_the_revision_it_recorded() {
    let dz = Emulator::new();
    let (domain, _project, created) = asset_to_revise(
        &dz,
        "revision-token-domain",
        json!({ "name": "orders", "typeIdentifier": "amazon.datazone.RelationalTable" }),
    )
    .await;
    let id = created.s("id");
    let asset_uri = format!("/v2/domains/{domain}/assets/{id}");
    let revisions_uri = format!("{asset_uri}/revisions");

    // Each row: the token, the body sent with it, and the revision number the answer
    // must carry. A token seen before answers with the revision it recorded — even when
    // the rest of the body differs — so a retry never records a second revision; a
    // fresh token records the next one.
    let rows = vec![
        ("tok-one", json!({ "name": "second" }), "2"),
        ("tok-one", json!({ "name": "second" }), "2"),
        (
            "tok-one",
            json!({ "name": "a different name under the same token" }),
            "2",
        ),
        ("tok-two", json!({ "name": "third" }), "3"),
        ("tok-two", json!({ "name": "third" }), "3"),
        ("tok-three", json!({ "name": "fourth" }), "4"),
    ];
    for (token, body, expected) in rows {
        let mut body = body;
        body["clientToken"] = json!(token);
        let answer = dz.call("POST", &revisions_uri, Some(body.clone())).await;
        assert_eq!(
            answer.status, 200,
            "case ({token:?}, {body}): expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.s("revision"),
            expected,
            "case ({token:?}, {body}): expected revision {expected}, got {:?}",
            answer.body
        );
    }
    // Three tokens recorded three revisions on top of the created one, and no more.
    let latest = dz.call("GET", &asset_uri, None).await;
    assert_eq!(
        latest.s("revision"),
        "4",
        "the asset must be at revision 4, got {:?}",
        latest.body
    );
    let absent = dz
        .call("GET", &format!("{asset_uri}?revision=5"), None)
        .await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a replayed token must not have recorded a fifth revision, got {:?}",
        absent.body
    );
    // The name the replayed token's revision carries is the one the FIRST request with
    // that token set, not the one the replay asked for.
    let second = dz
        .call("GET", &format!("{asset_uri}?revision=2"), None)
        .await;
    assert_eq!(
        second.s("name"),
        "second",
        "the replay must not have rewritten revision 2, got {:?}",
        second.body
    );
}

#[tokio::test]
async fn the_preconditions_a_revision_needs_are_refused_with_the_modelled_errors() {
    let dz = Emulator::new();
    let (domain, _project, created) = asset_to_revise(
        &dz,
        "revision-refusal-domain",
        json!({ "name": "orders", "typeIdentifier": "amazon.datazone.RelationalTable" }),
    )
    .await;
    let id = created.s("id");

    // Each row: the domain and asset the call addresses, the body, and the refusal.
    let rows = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // call before any lookup.
            "a domain nothing created",
            "dzd_0000000000".to_string(),
            id.clone(),
            json!({ "name": "revised" }),
            403,
            "AccessDeniedException",
        ),
        (
            "an asset the domain does not hold",
            domain.clone(),
            "nosuchasset".to_string(),
            json!({ "name": "revised" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a formsInput naming one form twice",
            domain.clone(),
            id.clone(),
            json!({
                "name": "revised",
                "formsInput": [
                    { "formName": "Twice", "content": "{}" },
                    { "formName": "Twice", "content": "{\"other\":true}" },
                ],
            }),
            400,
            "ValidationException",
        ),
    ];
    for (label, domain_id, asset_id, body, status, code) in rows {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain_id}/assets/{asset_id}/revisions"),
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

    // A domain on its way out: deleted, and revised on the very next call, before the
    // deletion the clock owes has settled, so the domain is still stored and DELETING.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting {domain:?}: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let too_late = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets/{id}/revisions"),
            Some(json!({ "name": "too late" })),
        )
        .await;
    assert_eq!(
        (too_late.status, too_late.error_code.as_str()),
        (409, "ConflictException"),
        "case \"a domain that is being deleted\": expected 409 ConflictException, got {} {:?}",
        too_late.status,
        too_late.body
    );
}

#[tokio::test]
async fn revising_a_published_asset_reports_the_listing_it_already_has() {
    let dz = Emulator::new();
    let (domain, _project, created) = asset_to_revise(
        &dz,
        "revision-listing-domain",
        json!({ "name": "orders", "typeIdentifier": "amazon.datazone.RelationalTable" }),
    )
    .await;
    let id = created.s("id");

    let published = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/listings/change-set"),
            Some(json!({
                "entityIdentifier": id,
                "entityType": "ASSET",
                "action": "PUBLISH",
            })),
        )
        .await;
    assert_eq!(
        published.status, 200,
        "publishing the asset: expected 200, got {} {:?}",
        published.status, published.body
    );
    let listing_id = published.s("listingId");

    let revised = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets/{id}/revisions"),
            Some(json!({ "name": "orders, revised" })),
        )
        .await;
    assert_eq!(
        revised.status, 200,
        "revising the published asset: expected 200, got {} {:?}",
        revised.status, revised.body
    );
    // The catalog record the asset already had, reported as the asset's own reader
    // reports it: revising neither publishes nor withdraws.
    let got = dz
        .call("GET", &format!("/v2/domains/{domain}/assets/{id}"), None)
        .await;
    assert_eq!(
        revised.body.get("listing"),
        got.body.get("listing"),
        "the revision and GetAsset must report one listing, got {:?} and {:?}",
        revised.body.get("listing"),
        got.body.get("listing")
    );
    assert_eq!(
        revised
            .body
            .get("listing")
            .and_then(|listing| listing.get("listingId"))
            .and_then(Value::as_str),
        Some(listing_id.as_str()),
        "the revision must report the listing the change set created, got {:?}",
        revised.body.get("listing")
    );
}
