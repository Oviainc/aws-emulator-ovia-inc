// SPDX-License-Identifier: CC0-1.0

//! A Clean Rooms not-found refusal is about the resource the REQUEST ADDRESSED: it names
//! that resource (and the parent the path scoped it to) in the service's own words, and
//! reports that resource in `ResourceNotFoundException`'s `resourceId` / `resourceType` —
//! never a different resource that happens to be nearby in the store.
//!
//! Two slices are under test, each asserted over a TABLE whose rows differ along what the
//! rule generalises over rather than re-running one recorded request:
//!
//! 1. `GetConfiguredTableAssociationAnalysisRule` — a rule is addressed by the
//!    (configured table association, analysis rule type) pair under the membership the
//!    association belongs to. An association holding no rule of the requested type is a
//!    404 naming the association and the type BY ITS INTERNAL NAME (the name the
//!    service's rule classes carry, which is not the modelled catalog value), while the
//!    rules the association does hold read back whole, with the modelled catalog value as
//!    their `type`. The rows span every rule type, an association that holds a subset of
//!    them, an association reached through the wrong membership, a membership the store
//!    does not hold, and an association whose membership has been deleted.
//! 2. `GetPrivacyBudgetTemplate` / `DeletePrivacyBudgetTemplate` — a template is addressed
//!    by its id through ONE membership, so a refusal names that pair and reports the
//!    TEMPLATE as the missing resource. The rows span ids nothing ever minted, an id that
//!    was minted and then deleted, and a template that exists but belongs to another
//!    collaboration, so it is not reachable through this membership.
//!
//! Every request goes over the wire through the same `MultiServiceRouter` the production
//! router uses; the store is read directly only once, to show that an association's
//! analysis rules do not outlive the association.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, minimal_collaboration,
    Emulator, ALL_LOG_STATUSES, ALL_MEMBER_ABILITIES,
};
use super::cleanrooms_configured_table::create_body;

/// Every `ConfiguredTableAssociationAnalysisRuleType` the model declares (shape
/// `ConfiguredTableAssociationAnalysisRuleType`: AGGREGATION | LIST | CUSTOM), paired with
/// the INTERNAL name the service reports that type by when it refuses a read: the catalog
/// value as one CamelCase word, except LIST, whose rule class is the intersection-list
/// rule and is named for it.
const RULE_TYPES: [(&str, &str); 3] = [
    ("AGGREGATION", "Aggregation"),
    ("LIST", "IntersectionList"),
    ("CUSTOM", "Custom"),
];

/// Identifiers in the model's `UUID` form that no create in this suite mints.
const NEVER_MINTED_IDS: [&str; 3] = [
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "0123456f-89ab-cdef-0123-456789abcdef",
];

/// The resource type a privacy-budget-template 404 reports: the kind of resource that was
/// not found. The model's `ResourceType` catalog (CONFIGURED_TABLE | COLLABORATION |
/// MEMBERSHIP | CONFIGURED_TABLE_ASSOCIATION) names no privacy budget template, so the
/// required member reports the kind rather than one of the four that is not it.
const PRIVACY_BUDGET_TEMPLATE_TYPE: &str = "PRIVACY_BUDGET_TEMPLATE";

impl Emulator {
    async fn read_association_rule(
        &self,
        membership_id: &str,
        association_id: &str,
        rule_type: &str,
    ) -> (u16, Value) {
        self.call(
            "GET",
            &format!(
                "/memberships/{membership_id}/configuredTableAssociations/{association_id}\
                 /analysisRule/{rule_type}"
            ),
            None,
        )
        .await
    }

    async fn create_association_rule(
        &self,
        membership_id: &str,
        association_id: &str,
        rule_type: &str,
    ) -> (u16, Value) {
        // Every member of the three policy shapes is optional; one receiver is enough to
        // show the stored policy comes back unchanged.
        let policy = json!({
            "v1": {
                association_policy_member(rule_type): {
                    "allowedResultReceivers": [account_id()],
                }
            }
        });
        self.call(
            "POST",
            &format!(
                "/memberships/{membership_id}/configuredTableAssociations/{association_id}\
                 /analysisRule"
            ),
            Some(json!({ "analysisRuleType": rule_type, "analysisRulePolicy": policy })),
        )
        .await
    }

    async fn read_table_association(&self, membership_id: &str, association_id: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/memberships/{membership_id}/configuredTableAssociations/{association_id}"),
            None,
        )
        .await
    }

    async fn create_privacy_budget_template(
        &self,
        membership_id: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/memberships/{membership_id}/privacybudgettemplates"),
            Some(body),
        )
        .await
    }

    async fn get_privacy_budget_template(
        &self,
        membership_id: &str,
        template_id: &str,
    ) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/memberships/{membership_id}/privacybudgettemplates/{template_id}"),
            None,
        )
        .await
    }

    async fn delete_privacy_budget_template(
        &self,
        membership_id: &str,
        template_id: &str,
    ) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/memberships/{membership_id}/privacybudgettemplates/{template_id}"),
            None,
        )
        .await
    }
}

/// A membership of the caller's plus a configured table association under it:
/// `(membership id, association id, association ARN)`.
///
/// `owned_elsewhere` makes the collaboration another account's, which is the only way the
/// caller's membership in it is one the caller may delete (a creator's membership is
/// removed only with the collaboration).
async fn membership_with_association(
    emulator: &Emulator,
    name: &str,
    owned_elsewhere: bool,
) -> (String, String, String) {
    let collaboration_id = if owned_elsewhere {
        collaboration_owned_elsewhere(emulator, name, Some(&[ALL_MEMBER_ABILITIES[0]])).await
    } else {
        let (status, created) = emulator
            .create_collaboration(minimal_collaboration(name))
            .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected CreateCollaboration 200, got {status} with {created}"
        );
        created["collaboration"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("setup {name:?}: no collaboration.id in {created}"))
            .to_string()
    };

    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": ALL_LOG_STATUSES[0],
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

    let (status, association) = emulator
        .call(
            "POST",
            &format!("/memberships/{membership_id}/configuredTableAssociations"),
            Some(json!({
                "name": format!("{name}_alias"),
                "configuredTableIdentifier": table_id,
                "roleArn": format!("arn:aws:iam::{}:role/clean-rooms-table-role", account_id()),
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateConfiguredTableAssociation 200, got {status} with \
         {association}"
    );
    let association_id = association["configuredTableAssociation"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {name:?}: no configuredTableAssociation.id in {association}"))
        .to_string();
    let association_arn = association["configuredTableAssociation"]["arn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("setup {name:?}: no configuredTableAssociation.arn in {association}")
        })
        .to_string();

    (membership_id, association_id, association_arn)
}

/// The union member of `ConfiguredTableAssociationAnalysisRulePolicyV1` that carries the
/// policy of `rule_type`: each member is the policy of exactly one type.
fn association_policy_member(rule_type: &str) -> &'static str {
    match rule_type {
        "AGGREGATION" => "aggregation",
        "LIST" => "list",
        "CUSTOM" => "custom",
        other => panic!("no policy member for analysis rule type {other:?}"),
    }
}

/// The refusal a rule-less (association, type) pair answers with.
fn expected_rule_absent_message(association_id: &str, internal_type: &str) -> String {
    format!("No such analysis rule with configuredTableAssociationId: {association_id} and type {internal_type}")
}

#[tokio::test]
async fn an_association_analysis_rule_read_is_keyed_on_the_association_and_the_type() {
    let emulator = Emulator::new();
    let (membership_id, association_id, association_arn) =
        membership_with_association(&emulator, "assoc-rule-reads", false).await;

    // Before any rule exists, every type is absent, and the refusal names the association
    // the request addressed and the type by its INTERNAL name.
    for (rule_type, internal) in RULE_TYPES {
        let (status, body) = emulator
            .read_association_rule(&membership_id, &association_id, rule_type)
            .await;
        assert_eq!(
            status, 404,
            "case {rule_type:?} before any create: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {rule_type:?} before any create: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_rule_absent_message(&association_id, internal),
            "case {rule_type:?} before any create: the message must name the association and \
             the internal type name, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(association_id.as_str()),
            "case {rule_type:?} before any create: expected resourceId {association_id:?}, got \
             {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("CONFIGURED_TABLE_ASSOCIATION"),
            "case {rule_type:?} before any create: expected resourceType \
             CONFIGURED_TABLE_ASSOCIATION, got {body}"
        );
    }

    // A SUBSET of the types is created, so the read is keyed on the pair and not on the
    // association alone: the created ones read back whole, the remaining one stays absent.
    let held = [RULE_TYPES[0].0, RULE_TYPES[1].0];
    for rule_type in held {
        let (status, created) = emulator
            .create_association_rule(&membership_id, &association_id, rule_type)
            .await;
        assert_eq!(
            status, 200,
            "case {rule_type:?}: expected CreateConfiguredTableAssociationAnalysisRule 200, got \
             {status} with {created}"
        );
    }

    for (rule_type, internal) in RULE_TYPES {
        let (status, body) = emulator
            .read_association_rule(&membership_id, &association_id, rule_type)
            .await;
        if held.contains(&rule_type) {
            assert_eq!(
                status, 200,
                "case {rule_type:?} once stored: expected 200, got {status} with {body}"
            );
            // The answer reports the MODELLED catalog value, not the internal name the
            // refusal uses.
            assert_eq!(
                body["analysisRule"]["type"].as_str(),
                Some(rule_type),
                "case {rule_type:?} once stored: expected type {rule_type:?}, got {body}"
            );
            assert_eq!(
                body["analysisRule"]["membershipIdentifier"].as_str(),
                Some(membership_id.as_str()),
                "case {rule_type:?} once stored: expected membershipIdentifier \
                 {membership_id:?}, got {body}"
            );
            assert_eq!(
                body["analysisRule"]["configuredTableAssociationId"].as_str(),
                Some(association_id.as_str()),
                "case {rule_type:?} once stored: expected configuredTableAssociationId \
                 {association_id:?}, got {body}"
            );
            assert_eq!(
                body["analysisRule"]["configuredTableAssociationArn"].as_str(),
                Some(association_arn.as_str()),
                "case {rule_type:?} once stored: expected configuredTableAssociationArn \
                 {association_arn:?}, got {body}"
            );
            assert_eq!(
                body["analysisRule"]["policy"]["v1"][association_policy_member(rule_type)]
                    ["allowedResultReceivers"][0]
                    .as_str(),
                Some(account_id()),
                "case {rule_type:?} once stored: expected the stored policy, got {body}"
            );
            // A read changes nothing: the second read answers the same.
            let (status, again) = emulator
                .read_association_rule(&membership_id, &association_id, rule_type)
                .await;
            assert_eq!(
                (status, &again),
                (200, &body),
                "case {rule_type:?} once stored: a second read must answer the same, got \
                 {status} with {again}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {rule_type:?} not stored: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                expected_rule_absent_message(&association_id, internal),
                "case {rule_type:?} not stored: the message must name the association and the \
                 internal type name, got {:?}",
                error_message(&body)
            );
        }
    }

    // The types the association holds a rule for are reported on the association itself.
    let (status, association) = emulator
        .read_table_association(&membership_id, &association_id)
        .await;
    assert_eq!(
        status, 200,
        "expected GetConfiguredTableAssociation 200, got {status} with {association}"
    );
    assert_eq!(
        association["configuredTableAssociation"]["analysisRuleTypes"],
        json!(held),
        "expected analysisRuleTypes {held:?}, got {}",
        association["configuredTableAssociation"]["analysisRuleTypes"]
    );

    // A membership the store does not hold reaches no association at all, and the refusal
    // is about the membership rather than about the rule.
    for absent in NEVER_MINTED_IDS {
        let (status, body) = emulator
            .read_association_rule(absent, &association_id, RULE_TYPES[1].0)
            .await;
        assert_eq!(
            status, 404,
            "case membership {absent:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("No such membership: {absent} for account {}", account_id()),
            "case membership {absent:?}: the refusal must be about the membership, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("MEMBERSHIP"),
            "case membership {absent:?}: expected resourceType MEMBERSHIP, got {body}"
        );
    }

    // An association of ANOTHER membership is not visible through this one, rules and all:
    // the pair resolves to nothing, which is the same 404 as a pair holding no rule.
    let (other_membership, other_association, _) =
        membership_with_association(&emulator, "assoc-rule-other", false).await;
    let (status, created) = emulator
        .create_association_rule(&other_membership, &other_association, RULE_TYPES[1].0)
        .await;
    assert_eq!(
        status, 200,
        "setup: the other membership's rule must be created, got {status} with {created}"
    );
    for (membership, association, label) in [
        (&membership_id, &other_association, "other association here"),
        (&other_membership, &association_id, "this association there"),
    ] {
        let (status, body) = emulator
            .read_association_rule(membership, association, RULE_TYPES[1].0)
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_rule_absent_message(association, RULE_TYPES[1].1),
            "case {label:?}: the refusal must name the pair that resolved to nothing, got {:?}",
            error_message(&body)
        );
    }

    // An association's rules go with the association: deleting the membership removes
    // both, so the pair resolves to nothing afterwards.
    let (deletable_membership, deletable_association, _) =
        membership_with_association(&emulator, "assoc-rule-cascade", true).await;
    let (status, created) = emulator
        .create_association_rule(
            &deletable_membership,
            &deletable_association,
            RULE_TYPES[2].0,
        )
        .await;
    assert_eq!(
        status, 200,
        "case cascade: expected the rule to be created, got {status} with {created}"
    );
    let (status, body) = emulator
        .read_association_rule(
            &deletable_membership,
            &deletable_association,
            RULE_TYPES[2].0,
        )
        .await;
    assert_eq!(
        status, 200,
        "case cascade: the stored rule must read back before the delete, got {status} with {body}"
    );
    let (status, body) = emulator.delete_membership(&deletable_membership).await;
    assert_eq!(
        status, 204,
        "case cascade: expected DeleteMembership 204, got {status} with {body}"
    );
    assert!(
        emulator
            .store()
            .lock()
            .configured_table_association_analysis_rule(&deletable_association, RULE_TYPES[2].0)
            .is_none(),
        "case cascade: the deleted association must hold no {} analysis rule",
        RULE_TYPES[2].0
    );
    let (status, body) = emulator
        .read_association_rule(
            &deletable_membership,
            &deletable_association,
            RULE_TYPES[2].0,
        )
        .await;
    assert_eq!(
        status, 404,
        "case cascade: expected 404 after the delete, got {status} with {body}"
    );
}

/// A `CreatePrivacyBudgetTemplate` body of the differential-privacy type, with parameters
/// inside the published bounds (`Epsilon` 1..20, `UsersNoisePerQuery` 10..100).
fn differential_privacy_template() -> Value {
    json!({
        "privacyBudgetType": "DIFFERENTIAL_PRIVACY",
        "parameters": { "differentialPrivacy": { "epsilon": 5, "usersNoisePerQuery": 20 } },
    })
}

#[tokio::test]
async fn a_privacy_budget_template_refusal_names_the_template_and_the_membership() {
    let emulator = Emulator::new();

    // Two memberships of two collaborations, each with its own template: the second one is
    // what makes "a template that exists but is not reachable here" a row.
    let mut memberships: Vec<(String, String)> = Vec::new();
    for name in ["budget-here", "budget-there"] {
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
                "queryLogStatus": ALL_LOG_STATUSES[0],
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

        let (status, template) = emulator
            .create_privacy_budget_template(&membership_id, differential_privacy_template())
            .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected CreatePrivacyBudgetTemplate 200, got {status} with {template}"
        );
        let template_id = template["privacyBudgetTemplate"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("setup {name:?}: no privacyBudgetTemplate.id in {template}"))
            .to_string();
        memberships.push((membership_id, template_id));
    }
    let (membership_id, template_id) = memberships[0].clone();
    let (other_membership, other_template) = memberships[1].clone();

    // A template of this membership's own collaboration, created and then deleted: the
    // row that proves the message is composed per request rather than stored.
    let (status, deleted) = emulator
        .delete_privacy_budget_template(&membership_id, &template_id)
        .await;
    assert_eq!(
        status, 204,
        "setup: expected DeletePrivacyBudgetTemplate 204, got {status} with {deleted}"
    );

    // Rows: ids nothing minted, the id that was minted and deleted, and a template that
    // exists in ANOTHER collaboration, so this membership does not reach it.
    let mut unreachable: Vec<(String, String)> = NEVER_MINTED_IDS
        .iter()
        .map(|id| (format!("never minted {id}"), id.to_string()))
        .collect();
    unreachable.push(("minted then deleted".to_string(), template_id.clone()));
    unreachable.push((
        "another collaboration's template".to_string(),
        other_template.clone(),
    ));

    for (label, requested) in unreachable {
        let expected =
            format!("No privacy budget template found with id {requested} for membership {membership_id}.");
        for (operation, (status, body)) in [
            (
                "GetPrivacyBudgetTemplate",
                emulator
                    .get_privacy_budget_template(&membership_id, &requested)
                    .await,
            ),
            (
                "DeletePrivacyBudgetTemplate",
                emulator
                    .delete_privacy_budget_template(&membership_id, &requested)
                    .await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {operation} {label:?}: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} {label:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {operation} {label:?}: the message must name the requested template and \
                 the membership it was addressed through, got {:?}",
                error_message(&body)
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(requested.as_str()),
                "case {operation} {label:?}: the missing resource is the template, so \
                 resourceId must be {requested:?}, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(PRIVACY_BUDGET_TEMPLATE_TYPE),
                "case {operation} {label:?}: expected resourceType \
                 {PRIVACY_BUDGET_TEMPLATE_TYPE}, got {body}"
            );
        }
    }

    // Every refusal above changed nothing: the other collaboration's template is still
    // readable through ITS own membership.
    let (status, body) = emulator
        .get_privacy_budget_template(&other_membership, &other_template)
        .await;
    assert_eq!(
        status, 200,
        "the other collaboration's template must survive, got {status} with {body}"
    );
    assert_eq!(
        body["privacyBudgetTemplate"]["id"].as_str(),
        Some(other_template.as_str()),
        "expected the template {other_template:?}, got {body}"
    );
}
