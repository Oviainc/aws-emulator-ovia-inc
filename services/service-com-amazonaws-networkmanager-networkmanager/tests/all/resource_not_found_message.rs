// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` message rule, over every operation of this service
//! that can raise one.
//!
//! The model declares `Message` required but states no text; two independent recordings
//! of different resources through different operations (`UpdateGlobalNetwork` on a
//! deleted global network, `TagResource` on the ARN of a deleted site) both answer the
//! identical string. So the rule is: the message is a fixed, resource-INDEPENDENT
//! constant and the identifying detail travels in the required `ResourceId` and
//! `ResourceType` members, never in the prose.
//!
//! Every row below addresses a different resource kind through a different operation, so
//! a message built from the request would differ between rows and fail here.
//!
//! Every row is an operation acting on an EXISTING resource, which is what makes the
//! absent resource a not-found. A CREATE whose input merely REFERENCES a parent that does
//! not exist is the other half of the rule — a field-validation failure on the referencing
//! member — and is pinned by `unknown_parent_reference`.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_arn, Emulator};
use crate::resource_tagging::{tags_uri, untag_uri};

/// The constant text AWS answers, from the two recordings above.
const NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The ARN resource types this service holds, which are also the `ResourceType` values a
/// not-found refusal reports.
const GLOBAL_NETWORK: &str = "global-network";
/// See [`GLOBAL_NETWORK`].
const SITE: &str = "site";
/// See [`GLOBAL_NETWORK`].
const LINK: &str = "link";

/// One row: a request that must be refused as not found, and the resource the refusal
/// has to identify.
struct NotFoundCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    resource_id: String,
    resource_type: &'static str,
}

/// Assert the refusal `case` produces: 404, the modelled code, the CONSTANT message, and
/// the identifying detail in `ResourceId` / `ResourceType`.
async fn assert_not_found(emulator: &Emulator, row: &NotFoundCase) {
    let NotFoundCase {
        case,
        method,
        uri,
        body,
        resource_id,
        resource_type,
    } = row;
    let (status, answer) = emulator.call(method, uri, body.clone()).await;
    assert_eq!(
        status, 404,
        "case {case}: expected 404, got {status} with {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case {case}: wrong error code in {answer}"
    );
    assert_eq!(
        answer["Message"].as_str(),
        Some(NOT_FOUND_MESSAGE),
        "case {case}: expected the constant message {NOT_FOUND_MESSAGE:?}, got {answer}"
    );
    assert_eq!(
        answer["ResourceId"].as_str(),
        Some(resource_id.as_str()),
        "case {case}: expected ResourceId {resource_id:?}, got {answer}"
    );
    assert_eq!(
        answer["ResourceType"].as_str(),
        Some(*resource_type),
        "case {case}: expected ResourceType {resource_type:?}, got {answer}"
    );
}

#[tokio::test]
async fn every_not_found_refusal_carries_the_constant_message_and_identifies_the_resource() {
    let emulator = Emulator::new();

    // A global network and a site that are then deleted, so their ids and ARNs are
    // well-formed and name nothing — the recordings' own situation.
    let gone_network = emulator.create_global_network("to be deleted").await;
    let live_network = emulator.create_global_network("kept").await;
    let gone_site = emulator.create_site(&live_network, "to be deleted").await;
    let (status, deleted_site) = emulator
        .delete(&format!(
            "/global-networks/{live_network}/sites/{gone_site}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite: expected 200, got {status} with {deleted_site}"
    );
    let (status, deleted_network) = emulator
        .delete(&format!("/global-networks/{gone_network}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {deleted_network}"
    );

    // Ids and ARNs that were never minted, for the rows that address a resource the store
    // never held at all.
    let unknown_network = "global-network-00000000000000099".to_string();
    let unknown_site = "site-00000000000000099".to_string();
    let unknown_link = "link-00000000000000099".to_string();

    let rows = vec![
        NotFoundCase {
            case: "UpdateGlobalNetwork on a deleted global network",
            method: "PATCH",
            uri: format!("/global-networks/{gone_network}"),
            body: Some(json!({ "Description": "post-delete" })),
            resource_id: gone_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            case: "DeleteGlobalNetwork on a deleted global network",
            method: "DELETE",
            uri: format!("/global-networks/{gone_network}"),
            body: None,
            resource_id: gone_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            case: "GetSites on a global network that never existed",
            method: "GET",
            uri: format!("/global-networks/{unknown_network}/sites"),
            body: None,
            resource_id: unknown_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            case: "GetLinks on a global network that never existed",
            method: "GET",
            uri: format!("/global-networks/{unknown_network}/links"),
            body: None,
            resource_id: unknown_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            case: "DeleteSite on a deleted site",
            method: "DELETE",
            uri: format!("/global-networks/{live_network}/sites/{gone_site}"),
            body: None,
            resource_id: gone_site.clone(),
            resource_type: SITE,
        },
        NotFoundCase {
            case: "TagResource on the ARN of a deleted site",
            method: "POST",
            uri: tags_uri(&expected_arn(SITE, &gone_site)),
            body: Some(json!({ "Tags": [{ "Key": "env", "Value": "test" }] })),
            resource_id: gone_site.clone(),
            resource_type: SITE,
        },
        NotFoundCase {
            case: "ListTagsForResource on the ARN of a deleted global network",
            method: "GET",
            uri: tags_uri(&expected_arn(GLOBAL_NETWORK, &gone_network)),
            body: None,
            resource_id: gone_network.clone(),
            resource_type: GLOBAL_NETWORK,
        },
        NotFoundCase {
            case: "UntagResource on the ARN of a link that never existed",
            method: "DELETE",
            uri: untag_uri(&expected_arn(LINK, &unknown_link), &["env"]),
            body: None,
            resource_id: unknown_link.clone(),
            resource_type: LINK,
        },
        NotFoundCase {
            case: "ListTagsForResource on the ARN of a site that never existed",
            method: "GET",
            uri: tags_uri(&expected_arn(SITE, &unknown_site)),
            body: None,
            resource_id: unknown_site,
            resource_type: SITE,
        },
    ];

    for row in &rows {
        assert_not_found(&emulator, row).await;
    }
}
