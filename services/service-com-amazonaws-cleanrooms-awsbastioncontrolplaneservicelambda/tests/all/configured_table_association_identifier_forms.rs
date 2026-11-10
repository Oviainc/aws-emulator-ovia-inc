// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two forms a `configuredTableAssociationIdentifier` takes.
//!
//! The rule under test: every operation that addresses a configured table association
//! under a membership resolves the identifier against the associations THAT membership
//! holds, by the association's own id or by the id of the configured table it associates
//! — the model documents both forms for that one member ("Currently accepts the
//! configured table association ID" on
//! `CreateConfiguredTableAssociationAnalysisRuleInput` and
//! `UpdateConfiguredTableAssociationInput`, "Currently accepts the configured table ID"
//! on `GetConfiguredTableAssociationInput` and `DeleteConfiguredTableAssociationInput`)
//! — and an identifier neither form names under that membership is the modelled 404
//! `ResourceNotFoundException`.
//!
//! The tables vary what the rule generalises over: both identifier forms, several
//! associations under one membership (so a form that resolves must pick the association
//! it names rather than the only one there is), every
//! `ConfiguredTableAssociationAnalysisRuleType` the model declares, the operation doing
//! the addressing (the association read, the analysis-rule create, the analysis-rule
//! read), and each form crossed with the other on the create/read round trip. Every
//! request goes over the wire through the same `MultiServiceRouter` the production
//! router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, error_code, minimal_collaboration, Emulator,
};
use super::cleanrooms_configured_table::{create_body, COLUMN_NAMES};

/// Every `ConfiguredTableAssociationAnalysisRuleType` the model declares (shape
/// `ConfiguredTableAssociationAnalysisRuleType`: AGGREGATION | LIST | CUSTOM).
const ALL_RULE_TYPES: [&str; 3] = ["AGGREGATION", "LIST", "CUSTOM"];

/// An identifier in the form the model's `ConfiguredTableAssociationIdentifier` pattern
/// declares (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no
/// create here mints.
const ABSENT_UUID: &str = "11111111-2222-3333-4444-555555555555";

/// An account id that is not the caller's, twelve digits as `AccountId` declares.
const OTHER_RESULT_RECEIVER: &str = "210987654321";

/// The union member of `ConfiguredTableAssociationAnalysisRulePolicyV1` that carries the
/// policy of `rule_type`.
fn policy_member(rule_type: &str) -> &'static str {
    match rule_type {
        "AGGREGATION" => "aggregation",
        "LIST" => "list",
        "CUSTOM" => "custom",
        other => panic!("no policy member for analysis rule type {other:?}"),
    }
}

/// A `ConfiguredTableAssociationAnalysisRulePolicy` of `rule_type`, carrying the one
/// optional member every one of the three policy shapes declares.
fn rule_policy(rule_type: &str) -> Value {
    json!({
        "v1": {
            policy_member(rule_type): { "allowedResultReceivers": [OTHER_RESULT_RECEIVER] }
        }
    })
}

/// One stored configured table association, with the configured table it associates.
struct Associated {
    /// The association's own id (`ConfiguredTableAssociation$id`).
    id: String,
    /// The association's ARN (`ConfiguredTableAssociation$arn`).
    arn: String,
    /// The id of the configured table the association refers to
    /// (`ConfiguredTableAssociation$configuredTableId`).
    table_id: String,
    /// The table alias the association was created under.
    name: String,
}

/// A collaboration, the caller's membership in it, and `count` configured tables each
/// associated once under that membership.
struct Fixture {
    membership_id: String,
    associations: Vec<Associated>,
}

async fn fixture(emulator: &Emulator, label: &str, count: usize) -> Fixture {
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration(label))
        .await;
    assert_eq!(
        status, 200,
        "setup {label:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {label:?}: no collaboration.id in {created}"))
        .to_string();

    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "setup {label:?}: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {label:?}: no membership.id in {joined}"))
        .to_string();

    let mut associations = Vec::new();
    for index in 0..count {
        let (status, table) = emulator
            .create_configured_table(create_body(
                &format!("{label}_table_{index}"),
                "GLUE",
                "DIRECT_QUERY",
                None,
                COLUMN_NAMES.len(),
            ))
            .await;
        assert_eq!(
            status, 200,
            "setup {label:?}: expected CreateConfiguredTable 200, got {status} with {table}"
        );
        let table_id = table["configuredTable"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("setup {label:?}: no configuredTable.id in {table}"))
            .to_string();

        let name = format!("sales_{index}");
        let (status, associated) = emulator
            .call(
                "POST",
                &format!("/memberships/{membership_id}/configuredTableAssociations"),
                Some(json!({
                    "name": name,
                    "configuredTableIdentifier": table_id,
                    "roleArn": format!("arn:aws:iam::{}:role/clean-rooms-table-role", account_id()),
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup {label:?}: expected CreateConfiguredTableAssociation 200, got {status} \
             with {associated}"
        );
        let association = &associated["configuredTableAssociation"];
        associations.push(Associated {
            id: association["id"]
                .as_str()
                .unwrap_or_else(|| panic!("setup {label:?}: no id in {associated}"))
                .to_string(),
            arn: association["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("setup {label:?}: no arn in {associated}"))
                .to_string(),
            table_id,
            name,
        });
    }

    Fixture {
        membership_id,
        associations,
    }
}

async fn read_configured_table_association(
    emulator: &Emulator,
    membership_id: &str,
    identifier: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/memberships/{membership_id}/configuredTableAssociations/{identifier}"),
            None,
        )
        .await
}

async fn create_rule(
    emulator: &Emulator,
    membership_id: &str,
    identifier: &str,
    rule_type: &str,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!(
                "/memberships/{membership_id}/configuredTableAssociations/{identifier}/analysisRule"
            ),
            Some(json!({
                "analysisRuleType": rule_type,
                "analysisRulePolicy": rule_policy(rule_type),
            })),
        )
        .await
}

async fn read_rule(
    emulator: &Emulator,
    membership_id: &str,
    identifier: &str,
    rule_type: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/memberships/{membership_id}/configuredTableAssociations/{identifier}/analysisRule/{rule_type}"
            ),
            None,
        )
        .await
}

/// The two forms the identifier takes for one stored association, as the model documents
/// them.
fn identifier_forms(association: &Associated) -> [(&'static str, &str); 2] {
    [
        ("the association's own id", association.id.as_str()),
        ("the configured table's id", association.table_id.as_str()),
    ]
}

#[tokio::test]
async fn either_identifier_form_addresses_the_association_it_names() {
    let emulator = Emulator::new();
    // Several associations under ONE membership, so a form that resolves has to pick the
    // association it names rather than the only one stored.
    let held = fixture(&emulator, "idforms-read", 3).await;

    for association in &held.associations {
        for (form, identifier) in identifier_forms(association) {
            let case = format!("{form} of {}", association.name);
            let (status, body) =
                read_configured_table_association(&emulator, &held.membership_id, identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetConfiguredTableAssociation 200, got {status} \
                 with {body}"
            );
            let reported = &body["configuredTableAssociation"];
            assert_eq!(
                reported["id"].as_str(),
                Some(association.id.as_str()),
                "case {case:?}: expected the association {}, got {}",
                association.id,
                reported["id"]
            );
            assert_eq!(
                reported["configuredTableId"].as_str(),
                Some(association.table_id.as_str()),
                "case {case:?}: expected the configured table {}, got {}",
                association.table_id,
                reported["configuredTableId"]
            );
            assert_eq!(
                reported["name"].as_str(),
                Some(association.name.as_str()),
                "case {case:?}: expected the alias {}, got {}",
                association.name,
                reported["name"]
            );
        }
    }
}

#[tokio::test]
async fn an_analysis_rule_round_trips_through_either_identifier_form() {
    for rule_type in ALL_RULE_TYPES {
        // Each form used for the create, crossed with each form used for the read.
        for create_form in 0..2 {
            for read_form in 0..2 {
                let emulator = Emulator::new();
                // Two associations, and the second is the one addressed, so neither form
                // can resolve by falling back to the first stored association.
                let held = fixture(
                    &emulator,
                    &format!("idforms-rt-{rule_type}-{create_form}{read_form}"),
                    2,
                )
                .await;
                let association = &held.associations[1];
                let forms = identifier_forms(association);
                let (created_by, create_identifier) = forms[create_form];
                let (read_by, read_identifier) = forms[read_form];
                let case =
                    format!("analysisRuleType={rule_type}, created by {created_by}, read by {read_by}");

                let (status, created) =
                    create_rule(&emulator, &held.membership_id, create_identifier, rule_type).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected CreateConfiguredTableAssociationAnalysisRule \
                     200, got {status} with {created}"
                );
                let rule = &created["analysisRule"];
                assert_eq!(
                    rule["membershipIdentifier"].as_str(),
                    Some(held.membership_id.as_str()),
                    "case {case:?}: expected the membership {}, got {}",
                    held.membership_id,
                    rule["membershipIdentifier"]
                );
                // Every member the service decides comes from the STORED association the
                // identifier resolved to, whichever form addressed it.
                assert_eq!(
                    rule["configuredTableAssociationId"].as_str(),
                    Some(association.id.as_str()),
                    "case {case:?}: expected the resolved association's id {}, got {}",
                    association.id,
                    rule["configuredTableAssociationId"]
                );
                assert_eq!(
                    rule["configuredTableAssociationArn"].as_str(),
                    Some(association.arn.as_str()),
                    "case {case:?}: expected the resolved association's ARN {}, got {}",
                    association.arn,
                    rule["configuredTableAssociationArn"]
                );
                assert_eq!(
                    rule["type"].as_str(),
                    Some(rule_type),
                    "case {case:?}: expected type {rule_type}, got {}",
                    rule["type"]
                );
                assert_eq!(
                    rule["policy"],
                    rule_policy(rule_type),
                    "case {case:?}: expected the policy that was sent, got {}",
                    rule["policy"]
                );

                let (status, read) =
                    read_rule(&emulator, &held.membership_id, read_identifier, rule_type).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected GetConfiguredTableAssociationAnalysisRule 200, \
                     got {status} with {read}"
                );
                assert_eq!(
                    read["analysisRule"], *rule,
                    "case {case:?}: the read must report the created rule {rule}, got {}",
                    read["analysisRule"]
                );

                // The rule landed on the association the identifier named, and on no
                // other association of the same membership.
                let (status, addressed) =
                    read_configured_table_association(&emulator, &held.membership_id, &association.id)
                        .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected GetConfiguredTableAssociation 200, got {status} \
                     with {addressed}"
                );
                assert_eq!(
                    addressed["configuredTableAssociation"]["analysisRuleTypes"],
                    json!([rule_type]),
                    "case {case:?}: expected the addressed association to hold [{rule_type}], \
                     got {}",
                    addressed["configuredTableAssociation"]["analysisRuleTypes"]
                );
                let sibling = &held.associations[0];
                let (status, untouched) =
                    read_configured_table_association(&emulator, &held.membership_id, &sibling.id)
                        .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected GetConfiguredTableAssociation 200 for the \
                     sibling, got {status} with {untouched}"
                );
                assert_eq!(
                    untouched["configuredTableAssociation"]["analysisRuleTypes"],
                    json!([]),
                    "case {case:?}: the sibling association must hold no rule, got {}",
                    untouched["configuredTableAssociation"]["analysisRuleTypes"]
                );
                let (status, sibling_rule) =
                    read_rule(&emulator, &held.membership_id, &sibling.id, rule_type).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: the sibling association holds no {rule_type} rule, so its \
                     read must be 404, got {status} with {sibling_rule}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_identifier_neither_form_names_under_the_membership_is_not_found() {
    let emulator = Emulator::new();
    let mine = fixture(&emulator, "idforms-notfound-mine", 1).await;
    let theirs = fixture(&emulator, "idforms-notfound-theirs", 1).await;
    // A configured table of this account that this membership never associated: it is a
    // stored configured table, so only the membership's own associations decide.
    let (status, unassociated) = emulator
        .create_configured_table(create_body(
            "idforms_notfound_unassociated",
            "GLUE",
            "DIRECT_QUERY",
            None,
            COLUMN_NAMES.len(),
        ))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateConfiguredTable 200, got {status} with {unassociated}"
    );
    let unassociated_table_id = unassociated["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no configuredTable.id in {unassociated}"))
        .to_string();

    let cases = [
        ("an id no create minted", ABSENT_UUID.to_string()),
        (
            "another membership's association id",
            theirs.associations[0].id.clone(),
        ),
        (
            "another membership's configured table id",
            theirs.associations[0].table_id.clone(),
        ),
        (
            "a configured table this membership never associated",
            unassociated_table_id,
        ),
        ("the membership's own id", mine.membership_id.clone()),
    ];

    for (case, identifier) in cases {
        let (status, body) =
            read_configured_table_association(&emulator, &mine.membership_id, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetConfiguredTableAssociation 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );

        let (status, body) = create_rule(&emulator, &mine.membership_id, &identifier, "LIST").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected CreateConfiguredTableAssociationAnalysisRule 404, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            body.get("analysisRule").is_none(),
            "case {case:?}: a refused create must report no analysis rule, got {body}"
        );

        let (status, body) = read_rule(&emulator, &mine.membership_id, &identifier, "LIST").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetConfiguredTableAssociationAnalysisRule 404, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The membership that DOES hold the association still resolves both of its forms, so
    // the refusals above are about the membership scope and not about the forms.
    for (form, identifier) in identifier_forms(&mine.associations[0]) {
        let (status, body) =
            read_configured_table_association(&emulator, &mine.membership_id, identifier).await;
        assert_eq!(
            status, 200,
            "case {form:?}: the holding membership must resolve its own association, got \
             {status} with {body}"
        );
    }
}
