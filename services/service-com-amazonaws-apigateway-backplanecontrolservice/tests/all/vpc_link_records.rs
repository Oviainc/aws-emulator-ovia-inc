// SPDX-License-Identifier: CC0-1.0

//! What a `CreateVpcLink` call ADDS to the account, asserted on the wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateVpcLink`/`GetVpcLink` helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle` and `vpc_links`.
//!
//! The RULE under test: each accepted create stores ANOTHER link under an id the service
//! mints for it, and each stored link keeps exactly the members its OWN request set — so
//! a create never rewrites, merges into or borrows from a link already stored, whatever
//! the two requests have in common. A VPC link is addressed by its `id`, "the identifier
//! of the VpcLink… used in an Integration to reference this VpcLink"
//! (`smithy_query(query="operation", operation="CreateVpcLink")`, output member `id`),
//! while `name` is "the name used to label and identify the VPC link" that the model
//! constrains in no way — so two links may carry one name and are still two records.
//!
//! The table spans what the rule generalises over: which optional members a request
//! carries, how many targets it lists, the ORDER it lists them in, a target listed twice,
//! and requests that repeat an earlier request's name or its whole body. The sibling
//! `vpc_links` suite asserts the round trip of one link and the refusals, and
//! `vpc_link_provisioning` which state a link settles in; this suite is about one create
//! not disturbing another's record.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    VPC_LINK_AVAILABLE, VPC_LINK_PENDING,
};

use crate::rest_api_lifecycle::{string_member, without, Gateway};
use crate::vpc_links::{create_vpc_link, get_vpc_link, target_arn};

/// The members a stored link must report back exactly as the request that created it set
/// them. `status` and `statusMessage` are left out: those are the provisioning state the
/// clock owes, which the sibling suites own.
const REQUESTED_MEMBERS: [&str; 4] = ["name", "description", "targetArns", "tags"];

#[tokio::test]
async fn each_create_stores_its_own_link_and_leaves_the_stored_ones_alone() {
    // Rows differ along what the rule generalises over: the optional members carried, the
    // number of targets, the order they are listed in, a target repeated inside one list,
    // a name shared with an earlier row, and a body identical to an earlier row's.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one target and nothing optional",
            json!({ "name": "shared-label", "targetArns": [target_arn(0)] }),
        ),
        (
            "the same name, described and tagged",
            json!({
                "name": "shared-label",
                "description": "the second link under that label",
                "targetArns": [target_arn(1)],
                "tags": { "team": "api" },
            }),
        ),
        (
            "the same name again, two targets",
            json!({
                "name": "shared-label",
                "targetArns": [target_arn(2), target_arn(3)],
            }),
        ),
        (
            // The same two load balancers in the opposite order: `targetArns` is a list
            // (`ListOfString`, no `uniqueItems`), so the order is the request's.
            "those two targets in the opposite order",
            json!({
                "name": "reversed-targets",
                "targetArns": [target_arn(3), target_arn(2)],
            }),
        ),
        (
            "one target listed twice",
            json!({
                "name": "repeated-target",
                "targetArns": [target_arn(4), target_arn(4)],
                "tags": { "team": "api", "env": "test" },
            }),
        ),
        (
            "a body identical to the first row's",
            json!({ "name": "shared-label", "targetArns": [target_arn(0)] }),
        ),
    ];

    let gateway = Gateway::new();
    let mut stored: Vec<(&str, &Value, String)> = Vec::new();
    for (case, request) in &rows {
        let created = create_vpc_link(&gateway, request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected 202 from CreateVpcLink, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);

        // The id is MINTED, not derived from the request: a request repeating an earlier
        // one gets its own id, and no id is the name the caller chose.
        assert!(
            stored.iter().all(|(_, _, already)| already != &id),
            "case {case}: every create mints its own id, got {id:?} which is already \
             stored in {:?}",
            stored
                .iter()
                .map(|(_, _, already)| already.clone())
                .collect::<Vec<String>>()
        );
        let name = string_member(request, "name", case);
        assert!(
            !id.contains(&name),
            "case {case}: the id must not be built from the request, got {id:?} for \
             name {name:?}"
        );

        // The create's answer is the record it just stored, member for member.
        assert_requested_members(&created.body, request, case, "CreateVpcLink");
        let status = created.body["status"].as_str();
        assert!(
            status == Some(VPC_LINK_PENDING) || status == Some(VPC_LINK_AVAILABLE),
            "case {case}: CreateVpcLink must report a modelled provisioning status, \
             got {status:?}"
        );

        // Every link stored BEFORE this one still reads back its own members: the new
        // record was added beside them, not merged into them.
        for (stored_case, stored_request, stored_id) in &stored {
            let read = get_vpc_link(&gateway, stored_id).await;
            assert_eq!(
                read.status, 200,
                "case {stored_case}: expected 200 from GetVpcLink after the create for \
                 {case}, got {} with {:?}",
                read.status, read.body
            );
            assert_requested_members(
                &read.body,
                stored_request,
                stored_case,
                &format!("GetVpcLink after the create for {case}"),
            );
            assert_eq!(
                string_member(&read.body, "id", stored_case),
                *stored_id,
                "case {stored_case}: a link reports its own id, got {:?}",
                read.body
            );
        }
        stored.push((case, request, id));
    }

    // And the read of each link is the create's answer for THAT link, status aside: one
    // record per create, reported the same way by both operations.
    for (case, request, id) in &stored {
        let read = get_vpc_link(&gateway, id).await;
        let expected = {
            let mut body = (*request).clone();
            if let Some(members) = body.as_object_mut() {
                members.insert("id".to_string(), json!(id));
            }
            body
        };
        assert_eq!(
            without(&read.body, &["status", "statusMessage"]),
            expected,
            "case {case}: GetVpcLink must report the link this request stored, got {:?} \
             for {:?}",
            read.body,
            request
        );
    }
}

/// Assert `body` reports each of [`REQUESTED_MEMBERS`] exactly as `request` set it — an
/// absent member staying absent, a list keeping its order and its repeats, and a map
/// keeping its entries.
fn assert_requested_members(body: &Value, request: &Value, case: &str, whose: &str) {
    for member in REQUESTED_MEMBERS {
        assert_eq!(
            body[member], request[member],
            "case {case}: {whose} must report {member} {:?}, got {:?}",
            request[member], body[member]
        );
    }
}
