// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms configured-table-association slice
//! (`CreateConfiguredTableAssociation` + `GetConfiguredTableAssociation` +
//! `ListConfiguredTableAssociations`).
//!
//! The rules under test:
//!
//! * `roleArn` is judged as a ROLE and not as a string: the service "will assume this
//!   role to access catalog metadata and query the table", so a value that does not name
//!   an IAM role of the CALLER'S OWN account — whatever its text looks like — is an
//!   authorization failure (`AccessDeniedException`, HTTP 403), never the boundary's
//!   `ValidationException` (HTTP 400). A value outside the modelled `@length` bound is
//!   still the 400 that bound declares, because that is a fault in the member's value;
//! * a create with a passable role PERSISTS the association, mints the members the
//!   service decides (a fresh `UUID` id, the `ConfiguredTableAssociationArn` nested in
//!   the membership, both timestamps, an empty `analysisRuleTypes`) and copies the
//!   configured table's and the membership's own identity onto it, and the reader and the
//!   listing report exactly that record;
//! * the table alias is "used to query the underlying configured table", so a second
//!   association of one membership under the same name is a `ConflictException` (409),
//!   while another name and another membership are accepted;
//! * an identifier the store does not hold — the membership or the configured table — is
//!   a `ResourceNotFoundException` (404).
//!
//! The tables vary what those rules generalise over: the shape of a non-passable role
//! value (no ARN at all, another account's role, another service's ARN, an IAM
//! non-role resource), the role-name characters the published form admits, the length
//! bound and one value past it, description present or absent, tags present or absent,
//! and how many associations one membership holds. Every request goes over the wire
//! through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, is_model_uuid,
    minimal_collaboration, Emulator, REGION,
};
use super::cleanrooms_configured_table::create_body;

/// The message AWS reports when the `roleArn` is not a role the caller can pass to the
/// service. Fixed text: the refusal names no value of the request.
const PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";

/// The published bounds of `RoleArn` (model shape `RoleArn`, len 32..512).
const ROLE_ARN_MIN_LEN: usize = 32;
const ROLE_ARN_MAX_LEN: usize = 512;

/// A membership id in the form the model's `MembershipIdentifier` pattern declares that
/// no create in these tests mints.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

impl Emulator {
    async fn create_table_association(&self, membership_id: &str, body: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/memberships/{membership_id}/configuredTableAssociations"),
            Some(body),
        )
        .await
    }

    async fn get_table_association(&self, membership_id: &str, id: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/memberships/{membership_id}/configuredTableAssociations/{id}"),
            None,
        )
        .await
    }

    async fn list_table_associations(&self, membership_id: &str, query: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/memberships/{membership_id}/configuredTableAssociations{query}"),
            None,
        )
        .await
    }
}

/// An IAM role ARN of `account`, in the form the model's `RoleArn` pattern declares
/// (`^arn:aws:iam::[\w]+:role/[\w+=./@-]+$`).
fn role_arn(account: &str, name: &str) -> String {
    format!("arn:aws:iam::{account}:role/{name}")
}

/// A collaboration, the caller's membership in it and a configured table to associate:
/// `(membership id, configured table id)`.
async fn membership_and_table(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration(name))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no collaboration.id in {created}"))
        .to_string();

    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no membership.id in {joined}"))
        .to_string();

    let (status, table) = emulator
        .create_configured_table(create_body(
            &format!("{name}_table"),
            "GLUE",
            "DIRECT_QUERY",
            None,
            2,
        ))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateConfiguredTable 200, got {status} with {table}"
    );
    let table_id = table["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no configuredTable.id in {table}"))
        .to_string();

    (membership_id, table_id)
}

/// One row of the pass-role table: a `roleArn` value and whether the service can pass it.
struct RoleCase {
    label: &'static str,
    role_arn: String,
}

/// Values that name no role of the caller's own account. Each is at least
/// [`ROLE_ARN_MIN_LEN`] characters, so the modelled `@length` is satisfied and the
/// pass-role decision is the only thing that can refuse them.
fn unpassable_roles() -> Vec<RoleCase> {
    vec![
        RoleCase {
            label: "not an ARN at all",
            role_arn: "not-an-arn-but-long-enough-to-pass-the-length-bound-1234567890".to_string(),
        },
        RoleCase {
            label: "an IAM role of another account",
            role_arn: role_arn("210987654321", "clean-rooms-table-role"),
        },
        RoleCase {
            label: "another service's ARN in this account",
            role_arn: format!(
                "arn:aws:lambda:{REGION}:{}:function:clean-rooms-table-role",
                account_id()
            ),
        },
        RoleCase {
            label: "an IAM resource of this account that is not a role",
            role_arn: format!("arn:aws:iam::{}:user/clean-rooms-table-user", account_id()),
        },
        RoleCase {
            label: "an IAM role ARN carrying a region segment",
            role_arn: format!(
                "arn:aws:iam:{REGION}:{}:role/clean-rooms-table-role",
                account_id()
            ),
        },
        RoleCase {
            label: "an IAM role ARN whose account segment is not an account",
            role_arn: role_arn("not-an-account-id", "clean-rooms-table-role"),
        },
    ]
}

/// Values that DO name a role of the caller's own account, spanning the characters the
/// published role-name form admits (`[\w+=./@-]+`).
fn passable_roles() -> Vec<RoleCase> {
    vec![
        RoleCase {
            label: "a plain role name",
            role_arn: role_arn(account_id(), "clean-rooms-table-role"),
        },
        RoleCase {
            label: "a role under a path",
            role_arn: role_arn(account_id(), "service-role/clean_rooms/table.role"),
        },
        RoleCase {
            label: "a role name using the whole admitted character set",
            role_arn: role_arn(account_id(), "role+name=with.every@admitted-char_1"),
        },
    ]
}

#[tokio::test]
async fn a_role_the_service_cannot_pass_is_an_authorization_failure() {
    for (row, case) in unpassable_roles().into_iter().enumerate() {
        let label = case.label;
        let emulator = Emulator::new();
        let (membership_id, table_id) =
            membership_and_table(&emulator, &format!("passrole{row}")).await;

        assert!(
            case.role_arn.len() >= ROLE_ARN_MIN_LEN,
            "case {label:?}: the row must satisfy the modelled length bound, got {} characters",
            case.role_arn.len()
        );

        let (status, body) = emulator
            .create_table_association(
                &membership_id,
                json!({
                    "name": "sales",
                    "configuredTableIdentifier": table_id,
                    "roleArn": case.role_arn,
                }),
            )
            .await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected 403 for a role the service cannot pass, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {label:?}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            PASS_ROLE_DENIED,
            "case {label:?}: expected the pass-role message, got {:?}",
            error_message(&body)
        );

        // The refusal created nothing: the listing is still empty.
        let (status, list) = emulator.list_table_associations(&membership_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListConfiguredTableAssociations 200, got {status} \
             with {list}"
        );
        assert_eq!(
            list["configuredTableAssociationSummaries"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {label:?}: a refused create must store nothing, got {list}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_the_modelled_length_bound_is_still_input_validation() {
    // The pattern is not the refusal path, but the LENGTH bound still is: a value outside
    // the published bound is a fault in the member's value, so it keeps its 400 — which is
    // what distinguishes "fix the field" from "the role cannot be passed". Both published
    // bounds are pinned, and so is each one's inside edge, so the bound the handler
    // enforces cannot drift from the model in either direction unnoticed.
    let emulator = Emulator::new();
    let (membership_id, table_id) = membership_and_table(&emulator, "shortrole").await;

    // A role of this account padded to exactly `len` characters, so the row tests the
    // length and nothing else.
    let padded_role = |len: usize| {
        let prefix = role_arn(account_id(), "");
        format!("{prefix}{}", "a".repeat(len - prefix.len()))
    };

    let refused = vec![
        (
            "one character under the minimum",
            "a".repeat(ROLE_ARN_MIN_LEN - 1),
        ),
        ("a single character", "x".to_string()),
        (
            // 31 characters: the published form with no role name after `role/`.
            "a role ARN of this account with no role name",
            role_arn(account_id(), ""),
        ),
        (
            "one character over the maximum",
            padded_role(ROLE_ARN_MAX_LEN + 1),
        ),
    ];

    for (label, value) in refused {
        assert!(
            value.len() < ROLE_ARN_MIN_LEN || value.len() > ROLE_ARN_MAX_LEN,
            "case {label:?}: the row must fall outside the published bound, got {} characters",
            value.len()
        );
        let (status, body) = emulator
            .create_table_association(
                &membership_id,
                json!({
                    "name": "sales",
                    "configuredTableIdentifier": table_id,
                    "roleArn": value,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for a value outside the length bound, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert!(
            error_message(&body).contains("roleArn"),
            "case {label:?}: the message must name the member it refused, got {:?}",
            error_message(&body)
        );
    }

    // And each bound's inside edge is accepted: a passable role of exactly the minimum
    // and of exactly the maximum length is judged as a role, not refused on its length.
    for (label, len) in [
        ("exactly the minimum", ROLE_ARN_MIN_LEN),
        ("exactly the maximum", ROLE_ARN_MAX_LEN),
    ] {
        let value = padded_role(len);
        assert_eq!(
            value.len(),
            len,
            "case {label:?}: the row must be exactly {len} characters, got {}",
            value.len()
        );
        let (status, body) = emulator
            .create_table_association(
                &membership_id,
                json!({
                    "name": format!("edge_{len}"),
                    "configuredTableIdentifier": table_id,
                    "roleArn": value,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a passable role at the bound must be accepted, got {status} \
             with {body}"
        );
    }
}

/// One row of the create/read round-trip table.
struct AssociationCase {
    label: String,
    name: &'static str,
    description: Option<&'static str>,
    role_arn: String,
    tags: Option<Value>,
}

fn round_trip_cases() -> Vec<AssociationCase> {
    let names = ["sales", "Orders_2026", "customer events"];
    passable_roles()
        .into_iter()
        .enumerate()
        .map(|(i, role)| AssociationCase {
            label: format!("{} with name {:?}", role.label, names[i % names.len()]),
            name: names[i % names.len()],
            description: if i % 2 == 0 {
                Some("the association under test")
            } else {
                None
            },
            role_arn: role.role_arn,
            tags: if i % 2 == 0 {
                None
            } else {
                Some(json!({ "team": "data" }))
            },
        })
        .collect()
}

#[tokio::test]
async fn a_created_association_is_persisted_and_read_back() {
    for (row, case) in round_trip_cases().into_iter().enumerate() {
        let label = &case.label;
        let emulator = Emulator::new();
        let (membership_id, table_id) =
            membership_and_table(&emulator, &format!("roundtrip{row}")).await;

        let mut body = json!({
            "name": case.name,
            "configuredTableIdentifier": table_id,
            "roleArn": case.role_arn,
        });
        if let Some(description) = case.description {
            body["description"] = json!(description);
        }
        if let Some(tags) = case.tags.clone() {
            body["tags"] = tags;
        }

        let (status, created) = emulator.create_table_association(&membership_id, body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTableAssociation 200, got {status} \
             with {created}"
        );
        let association = created["configuredTableAssociation"].clone();
        let id = association["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no id in {association}"))
            .to_string();

        assert!(
            is_model_uuid(&id),
            "case {label:?}: the id must be a model UUID, got {id:?}"
        );
        assert_eq!(
            association["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:cleanrooms:{REGION}:{}:configuredtableassociation/{membership_id}/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {label:?}: the ARN must nest the association in its membership, got {:?}",
            association["arn"]
        );
        assert_eq!(
            association["configuredTableId"].as_str(),
            Some(table_id.as_str()),
            "case {label:?}: expected configuredTableId {table_id}, got {:?}",
            association["configuredTableId"]
        );
        assert_eq!(
            association["membershipId"].as_str(),
            Some(membership_id.as_str()),
            "case {label:?}: expected membershipId {membership_id}, got {:?}",
            association["membershipId"]
        );
        assert_eq!(
            association["roleArn"].as_str(),
            Some(case.role_arn.as_str()),
            "case {label:?}: expected roleArn {:?}, got {:?}",
            case.role_arn,
            association["roleArn"]
        );
        assert_eq!(
            association["name"].as_str(),
            Some(case.name),
            "case {label:?}: expected name {:?}, got {:?}",
            case.name,
            association["name"]
        );
        assert_eq!(
            association["description"].as_str(),
            case.description,
            "case {label:?}: expected description {:?}, got {:?}",
            case.description,
            association["description"]
        );
        assert_eq!(
            association["analysisRuleTypes"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: an association is created with no analysis rules, got {:?}",
            association["analysisRuleTypes"]
        );
        assert_eq!(
            association["createTime"], association["updateTime"],
            "case {label:?}: a create stamps both timestamps at one instant, got {:?} and {:?}",
            association["createTime"], association["updateTime"]
        );

        // The reader reports exactly the persisted record.
        let (status, fetched) = emulator.get_table_association(&membership_id, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTableAssociation 200, got {status} \
             with {fetched}"
        );
        assert_eq!(
            fetched["configuredTableAssociation"], association,
            "case {label:?}: the read must report the created association, got {:?}",
            fetched["configuredTableAssociation"]
        );

        // And so does the listing, through its own summary projection.
        let (status, list) = emulator.list_table_associations(&membership_id, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListConfiguredTableAssociations 200, got {status} \
             with {list}"
        );
        let summaries = list["configuredTableAssociationSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: no summaries in {list}"));
        assert_eq!(
            summaries.len(),
            1,
            "case {label:?}: expected one summary, got {summaries:?}"
        );
        for member in [
            "id",
            "arn",
            "configuredTableId",
            "membershipId",
            "membershipArn",
            "name",
            "createTime",
            "updateTime",
            "analysisRuleTypes",
        ] {
            assert_eq!(
                summaries[0][member], association[member],
                "case {label:?}: the summary's {member} must be the stored one, got {:?}",
                summaries[0][member]
            );
        }
    }
}

#[tokio::test]
async fn one_membership_holds_one_association_per_table_alias() {
    let emulator = Emulator::new();
    let (membership_id, table_id) = membership_and_table(&emulator, "alias").await;
    let role = role_arn(account_id(), "clean-rooms-table-role");

    let create = |name: &'static str, table: String, role: String| {
        json!({
            "name": name,
            "configuredTableIdentifier": table,
            "roleArn": role,
        })
    };

    let (status, created) = emulator
        .create_table_association(&membership_id, create("sales", table_id.clone(), role.clone()))
        .await;
    assert_eq!(status, 200, "the first create must succeed, got {created}");
    let first_id = created["configuredTableAssociation"]["id"]
        .as_str()
        .expect("association id")
        .to_string();

    // Rows: the same alias again (a conflict, whatever else the request says), and a
    // different alias over the same configured table (accepted — the alias is the key).
    let (status, refused) = emulator
        .create_table_association(
            &membership_id,
            create("sales", table_id.clone(), role_arn(account_id(), "other-role")),
        )
        .await;
    assert_eq!(
        status, 409,
        "case the same alias: expected 409, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case the same alias: expected ConflictException, got {refused}"
    );
    assert_eq!(
        refused["reason"].as_str(),
        Some("ALREADY_EXISTS"),
        "case the same alias: expected reason ALREADY_EXISTS, got {:?}",
        refused["reason"]
    );
    assert_eq!(
        refused["resourceId"].as_str(),
        Some(first_id.as_str()),
        "case the same alias: the conflict must name the existing association, got {:?}",
        refused["resourceId"]
    );

    let (status, second) = emulator
        .create_table_association(
            &membership_id,
            create("other_alias", table_id.clone(), role.clone()),
        )
        .await;
    assert_eq!(
        status, 200,
        "case another alias: a second alias over one table must be accepted, got {status} \
         with {second}"
    );

    // Another membership may use the alias the first one took: the key is per membership.
    let (other_membership_id, other_table_id) = membership_and_table(&emulator, "alias-other").await;
    let (status, elsewhere) = emulator
        .create_table_association(
            &other_membership_id,
            create("sales", other_table_id, role.clone()),
        )
        .await;
    assert_eq!(
        status, 200,
        "case another membership: the same alias elsewhere must be accepted, got {status} \
         with {elsewhere}"
    );

    // Each listing reports only its own membership's associations, in creation order.
    let (_, list) = emulator.list_table_associations(&membership_id, "").await;
    let names: Vec<&str> = list["configuredTableAssociationSummaries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|summary| summary["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec!["sales", "other_alias"],
        "the listing must report this membership's associations in creation order, got {names:?}"
    );

    // And paging it one row at a time walks the same order.
    let (_, page) = emulator
        .list_table_associations(&membership_id, "?maxResults=1")
        .await;
    assert_eq!(
        page["configuredTableAssociationSummaries"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "a page of one must carry one row, got {page}"
    );
    let token = page["nextToken"].as_str().expect("a token for the next page");
    let (_, next) = emulator
        .list_table_associations(&membership_id, &format!("?maxResults=1&nextToken={token}"))
        .await;
    assert_eq!(
        next["configuredTableAssociationSummaries"][0]["name"].as_str(),
        Some("other_alias"),
        "the second page must resume at the second row, got {next}"
    );
}

#[tokio::test]
async fn an_association_does_not_outlive_what_it_links() {
    // An association is a sub-resource of ONE membership and links it to ONE
    // collaboration, so it is unreachable once either is gone and goes with it rather
    // than staying behind, counting against the store's cap and holding its tags alive.
    // Rows: the collaboration is deleted (the membership itself survives), and a
    // participant leaves (the collaboration survives).
    for (label, delete_the_collaboration) in [
        ("the collaboration is deleted", true),
        ("the member leaves", false),
    ] {
        let emulator = Emulator::new();
        let collaboration_id = collaboration_owned_elsewhere(
            &emulator,
            &format!("outlive-{}", u8::from(delete_the_collaboration)),
            Some(&["CAN_QUERY"]),
        )
        .await;
        if delete_the_collaboration {
            // Re-own it so the caller may delete it: "it can only be called by the
            // collaboration owner".
            let store = emulator.store();
            let mut store = store.lock();
            let record = store
                .collaborations
                .get_mut(&collaboration_id)
                .expect("the collaboration just created");
            record.creator_account_id = account_id().to_string();
        }

        let (status, joined) = emulator
            .create_membership(json!({
                "collaborationIdentifier": collaboration_id,
                "queryLogStatus": "ENABLED",
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateMembership failed with {status}: {joined}"
        );
        let membership_id = joined["membership"]["id"].as_str().unwrap().to_string();

        let (status, table) = emulator
            .create_configured_table(create_body("outlive_table", "GLUE", "DIRECT_QUERY", None, 2))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateConfiguredTable failed with {status}: {table}"
        );
        let table_id = table["configuredTable"]["id"].as_str().unwrap().to_string();

        let (status, created) = emulator
            .create_table_association(
                &membership_id,
                json!({
                    "name": "sales",
                    "configuredTableIdentifier": table_id,
                    "roleArn": role_arn(account_id(), "clean-rooms-table-role"),
                    "tags": { "team": "data" },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateConfiguredTableAssociation failed with {status}: {created}"
        );
        let association_arn = created["configuredTableAssociation"]["arn"]
            .as_str()
            .expect("association arn")
            .to_string();

        if delete_the_collaboration {
            let (status, body) = emulator.delete_collaboration(&collaboration_id).await;
            assert_eq!(
                status, 204,
                "case {label:?}: expected DeleteCollaboration 204, got {status} with {body}"
            );
        } else {
            let (status, body) = emulator.delete_membership(&membership_id).await;
            assert_eq!(
                status, 204,
                "case {label:?}: expected DeleteMembership 204, got {status} with {body}"
            );
        }

        let store = emulator.store();
        let store = store.lock();
        assert!(
            store.configured_table_associations.is_empty()
                && store.configured_table_association_order.is_empty()
                && store.configured_table_association_by_name.is_empty(),
            "case {label:?}: the association must not outlive what it links, {} left",
            store.configured_table_associations.len()
        );
        assert!(
            !store.tags.contains_key(&association_arn),
            "case {label:?}: the association's tags must go with it, got {:?}",
            store.tags.get(&association_arn)
        );
    }
}

#[tokio::test]
async fn an_identifier_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let (membership_id, table_id) = membership_and_table(&emulator, "notfound").await;
    let role = role_arn(account_id(), "clean-rooms-table-role");

    // Rows: the membership is absent, the configured table is absent, and the
    // association read under a membership that holds no such association.
    let (status, body) = emulator
        .create_table_association(
            ABSENT_UUID,
            json!({
                "name": "sales",
                "configuredTableIdentifier": table_id,
                "roleArn": role,
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case absent membership: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case absent membership: expected ResourceNotFoundException, got {body}"
    );

    let (status, body) = emulator
        .create_table_association(
            &membership_id,
            json!({
                "name": "sales",
                "configuredTableIdentifier": ABSENT_UUID,
                "roleArn": role,
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case absent configured table: expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some("CONFIGURED_TABLE"),
        "case absent configured table: the refusal must name the kind it looked for, got {body}"
    );

    let (status, body) = emulator
        .get_table_association(&membership_id, ABSENT_UUID)
        .await;
    assert_eq!(
        status, 404,
        "case absent association: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case absent association: expected ResourceNotFoundException, got {body}"
    );
}
