// SPDX-License-Identifier: CC0-1.0

//! An OpsItem's related items end to end: `AssociateOpsItemRelatedItem` associates a
//! resource with an OpsItem and `ListOpsItemRelatedItems` reports what was associated.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`], [`error_code`])
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the
//! multi-service router. Nothing reaches into the state struct, so a response that is not
//! computed from persisted state cannot pass: every listing follows an association.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — the association type, the resource type, the resource ARN, how many
//! associations an OpsItem carries, which filter narrows the listing, the page size, the
//! Region — rather than the single association a recorded unit happened to make.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, REGION};

/// The association types "OpsCenter supports" (`smithy_query(query="doc",
/// shape="AssociateOpsItemRelatedItemRequest", member="AssociationType")`: "OpsCenter
/// supports IsParentOf and RelatesTo association types").
const ASSOCIATION_TYPES: &[&str] = &["IsParentOf", "RelatesTo"];

/// The resource types OpsCenter supports as a related item (`smithy_query(query="doc",
/// shape="AssociateOpsItemRelatedItemRequest", member="ResourceType")`:
/// "AWS::SSMIncidents::IncidentRecord: an Incident Manager incident. AWS::SSM::Document: a
/// Systems Manager (SSM) document").
const RESOURCE_TYPES: &[&str] = &["AWS::SSMIncidents::IncidentRecord", "AWS::SSM::Document"];

/// The filter keys a related-item listing accepts (`smithy_query(query="shape",
/// shape="OpsItemRelatedItemsFilterKey")`: `ResourceType`, `AssociationId`,
/// `ResourceUri`).
const FILTER_KEYS: &[&str] = &["ResourceType", "AssociationId", "ResourceUri"];

/// The bounds the modelled `OpsItemRelatedItemsMaxResults` carries (`smithy_query(
/// query="operation", operation="ListOpsItemRelatedItems")`: range 1..50).
const PAGE_SIZE_BOUNDS: &[i64] = &[1, 50];

/// The message Systems Manager words for `OpsItemRelatedItemAlreadyExistsException`: the
/// name of the input member whose value is already associated with the OpsItem, then the
/// frame the service appends to a refusal of this operation — the service it came from, the
/// status and error code it carries, and the request id of the response it travels on,
/// which this protocol's responses do not carry.
const ALREADY_EXISTS_MESSAGE: &str = "ResourceUri (Service: AmazonSSM; Status Code: 400; \
     Error Code: OpsItemRelatedItemAlreadyExistsException; Request ID: None; Proxy: null)";

/// An OpsItem to hang related items off, returning its id.
async fn ops_item(router: &MultiServiceRouter, title: &str) -> String {
    ops_item_in_region(router, REGION, title).await
}

/// The same, at `region`'s endpoint.
async fn ops_item_in_region(router: &MultiServiceRouter, region: &str, title: &str) -> String {
    let (status, response) = call_in_region(
        router,
        region,
        "CreateOpsItem",
        &json!({
            "Title": title,
            "Description": format!("what {title} is about"),
            "Source": "emulator-tests",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {title}: CreateOpsItem must succeed, got {status} {response}"
    );
    response["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {title}: CreateOpsItem returned no OpsItemId: {response}"))
        .to_string()
}

/// `AssociateOpsItemRelatedItem` with `body`, returning `(status, body)`.
async fn associate(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "AssociateOpsItemRelatedItem", body).await
}

/// `AssociateOpsItemRelatedItem`, asserting it succeeded, and returning the association
/// id it minted.
async fn associate_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = associate(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: AssociateOpsItemRelatedItem must succeed, got {status} {response}"
    );
    response["AssociationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no AssociationId in {response}"))
        .to_string()
}

/// `ListOpsItemRelatedItems` with `body`, asserting it succeeded.
async fn list_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "ListOpsItemRelatedItems", body).await;
    assert_eq!(
        status, 200,
        "case {case}: ListOpsItemRelatedItems must succeed, got {status} {response}"
    );
    response
}

/// The summaries a listing of `ops_item_id` reports.
async fn summaries(router: &MultiServiceRouter, case: &str, ops_item_id: &str) -> Vec<Value> {
    let response = list_ok(router, case, &json!({ "OpsItemId": ops_item_id })).await;
    response["Summaries"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: no Summaries in {response}"))
}

/// Whether `id` has the UUID form a minted `AssociationId` carries: 8-4-4-4-12 lowercase
/// hex digits.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(want, group)| {
                group.len() == *want
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
}

/// One row of the round-trip table: the association a request makes.
struct RoundTrip<'a> {
    case: &'a str,
    association_type: &'a str,
    resource_type: &'a str,
    resource_uri: &'a str,
}

/// Every supported form of association round-trips: what the association persisted is what
/// a later `ListOpsItemRelatedItems` reports, and the members the request never named — the
/// association id, the creator, the times — are the service's own.
#[tokio::test]
async fn associated_related_items_round_trip_through_the_listing() {
    let router = router();
    // One row per (association type, resource type) pair the docs name, so the table spans
    // the whole supported class rather than the one pair a recording used.
    let mut cases = Vec::new();
    for association_type in ASSOCIATION_TYPES {
        for resource_type in RESOURCE_TYPES {
            let resource_uri = if *resource_type == "AWS::SSM::Document" {
                "arn:aws:ssm:us-west-2:111122223333:document/RunPatchBaseline"
            } else {
                "arn:aws:ssm-incidents::111122223333:incident-record/response-plan/1a2b3c"
            };
            cases.push(RoundTrip {
                case: "supported pair",
                association_type,
                resource_type,
                resource_uri,
            });
        }
    }

    for row in &cases {
        let case = format!("{} {}/{}", row.case, row.association_type, row.resource_type);
        let ops_item_id = ops_item(&router, &case).await;
        let request = json!({
            "OpsItemId": ops_item_id,
            "AssociationType": row.association_type,
            "ResourceType": row.resource_type,
            "ResourceUri": row.resource_uri,
        });
        let association_id = associate_ok(&router, &case, &request).await;
        assert!(
            is_uuid(&association_id),
            "case {case}: AssociationId must be a minted UUID, got {association_id:?}"
        );

        let reported = summaries(&router, &case, &ops_item_id).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case}: the OpsItem must report exactly its one related item, got {reported:?}"
        );
        let summary = &reported[0];
        assert_eq!(
            summary["AssociationId"].as_str(),
            Some(association_id.as_str()),
            "case {case}: expected the minted association id, got {summary:?}"
        );
        assert_eq!(
            summary["OpsItemId"].as_str(),
            Some(ops_item_id.as_str()),
            "case {case}: expected the OpsItem the association was made on, got {summary:?}"
        );
        assert_eq!(
            summary["AssociationType"].as_str(),
            Some(row.association_type),
            "case {case}: expected the association type the request named, got {summary:?}"
        );
        assert_eq!(
            summary["ResourceType"].as_str(),
            Some(row.resource_type),
            "case {case}: expected the resource type the request named, got {summary:?}"
        );
        assert_eq!(
            summary["ResourceUri"].as_str(),
            Some(row.resource_uri),
            "case {case}: expected the resource ARN the request named, got {summary:?}"
        );
        assert!(
            summary["CreatedBy"]["Arn"]
                .as_str()
                .is_some_and(|arn| arn.starts_with("arn:aws:")),
            "case {case}: expected the creating identity's ARN, got {summary:?}"
        );
        assert_eq!(
            summary["LastModifiedBy"]["Arn"], summary["CreatedBy"]["Arn"],
            "case {case}: nothing has updated the association, so its last modifier is its \
             creator, got {summary:?}"
        );
        let created = summary["CreatedTime"].as_f64();
        assert!(
            created.is_some_and(|seconds| seconds > 0.0),
            "case {case}: expected the time the association was created, got {summary:?}"
        );
        assert_eq!(
            summary["LastModifiedTime"].as_f64(),
            created,
            "case {case}: nothing has updated the association, so its last update is its \
             create, got {summary:?}"
        );
    }
}

/// One OpsItem carries every resource associated with it, and each association is reported
/// once — however many associations it holds.
#[tokio::test]
async fn an_ops_item_reports_every_resource_associated_with_it() {
    let router = router();
    // The row is the COUNT of associations: the rule holds for one resource and for many.
    for count in [1usize, 2, 5] {
        let case = format!("{count} related item(s)");
        let ops_item_id = ops_item(&router, &case).await;
        let mut minted = Vec::new();
        for index in 0..count {
            let association_id = associate_ok(
                &router,
                &case,
                &json!({
                    "OpsItemId": ops_item_id,
                    "AssociationType": ASSOCIATION_TYPES[index % ASSOCIATION_TYPES.len()],
                    "ResourceType": RESOURCE_TYPES[index % RESOURCE_TYPES.len()],
                    "ResourceUri": format!(
                        "arn:aws:ssm:us-west-2:111122223333:document/Runbook-{index}"
                    ),
                }),
            )
            .await;
            minted.push(association_id);
        }
        minted.sort();
        let mut unique = minted.clone();
        unique.dedup();
        assert_eq!(
            unique.len(),
            minted.len(),
            "case {case}: two associations of one store must not carry one id, got {minted:?}"
        );

        let reported = summaries(&router, &case, &ops_item_id).await;
        let mut reported_ids: Vec<String> = reported
            .iter()
            .filter_map(|summary| summary["AssociationId"].as_str().map(str::to_string))
            .collect();
        reported_ids.sort();
        assert_eq!(
            reported_ids, minted,
            "case {case}: the listing must report exactly the associations that were made"
        );
    }
}

/// A listing of an OpsItem reports only ITS related items, and only the ones created at the
/// Region it is read through.
#[tokio::test]
async fn a_listing_is_scoped_to_one_ops_item_and_one_region() {
    let router = router();
    let first = ops_item(&router, "the first OpsItem").await;
    let second = ops_item(&router, "the second OpsItem").await;
    let shared_uri = "arn:aws:ssm:us-west-2:111122223333:document/SharedRunbook";

    // The same resource may be a related item of two different OpsItems: the one
    // association per resource is per OpsItem.
    let on_first = associate_ok(
        &router,
        "the first OpsItem",
        &json!({
            "OpsItemId": first,
            "AssociationType": "RelatesTo",
            "ResourceType": "AWS::SSM::Document",
            "ResourceUri": shared_uri,
        }),
    )
    .await;
    let on_second = associate_ok(
        &router,
        "the second OpsItem",
        &json!({
            "OpsItemId": second,
            "AssociationType": "IsParentOf",
            "ResourceType": "AWS::SSM::Document",
            "ResourceUri": shared_uri,
        }),
    )
    .await;
    assert_ne!(
        on_first, on_second,
        "case two OpsItems, one resource: each association is its own, got {on_first} twice"
    );

    for (case, ops_item_id, expected) in [
        ("the first OpsItem", &first, &on_first),
        ("the second OpsItem", &second, &on_second),
    ] {
        let reported = summaries(&router, case, ops_item_id).await;
        let ids: Vec<&str> = reported
            .iter()
            .filter_map(|summary| summary["AssociationId"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![expected.as_str()],
            "case {case}: a listing reports only this OpsItem's related items"
        );
    }

    // An OpsItem lives at the endpoint it was created through, so an association made
    // there is not reported at another Region's endpoint.
    let elsewhere = "eu-central-1";
    let response = call_in_region(
        &router,
        elsewhere,
        "ListOpsItemRelatedItems",
        &json!({ "OpsItemId": first }),
    )
    .await;
    assert_eq!(
        response.0, 200,
        "case another Region: the listing must succeed, got {} {}",
        response.0, response.1
    );
    assert_eq!(
        response.1["Summaries"].as_array().map(Vec::len),
        Some(0),
        "case another Region: {elsewhere} holds no association of this OpsItem, got {}",
        response.1
    );
}

/// One resource ARN is a related item of an OpsItem at most once: the second association of
/// the same ARN is refused and nothing is added.
#[tokio::test]
async fn associating_one_resource_twice_is_refused() {
    let router = router();
    // The row is the resource ARN, so the rule is asserted on every supported resource
    // type rather than on the one a recording used.
    for resource_type in RESOURCE_TYPES {
        let case = format!("a second association of a {resource_type}");
        let ops_item_id = ops_item(&router, &case).await;
        let resource_uri =
            format!("arn:aws:ssm:us-west-2:111122223333:document/Twice-{resource_type}");
        let request = json!({
            "OpsItemId": ops_item_id,
            "AssociationType": "RelatesTo",
            "ResourceType": resource_type,
            "ResourceUri": resource_uri,
        });
        let association_id = associate_ok(&router, &case, &request).await;

        let (status, response) = associate(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: must be refused with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemRelatedItemAlreadyExistsException",
            "case {case}: expected the already-exists refusal, got {response}"
        );
        assert_eq!(
            response["Message"].as_str(),
            Some(ALREADY_EXISTS_MESSAGE),
            "case {case}: expected the message Systems Manager words for this error class, \
             got {response}"
        );
        assert_eq!(
            response["ResourceUri"].as_str(),
            Some(resource_uri.as_str()),
            "case {case}: the refusal names the ARN it collided on, got {response}"
        );
        assert_eq!(
            response["OpsItemId"].as_str(),
            Some(ops_item_id.as_str()),
            "case {case}: the refusal names the OpsItem it collided on, got {response}"
        );

        let reported = summaries(&router, &case, &ops_item_id).await;
        let ids: Vec<&str> = reported
            .iter()
            .filter_map(|summary| summary["AssociationId"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec![association_id.as_str()],
            "case {case}: the refused association must not have been persisted"
        );
    }
}

/// One row of the refusal table: a request that cannot be served, and the error it is
/// answered with.
struct Refusal<'a> {
    case: &'a str,
    body: Value,
    code: &'a str,
}

/// An association that names an OpsItem the Region holds none of, or a type outside the
/// documented sets, is refused — and the refusal names the member at fault.
#[tokio::test]
async fn associations_outside_the_documented_rules_are_refused() {
    let router = router();
    let ops_item_id = ops_item(&router, "an OpsItem to refuse associations on").await;
    let resource_uri = "arn:aws:ssm:us-west-2:111122223333:document/RunPatchBaseline";
    let cases = vec![
        Refusal {
            case: "an OpsItem id nothing created",
            body: json!({
                "OpsItemId": "oi-0123456789ab",
                "AssociationType": "RelatesTo",
                "ResourceType": "AWS::SSM::Document",
                "ResourceUri": resource_uri,
            }),
            code: "OpsItemNotFoundException",
        },
        Refusal {
            case: "an association type OpsCenter does not support",
            body: json!({
                "OpsItemId": ops_item_id,
                "AssociationType": "IsChildOf",
                "ResourceType": "AWS::SSM::Document",
                "ResourceUri": resource_uri,
            }),
            code: "OpsItemInvalidParameterException",
        },
        Refusal {
            case: "an association type in the wrong case",
            body: json!({
                "OpsItemId": ops_item_id,
                "AssociationType": "relatesto",
                "ResourceType": "AWS::SSM::Document",
                "ResourceUri": resource_uri,
            }),
            code: "OpsItemInvalidParameterException",
        },
        Refusal {
            case: "a resource type OpsCenter does not relate",
            body: json!({
                "OpsItemId": ops_item_id,
                "AssociationType": "RelatesTo",
                "ResourceType": "AWS::S3::Bucket",
                "ResourceUri": "arn:aws:s3:::a-bucket",
            }),
            code: "OpsItemInvalidParameterException",
        },
    ];

    for row in &cases {
        let (status, response) = associate(&router, &row.body).await;
        assert_eq!(
            status, 400,
            "case {}: must be refused with 400, got {status} {response}",
            row.case
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {}: expected {}, got {response}",
            row.case,
            row.code
        );
    }

    // A refused request persists nothing: the OpsItem still holds no related item.
    let reported = summaries(&router, "after the refusals", &ops_item_id).await;
    assert!(
        reported.is_empty(),
        "case after the refusals: nothing may have been persisted, got {reported:?}"
    );
}

/// A refusal of a type outside the documented sets names the member at fault, so a client
/// learns which one to correct.
#[tokio::test]
async fn a_type_refusal_names_the_member_at_fault() {
    let router = router();
    let ops_item_id = ops_item(&router, "an OpsItem for parameter names").await;
    for (member, body) in [
        (
            "AssociationType",
            json!({
                "OpsItemId": ops_item_id,
                "AssociationType": "Mentions",
                "ResourceType": "AWS::SSM::Document",
                "ResourceUri": "arn:aws:ssm:us-west-2:111122223333:document/A",
            }),
        ),
        (
            "ResourceType",
            json!({
                "OpsItemId": ops_item_id,
                "AssociationType": "RelatesTo",
                "ResourceType": "AWS::EC2::Instance",
                "ResourceUri": "arn:aws:ec2:us-west-2:111122223333:instance/i-0123",
            }),
        ),
    ] {
        let (status, response) = associate(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {member}: must be refused with 400, got {status} {response}"
        );
        assert_eq!(
            response["ParameterNames"]
                .as_array()
                .and_then(|names| names.first())
                .and_then(Value::as_str),
            Some(member),
            "case {member}: the refusal must name the member at fault, got {response}"
        );
    }
}

/// Each filter key narrows a listing to the associations whose property it names, and the
/// values of one filter are alternatives.
#[tokio::test]
async fn every_filter_key_narrows_the_listing() {
    let router = router();
    let ops_item_id = ops_item(&router, "an OpsItem with two related items").await;
    let document_uri = "arn:aws:ssm:us-west-2:111122223333:document/RunPatchBaseline";
    let incident_uri = "arn:aws:ssm-incidents::111122223333:incident-record/response-plan/1a2b3c";
    let document = associate_ok(
        &router,
        "the document",
        &json!({
            "OpsItemId": ops_item_id,
            "AssociationType": "RelatesTo",
            "ResourceType": "AWS::SSM::Document",
            "ResourceUri": document_uri,
        }),
    )
    .await;
    let incident = associate_ok(
        &router,
        "the incident",
        &json!({
            "OpsItemId": ops_item_id,
            "AssociationType": "IsParentOf",
            "ResourceType": "AWS::SSMIncidents::IncidentRecord",
            "ResourceUri": incident_uri,
        }),
    )
    .await;

    // One row per filter key, each asked for the document's own value — so every key the
    // model declares is exercised, not just the one a recording filtered on.
    let expected_for_key = |key: &str| -> String {
        match key {
            "ResourceType" => "AWS::SSM::Document".to_string(),
            "AssociationId" => document.clone(),
            _ => document_uri.to_string(),
        }
    };
    for key in FILTER_KEYS {
        let case = format!("the {key} filter");
        let response = list_ok(
            &router,
            &case,
            &json!({
                "OpsItemId": ops_item_id,
                "Filters": [{
                    "Key": key,
                    "Values": [expected_for_key(key)],
                    "Operator": "Equal",
                }],
            }),
        )
        .await;
        let ids: Vec<&str> = response["Summaries"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|summary| summary["AssociationId"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![document.as_str()],
            "case {case}: expected only the document's association, got {}",
            response
        );
    }

    // The values of one filter are alternatives, so naming both resource types reports
    // both associations.
    let response = list_ok(
        &router,
        "both resource types",
        &json!({
            "OpsItemId": ops_item_id,
            "Filters": [{
                "Key": "ResourceType",
                "Values": RESOURCE_TYPES,
                "Operator": "Equal",
            }],
        }),
    )
    .await;
    let mut ids: Vec<String> = response["Summaries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|summary| summary["AssociationId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    let mut both = vec![document.clone(), incident.clone()];
    both.sort();
    assert_eq!(
        ids, both,
        "case both resource types: the values of one filter are alternatives, got {response}"
    );

    // A value nothing holds narrows to nothing rather than to everything.
    let response = list_ok(
        &router,
        "a value nothing holds",
        &json!({
            "OpsItemId": ops_item_id,
            "Filters": [{
                "Key": "ResourceUri",
                "Values": ["arn:aws:ssm:us-west-2:111122223333:document/Nothing"],
                "Operator": "Equal",
            }],
        }),
    )
    .await;
    assert_eq!(
        response["Summaries"].as_array().map(Vec::len),
        Some(0),
        "case a value nothing holds: expected no summary, got {response}"
    );
}

/// A filter this operation cannot serve is refused rather than ignored.
#[tokio::test]
async fn an_unservable_filter_is_refused() {
    let router = router();
    let ops_item_id = ops_item(&router, "an OpsItem with one related item").await;
    associate_ok(
        &router,
        "the related item",
        &json!({
            "OpsItemId": ops_item_id,
            "AssociationType": "RelatesTo",
            "ResourceType": "AWS::SSM::Document",
            "ResourceUri": "arn:aws:ssm:us-west-2:111122223333:document/RunPatchBaseline",
        }),
    )
    .await;

    for (case, filter) in [
        (
            "a key no related item holds",
            json!({ "Key": "OpsItemId", "Values": [ops_item_id], "Operator": "Equal" }),
        ),
        (
            "an operator this listing does not support",
            json!({
                "Key": "ResourceType",
                "Values": ["AWS::SSM::Document"],
                "Operator": "Contains",
            }),
        ),
        (
            "a filter carrying no value",
            json!({ "Key": "ResourceType", "Values": [], "Operator": "Equal" }),
        ),
    ] {
        let (status, response) = call(
            &router,
            "ListOpsItemRelatedItems",
            &json!({ "OpsItemId": ops_item_id, "Filters": [filter] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: must be refused with 400, got {status} {response}"
        );
    }
}

/// A listing pages: every page is at most `MaxResults` long, the token resumes where the
/// page stopped, and walking the pages reports each association exactly once.
#[tokio::test]
async fn a_listing_pages_through_every_association() {
    let router = router();
    let ops_item_id = ops_item(&router, "an OpsItem with five related items").await;
    let mut minted = Vec::new();
    for index in 0..5 {
        minted.push(
            associate_ok(
                &router,
                "a related item",
                &json!({
                    "OpsItemId": ops_item_id,
                    "AssociationType": "RelatesTo",
                    "ResourceType": "AWS::SSM::Document",
                    "ResourceUri": format!(
                        "arn:aws:ssm:us-west-2:111122223333:document/Paged-{index}"
                    ),
                }),
            )
            .await,
        );
    }
    minted.sort();

    // The row is the page size: each published bound of the modelled range, and a size
    // between them.
    let mut sizes: Vec<i64> = PAGE_SIZE_BOUNDS.to_vec();
    sizes.push(2);
    for page_size in sizes {
        let case = format!("pages of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..minted.len() + 1 {
            let mut request = json!({ "OpsItemId": ops_item_id, "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let response = list_ok(&router, &case, &request).await;
            let page = response["Summaries"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() as i64 <= page_size,
                "case {case}: a page holds at most {page_size} summaries, got {}",
                page.len()
            );
            walked.extend(
                page.iter()
                    .filter_map(|summary| summary["AssociationId"].as_str().map(str::to_string)),
            );
            token = response["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case}: the walk must end without a token, got {token:?}"
        );
        walked.sort();
        assert_eq!(
            walked, minted,
            "case {case}: the pages together report every association exactly once"
        );
    }

    // A page size outside the modelled range is refused, at either side of it.
    for page_size in [0, 51] {
        let (status, response) = call(
            &router,
            "ListOpsItemRelatedItems",
            &json!({ "OpsItemId": ops_item_id, "MaxResults": page_size }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults {page_size}: must be refused with 400, got {status} {response}"
        );
    }

    // A token this listing never handed out resumes nothing, so it is refused rather than
    // served as the first page.
    let (status, response) = call(
        &router,
        "ListOpsItemRelatedItems",
        &json!({ "OpsItemId": ops_item_id, "NextToken": "not-a-token-this-listing-minted" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case an invented token: must be refused with 400, got {status} {response}"
    );
}

/// A listing that names no OpsItem reports the Region's associations across every OpsItem,
/// and an OpsItem nothing associated anything with reports none.
#[tokio::test]
async fn a_listing_without_an_ops_item_spans_the_region() {
    let router = router();
    let mut expected = Vec::new();
    for index in 0..3 {
        let case = format!("OpsItem {index}");
        let ops_item_id = ops_item(&router, &case).await;
        expected.push(
            associate_ok(
                &router,
                &case,
                &json!({
                    "OpsItemId": ops_item_id,
                    "AssociationType": ASSOCIATION_TYPES[index % ASSOCIATION_TYPES.len()],
                    "ResourceType": "AWS::SSM::Document",
                    "ResourceUri": format!(
                        "arn:aws:ssm:us-west-2:111122223333:document/Spanning-{index}"
                    ),
                }),
            )
            .await,
        );
    }
    expected.sort();

    let response = list_ok(&router, "every OpsItem", &json!({})).await;
    let mut reported: Vec<String> = response["Summaries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|summary| summary["AssociationId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    reported.sort();
    assert_eq!(
        reported, expected,
        "case every OpsItem: a listing naming no OpsItem reports the Region's associations"
    );

    let bare = ops_item(&router, "an OpsItem with nothing associated").await;
    let reported = summaries(&router, "an OpsItem with nothing associated", &bare).await;
    assert!(
        reported.is_empty(),
        "case an OpsItem with nothing associated: expected no summary, got {reported:?}"
    );
}
