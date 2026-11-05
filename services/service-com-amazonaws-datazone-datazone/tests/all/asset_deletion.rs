// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteAsset`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the responses as evidence, so a handler that
//! answered from a hardcoded value instead of from the store fails here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. Deleting a stored asset nothing references removes the whole record: the call
//!    answers 204 with an empty body, every recorded revision of it stops being
//!    readable, and a second delete of the same identifier is a miss. The assets beside
//!    it in the same domain are untouched.
//! 2. "Asset must not be referenced in any existing asset filters"
//!    (`smithy_query(query="doc",shape="DeleteAsset")`): an asset a stored asset filter
//!    is a view of is refused with `ConflictException`, naming the asset and the filter
//!    that prevented the delete, and the refusal stores nothing.
//! 3. A delete is scoped to the domain in the request path and addresses an asset by the
//!    identifier that reads it, so an id stored in another domain, or in no domain, is a
//!    `ResourceNotFoundException` naming the resource KIND and never the caller's
//!    identifier — and an asset in a domain that is being deleted is already gone.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// `AssetIdentifier`'s modelled maximum length, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="AssetIdentifier").
const IDENTIFIER_MAX: usize = 36;

/// The `ResourceNotFoundException` message the service answers for a catalog asset it
/// cannot find: a fixed sentence naming the resource KIND rather than the caller's
/// identifier (`crate::state::ASSET_NOT_FOUND_MESSAGE`).
const ASSET_NOT_FOUND: &str = "The given Asset doesn't exist. Try creating Asset before accessing it.";

/// The same, for a domain the service cannot find.
const DOMAIN_NOT_FOUND: &str = "Domain not found";

/// One object as a deletion refusal names it: `type <tag>#<domainId>#<identifier>`.
/// SOURCE: the recorded `ConflictException` wording, which names an asset-domain object
/// `type asset#<domainId>#<identifier>`.
fn asset_object(domain: &str, identifier: &str) -> String {
    format!("type asset#{domain}#{identifier}")
}

/// The `ConflictException` message a delete a stored dependency prevents is refused
/// with: recorded wording, built from the objects the call found, ending in a period and
/// a space.
fn deletion_prevented_message(object: &str, blocker: &str) -> String {
    format!("The following objects could not be deleted: {object} deletion prevented by {blocker}. ")
}

/// One asset to delete when nothing references it. The rows differ along what the rule
/// generalises over: which optional members the asset carries, how many metadata forms
/// it attaches, whether it has glossary terms or an external identifier, and how many
/// revisions are recorded under it.
struct DeletableCase {
    label: &'static str,
    /// The `CreateAsset` body, minus `owningProjectIdentifier`, which the test fills in.
    body: Value,
    /// Further `CreateAssetRevision` bodies recorded on the asset before it is deleted.
    revisions: Vec<Value>,
}

fn deletable_cases() -> Vec<DeletableCase> {
    vec![
        DeletableCase {
            label: "required members only, one revision",
            body: json!({
                "name": "minimal-asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
            }),
            revisions: vec![],
        },
        DeletableCase {
            label: "a description, one form and an external identifier",
            body: json!({
                "name": "orders table",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "description": "the orders fact table",
                "externalIdentifier": "arn:aws:glue:us-west-2:123456789012:table/sales/orders",
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": "{\"columns\":[{\"columnName\":\"order_id\",\"dataType\":\"bigint\"}]}",
                }],
            }),
            revisions: vec![],
        },
        DeletableCase {
            label: "glossary terms, three forms and three recorded revisions",
            body: json!({
                "name": "customers.v2",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "glossaryTerms": ["term0000000000000001", "term0000000000000002"],
                "formsInput": [
                    {
                        "formName": "RelationalTableForm",
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "content": "{\"columns\":[]}",
                    },
                    { "formName": "_private_notes", "content": "no content type" },
                    {
                        "formName": "OwnershipForm",
                        "typeIdentifier": "CustomOwnership",
                        "content": "{\"owner\":\"data-eng\"}",
                    },
                ],
            }),
            revisions: vec![
                json!({ "name": "customers.v3" }),
                json!({
                    "name": "customers.v4",
                    "description": "re-documented",
                    "formsInput": [],
                }),
            ],
        },
        DeletableCase {
            label: "an empty form list and a name at the modelled maximum",
            body: json!({
                "name": "a".repeat(256),
                "typeIdentifier": "amazon.datazone.GlueTableAssetType",
                "formsInput": [],
            }),
            revisions: vec![],
        },
    ]
}

#[tokio::test]
async fn deleting_an_unreferenced_asset_takes_every_revision_of_it_out_of_the_store() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-delete-domain").await;
    let project = dz.project(&domain, "delete owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    // An asset nothing below touches: every delete must leave it exactly where it was,
    // so a handler that cleared the whole map instead of one entry fails here.
    let bystander = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "bystander",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        bystander.status, 201,
        "creating the bystander asset: expected 201, got {} {:?}",
        bystander.status, bystander.body
    );
    let bystander_id = bystander.s("id");

    for case in deletable_cases() {
        let mut body = case.body.clone();
        body["owningProjectIdentifier"] = json!(project);
        let created = dz.call("POST", &assets_uri, Some(body)).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: creating the asset: expected 201, got {} {:?}",
            case.label, created.status, created.body
        );
        let id = created.s("id");
        let asset_uri = format!("{assets_uri}/{id}");

        let mut revisions = vec!["1".to_string()];
        for (index, revision_body) in case.revisions.iter().enumerate() {
            let revised = dz
                .call(
                    "POST",
                    &format!("{asset_uri}/revisions"),
                    Some(revision_body.clone()),
                )
                .await;
            assert_eq!(
                revised.status, 200,
                "case {:?}: recording revision {}: expected 200, got {} {:?}",
                case.label,
                index + 2,
                revised.status,
                revised.body
            );
            revisions.push(revised.s("revision"));
        }

        // The asset is there before the delete, so the 404 afterwards is the delete's
        // doing and not a read that never worked.
        let before = dz.call("GET", &asset_uri, None).await;
        assert_eq!(
            before.status, 200,
            "case {:?}: the asset must be readable before the delete, got {} {:?}",
            case.label, before.status, before.body
        );

        let deleted = dz.call("DELETE", &asset_uri, None).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected 204, got {} {:?}",
            case.label, deleted.status, deleted.body
        );
        // A 204 carries nothing about the asset: `DeleteAssetOutput` has no members
        // (smithy_query operation="DeleteAsset").
        assert!(
            deleted.body == Value::Null || deleted.body == json!({}),
            "case {:?}: a 204 must carry no members, got {:?}",
            case.label,
            deleted.body
        );

        // Every recorded revision went with the asset: the latest read, and each
        // revision the store had served by number.
        for revision in [None]
            .into_iter()
            .chain(revisions.iter().map(|number| Some(number.as_str())))
        {
            let uri = match revision {
                Some(number) => format!("{asset_uri}?revision={number}"),
                None => asset_uri.clone(),
            };
            let gone = dz.call("GET", &uri, None).await;
            assert_eq!(
                (gone.status, gone.error_code.as_str(), gone.s("message")),
                (
                    404,
                    "ResourceNotFoundException",
                    ASSET_NOT_FOUND.to_string()
                ),
                "case {:?}: reading {:?} after the delete: expected the modelled \
                 not-found error, got {} {:?}",
                case.label,
                revision,
                gone.status,
                gone.body
            );
            assert!(
                !gone.s("message").contains(&id),
                "case {:?}: a not-found message must not echo the identifier, got {:?}",
                case.label,
                gone.s("message")
            );
        }

        // A second delete of the same identifier is a miss, not a second 204: the
        // record is gone, so there is nothing left to delete.
        let again = dz.call("DELETE", &asset_uri, None).await;
        assert_eq!(
            (again.status, again.error_code.as_str(), again.s("message")),
            (
                404,
                "ResourceNotFoundException",
                ASSET_NOT_FOUND.to_string()
            ),
            "case {:?}: a repeated delete: expected the modelled not-found error, got {} {:?}",
            case.label,
            again.status,
            again.body
        );

        // The asset beside it is still readable, with its own identity.
        let survivor = dz
            .call("GET", &format!("{assets_uri}/{bystander_id}"), None)
            .await;
        assert_eq!(
            (survivor.status, survivor.s("name").as_str()),
            (200, "bystander"),
            "case {:?}: the asset beside the deleted one must survive, got {} {:?}",
            case.label,
            survivor.status,
            survivor.body
        );
    }
}

#[tokio::test]
async fn an_asset_an_existing_filter_views_cannot_be_deleted() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-delete-conflict-domain").await;
    let project = dz.project(&domain, "conflict owner").await;
    let assets_uri = format!("/v2/domains/{domain}/assets");

    // The rows differ along which member of the `AssetFilterConfiguration` union the
    // filter uses and how many columns it names: the rule is about the filter EXISTING,
    // not about what it selects.
    let configurations: Vec<(&str, Value)> = vec![
        (
            "a column filter naming one column",
            json!({ "columnConfiguration": { "includedColumnNames": ["order_id"] } }),
        ),
        (
            "a column filter naming several columns",
            json!({ "columnConfiguration": { "includedColumnNames": ["order_id", "region"] } }),
        ),
        (
            "a column filter naming no column at all",
            json!({ "columnConfiguration": { "includedColumnNames": [] } }),
        ),
        (
            "a row filter over one expression",
            json!({
                "rowConfiguration": {
                    "rowFilter": {
                        "expression": { "equalTo": { "columnName": "region", "value": "us-west-2" } },
                    },
                },
            }),
        ),
        (
            "a row filter over a group of expressions",
            json!({
                "rowConfiguration": {
                    "rowFilter": { "and": [
                        { "expression": { "isNotNull": { "columnName": "region" } } },
                        { "expression": { "greaterThan": { "columnName": "amount", "value": "10" } } },
                    ] },
                    "sensitive": true,
                },
            }),
        ),
    ];

    for (index, (label, configuration)) in configurations.into_iter().enumerate() {
        let created = dz
            .call(
                "POST",
                &assets_uri,
                Some(json!({
                    "name": format!("filtered-{index}"),
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                    "formsInput": [{
                        "formName": "RelationalTableForm",
                        "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                        "content": "{\"columns\":[{\"columnName\":\"order_id\",\"dataType\":\"bigint\"},\
                                     {\"columnName\":\"region\",\"dataType\":\"string\"},\
                                     {\"columnName\":\"amount\",\"dataType\":\"bigint\"}]}",
                    }],
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: creating the asset: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.s("id");
        let asset_uri = format!("{assets_uri}/{id}");

        let filter = dz
            .call(
                "POST",
                &format!("{asset_uri}/filters"),
                Some(json!({ "name": format!("view-{index}"), "configuration": configuration })),
            )
            .await;
        assert_eq!(
            filter.status, 201,
            "case {label:?}: creating the filter: expected 201, got {} {:?}",
            filter.status, filter.body
        );
        let filter_id = filter.s("id");

        let refused = dz.call("DELETE", &asset_uri, None).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {label:?}: expected the modelled conflict, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.s("message"),
            deletion_prevented_message(
                &asset_object(&domain, &id),
                &asset_object(&domain, &filter_id)
            ),
            "case {label:?}: the refusal must name the asset and the filter that \
             prevented it, got {:?}",
            refused.s("message")
        );

        // The refusal stored nothing: the asset, and the filter on it, are both still
        // there afterwards.
        let survivor = dz.call("GET", &asset_uri, None).await;
        assert_eq!(
            (survivor.status, survivor.s("id").as_str()),
            (200, id.as_str()),
            "case {label:?}: a refused delete must store nothing, got {} {:?}",
            survivor.status,
            survivor.body
        );
        let kept_filter = dz
            .call("GET", &format!("{asset_uri}/filters/{filter_id}"), None)
            .await;
        assert_eq!(
            (kept_filter.status, kept_filter.s("id").as_str()),
            (200, filter_id.as_str()),
            "case {label:?}: the filter must survive the refusal, got {} {:?}",
            kept_filter.status,
            kept_filter.body
        );
    }

    // An asset in the same domain that no filter views still deletes, so the refusal
    // above is about the filter and not about the domain holding one.
    let unfiltered = dz
        .call(
            "POST",
            &assets_uri,
            Some(json!({
                "name": "unfiltered",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        unfiltered.status, 201,
        "creating the unfiltered asset: expected 201, got {} {:?}",
        unfiltered.status, unfiltered.body
    );
    let deleted = dz
        .call(
            "DELETE",
            &format!("{assets_uri}/{}", unfiltered.s("id")),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "an asset no filter views must delete: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
}

#[tokio::test]
async fn a_delete_the_service_cannot_satisfy_is_refused_with_a_modelled_error() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-delete-refusal-domain").await;
    let other_domain = dz.available_domain("asset-delete-refusal-other").await;
    let project = dz.project(&domain, "refusal owner").await;
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "scoped",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the scoped asset: expected 201, got {} {:?}",
        created.status, created.body
    );
    let id = created.s("id");

    // (label, domain in the path, asset in the path, status, wire code)
    let cases: Vec<(&str, String, String, u16, &str)> = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // delete before any lookup.
            "a well-formed domain id that names no domain",
            "dzd_doesnotexist".to_string(),
            id.clone(),
            403,
            "AccessDeniedException",
        ),
        (
            "a real domain and an asset nobody created",
            domain.clone(),
            "nosuchasset".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            // The asset is stored in `domain`, so the OTHER domain does not hold it.
            "an asset that lives in another domain",
            other_domain.clone(),
            id.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            // `GetAsset` addresses an asset by the exact stored id, so a delete does
            // too: a different spelling of a stored id names no asset.
            "a different case of a stored identifier",
            domain.clone(),
            id.to_uppercase(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a domain id outside the modelled pattern",
            "not-a-domain".to_string(),
            id.clone(),
            400,
            "ValidationException",
        ),
        (
            "an identifier one character past the modelled maximum",
            domain.clone(),
            "a".repeat(IDENTIFIER_MAX + 1),
            400,
            "ValidationException",
        ),
        (
            "an identifier holding a character the pattern does not admit",
            domain.clone(),
            "not.an.asset".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (label, path_domain, path_asset, status, code) in cases {
        let answer = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{path_domain}/assets/{path_asset}"),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
        if status == 404 {
            // The message names the KIND that was not found and never the identifier
            // the caller supplied.
            let message = answer.s("message");
            assert!(
                message == DOMAIN_NOT_FOUND || message == ASSET_NOT_FOUND,
                "case {label:?}: expected a not-found message naming only the kind, got \
                 {message:?}"
            );
            assert!(
                !message.contains(&path_asset) && !message.contains(&path_domain),
                "case {label:?}: a not-found message must not echo the request, got {message:?}"
            );
        }
    }

    // Every refusal above left the stored asset alone.
    let survivor = dz
        .call("GET", &format!("/v2/domains/{domain}/assets/{id}"), None)
        .await;
    assert_eq!(
        (survivor.status, survivor.s("name").as_str()),
        (200, "scoped"),
        "a refused delete must store nothing, got {} {:?}",
        survivor.status,
        survivor.body
    );
}

#[tokio::test]
async fn an_asset_in_a_domain_being_deleted_is_already_gone() {
    let dz = Emulator::new();
    let domain = dz.available_domain("asset-delete-deleting-domain").await;
    let project = dz.project(&domain, "deleting owner").await;
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "doomed",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the doomed asset: expected 201, got {} {:?}",
        created.status, created.body
    );
    let id = created.s("id");

    let domain_deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        domain_deleted.status, 202,
        "DeleteDomain: expected 202, got {} {:?}",
        domain_deleted.status, domain_deleted.body
    );

    // Straight away, while the domain is still reported DELETING, the asset is already
    // gone with the domain that held it; once the clock has taken the domain away the
    // identifier names no domain at all, so authorization denies the call instead.
    let cases: [(&str, bool, u16, &str, String); 2] = [
        (
            "while the domain is DELETING",
            false,
            404,
            "ResourceNotFoundException",
            DOMAIN_NOT_FOUND.to_string(),
        ),
        (
            "once it is gone",
            true,
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: DeleteAsset".to_string(),
        ),
    ];
    for (label, wait, status, code, message) in cases {
        if wait {
            tokio::time::sleep(SETTLED).await;
        }
        let answer = dz
            .call("DELETE", &format!("/v2/domains/{domain}/assets/{id}"), None)
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str(), answer.s("message")),
            (status, code, message),
            "case {label:?}: expected the domain to be unreachable, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
