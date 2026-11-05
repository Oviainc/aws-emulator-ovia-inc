// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AppIntegrations data-integration ASSOCIATION operations:
//! `CreateDataIntegrationAssociation`, `ListDataIntegrationAssociations` and
//! `UpdateDataIntegrationAssociation`, plus the rule `DeleteDataIntegration` reads from
//! the same store.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (see `harness.rs`), so a response that is not computed
//! from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (which optional members the request carries, the execution mode and
//! which configuration block goes with it, the identifier form, the token length, the
//! page size).

use serde_json::{json, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `MaxResults` is `range 1..50` on `ListDataIntegrationAssociations` (model:
/// `smithy_query(query="operation", operation="ListDataIntegrationAssociations")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 50;

/// The values `ExecutionMode` accepts, from the model's enum
/// (`smithy_query(query="shape", shape="ExecutionMode")`).
const EXECUTION_MODES: [&str; 2] = ["ON_DEMAND", "SCHEDULED"];

/// Create one DataIntegration and answer `(Id, Arn)`.
async fn create_integration(emulator: &Emulator, kms_key: &str, name: &str) -> (String, String) {
    let reply = emulator
        .send(
            "POST",
            "/dataIntegrations",
            REGION,
            Some(json!({
                "Name": name,
                "KmsKey": kms_key,
                "SourceURI": "s3://feed-bucket/accounts",
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "CreateDataIntegration must succeed so a case has an integration, got {} / {}",
        reply.status, reply.body
    );
    (
        reply.string("Id").expect("the create reported an Id"),
        reply.string("Arn").expect("the create reported an ARN"),
    )
}

/// `CreateDataIntegrationAssociation`:
/// `POST /dataIntegrations/{DataIntegrationIdentifier}/associations`.
async fn associate(emulator: &Emulator, integration: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/dataIntegrations/{}/associations",
                encode_label(integration)
            ),
            REGION,
            Some(body),
        )
        .await
}

/// `ListDataIntegrationAssociations`:
/// `GET /dataIntegrations/{DataIntegrationIdentifier}/associations`.
async fn list_associations(emulator: &Emulator, integration: &str, query: &str) -> Reply {
    let base = format!(
        "/dataIntegrations/{}/associations",
        encode_label(integration)
    );
    let uri = if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    };
    emulator.send("GET", &uri, REGION, None).await
}

/// `UpdateDataIntegrationAssociation`: `PATCH /dataIntegrations/{DataIntegrationIdentifier}
/// /associations/{DataIntegrationAssociationIdentifier}`.
async fn update_association(
    emulator: &Emulator,
    integration: &str,
    association: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "PATCH",
            &format!(
                "/dataIntegrations/{}/associations/{}",
                encode_label(integration),
                encode_label(association)
            ),
            REGION,
            Some(body),
        )
        .await
}

/// `DeleteDataIntegration`, to show the association rule it reads.
async fn delete_integration(emulator: &Emulator, integration: &str) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!("/dataIntegrations/{}", encode_label(integration)),
            REGION,
            None,
        )
        .await
}

/// The reported page of associations, as a list (an absent member is an empty page).
fn page_of(reply: &Reply) -> Vec<Value> {
    reply
        .field("DataIntegrationAssociations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Whether `value` has the shape the model's `UUID` pattern publishes:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
fn is_uuid_shaped(value: &str) -> bool {
    let group_lengths: Vec<usize> = value.split('-').map(str::len).collect();
    group_lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f' | '-'))
}

/// A valid `ExecutionConfiguration` for `mode`: the block that belongs to it, and no
/// other.
fn execution_configuration(mode: &str) -> Value {
    match mode {
        "ON_DEMAND" => json!({
            "ExecutionMode": "ON_DEMAND",
            "OnDemandConfiguration": { "StartTime": "1685400000000" },
        }),
        _ => json!({
            "ExecutionMode": "SCHEDULED",
            "ScheduleConfiguration": { "ScheduleExpression": "rate(1 hour)" },
        }),
    }
}

#[tokio::test]
async fn a_created_association_is_reported_by_the_listing_exactly_as_stored() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in which optional members the request carries and in the execution
    // mode and its configuration block. An integration can be associated once, so each
    // row gets its own integration.
    let rows: Vec<(&str, Value)> = vec![
        ("minimal: no optional member at all", json!({})),
        (
            "a client id and a destination",
            json!({
                "ClientId": "connect-instance-1",
                "DestinationURI": "s3://destination-bucket/accounts",
            }),
        ),
        (
            "an on-demand window with a start only",
            json!({ "ExecutionConfiguration": execution_configuration("ON_DEMAND") }),
        ),
        (
            "an on-demand window with a start and an end",
            json!({
                "ClientId": "c",
                "ExecutionConfiguration": {
                    "ExecutionMode": "ON_DEMAND",
                    "OnDemandConfiguration": {
                        "StartTime": "1685400000000",
                        "EndTime": "1685486400000",
                    },
                },
            }),
        ),
        (
            "a schedule carrying every member of ScheduleConfiguration",
            json!({
                "DestinationURI": "https://example.com/sink",
                "ExecutionConfiguration": {
                    "ExecutionMode": "SCHEDULED",
                    "ScheduleConfiguration": {
                        "ScheduleExpression": "rate(15 minutes)",
                        "FirstExecutionFrom": "1685400000000",
                        "Object": "Account",
                    },
                },
            }),
        ),
        (
            "members the summary shape does not carry",
            json!({
                "ObjectConfiguration": { "Customer": { "Account": ["Id", "Name"] } },
                "ClientAssociationMetadata": { "owner": "crm" },
            }),
        ),
    ];

    for (index, (label, body)) in rows.iter().enumerate() {
        let (integration_id, integration_arn) =
            create_integration(&emulator, &kms_key, &format!("assoc-case-{index}")).await;

        let created = associate(&emulator, &integration_id, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateDataIntegrationAssociation, got {} / {}",
            created.status, created.body
        );
        let association_id = created
            .string("DataIntegrationAssociationId")
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: expected a DataIntegrationAssociationId, got {}",
                    created.body
                )
            });
        assert!(
            is_uuid_shaped(&association_id),
            "case {label:?}: expected a UUID-shaped DataIntegrationAssociationId (model: UUID), \
             got {association_id:?}"
        );
        assert_eq!(
            created.string("DataIntegrationArn").as_deref(),
            Some(integration_arn.as_str()),
            "case {label:?}: expected the ARN of the integration associated, got {:?}",
            created.field("DataIntegrationArn")
        );

        // Both identifier forms of the integration report the same association.
        for (form, identifier) in [
            ("by id", integration_id.clone()),
            ("by arn", integration_arn.clone()),
        ] {
            let listed = list_associations(&emulator, &identifier, "").await;
            assert_eq!(
                listed.status, 200,
                "case {label:?} {form}: expected HTTP 200 from \
                 ListDataIntegrationAssociations, got {} / {}",
                listed.status, listed.body
            );
            let page = page_of(&listed);

            // The summary is exactly what `DataIntegrationAssociationSummary` models for
            // the stored association: the two ARNs, the members the request carried, and
            // no `LastExecutionStatus` (no import/export job has run for it).
            let mut expected = json!({
                "DataIntegrationAssociationArn": format!(
                    "arn:aws:app-integrations:{REGION}:{ACCOUNT}:\
                     data-integration-association/{integration_id}/{association_id}"
                ),
                "DataIntegrationArn": integration_arn,
            });
            let expected_object = expected.as_object_mut().expect("the summary is an object");
            for member in ["ClientId", "DestinationURI", "ExecutionConfiguration"] {
                if let Some(value) = body.get(member) {
                    expected_object.insert(member.to_string(), value.clone());
                }
            }
            assert_eq!(
                page,
                vec![expected],
                "case {label:?} {form}: expected exactly the stored association, got {}",
                listed.body
            );
            assert_eq!(
                listed.field("NextToken"),
                None,
                "case {label:?} {form}: a single page must report no NextToken, got {:?}",
                listed.field("NextToken")
            );
        }
    }
}

#[tokio::test]
async fn an_unassociated_data_integration_reports_an_empty_association_list() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in the identifier form, and in whether anything else in the store is
    // associated: an integration's listing reports ITS associations, so a neighbour's
    // association is not reported for it.
    let (lonely_id, lonely_arn) = create_integration(&emulator, &kms_key, "lonely").await;
    let (neighbour_id, _) = create_integration(&emulator, &kms_key, "neighbour").await;

    for stage in ["before a neighbour is associated", "after it is"] {
        for (form, identifier) in [("by id", lonely_id.clone()), ("by arn", lonely_arn.clone())] {
            let listed = list_associations(&emulator, &identifier, "").await;
            assert_eq!(
                listed.status, 200,
                "case {stage} {form}: expected HTTP 200 for an unassociated integration, \
                 got {} / {}",
                listed.status, listed.body
            );
            assert_eq!(
                page_of(&listed),
                Vec::<Value>::new(),
                "case {stage} {form}: expected no association, got {}",
                listed.body
            );
            assert_eq!(
                listed.field("NextToken"),
                None,
                "case {stage} {form}: an empty page must report no NextToken, got {:?}",
                listed.field("NextToken")
            );
        }
        if stage == "before a neighbour is associated" {
            let created = associate(&emulator, &neighbour_id, json!({ "ClientId": "n" })).await;
            assert_eq!(
                created.status, 200,
                "the neighbour must be associable, got {} / {}",
                created.status, created.body
            );
        }
    }

    // An identifier this account holds no DataIntegration for has no associations
    // either, which is an empty page and not a refusal: this operation reports the
    // associations an identifier names, it does not verify the integration exists.
    // Rows differ in the FORM of that identifier, including one whose integration was
    // deleted after it was created.
    let (doomed_id, doomed_arn) = create_integration(&emulator, &kms_key, "doomed").await;
    let deleted = delete_integration(&emulator, &doomed_id).await;
    assert_eq!(
        deleted.status, 200,
        "the integration must be deletable so a row can list a gone one, got {} / {}",
        deleted.status, deleted.body
    );
    for (label, identifier) in [
        (
            "a UUID this service never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an ARN from another account",
            format!("arn:aws:app-integrations:{REGION}:111122223333:data-integration/{lonely_id}"),
        ),
        ("a name rather than an identifier", "lonely".to_string()),
        ("an identifier that is neither form", "gone-1".to_string()),
        ("the id of a deleted integration", doomed_id.clone()),
        ("the ARN of a deleted integration", doomed_arn.clone()),
    ] {
        let listed = list_associations(&emulator, &identifier, "").await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected HTTP 200, got {} / {}",
            listed.status, listed.body
        );
        assert_eq!(
            page_of(&listed),
            Vec::<Value>::new(),
            "case {label:?}: expected an empty page, got {}",
            listed.body
        );
        assert_eq!(
            listed.field("NextToken"),
            None,
            "case {label:?}: an empty page must report no NextToken, got {:?}",
            listed.field("NextToken")
        );
    }

    // The integration that DOES hold an association still reports it: answering an
    // unresolvable identifier with an empty page reports nothing for the resolvable one.
    let neighbour_page = list_associations(&emulator, &neighbour_id, "").await;
    assert_eq!(
        neighbour_page.status, 200,
        "expected HTTP 200 for the associated integration, got {} / {}",
        neighbour_page.status, neighbour_page.body
    );
    assert_eq!(
        page_of(&neighbour_page).len(),
        1,
        "expected the associated integration to still report its association, got {}",
        neighbour_page.body
    );
}

#[tokio::test]
async fn an_execution_configuration_whose_block_does_not_match_its_mode_is_refused() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in HOW the configuration and the mode disagree, and in whether the
    // epoch-millisecond times an on-demand window carries are times at all. `accepted`
    // is what the rule says about each one.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "ON_DEMAND with its own block",
            execution_configuration("ON_DEMAND"),
            true,
        ),
        (
            "SCHEDULED with its own block",
            execution_configuration("SCHEDULED"),
            true,
        ),
        (
            "ON_DEMAND with no block at all",
            json!({ "ExecutionMode": "ON_DEMAND" }),
            false,
        ),
        (
            "SCHEDULED with no block at all",
            json!({ "ExecutionMode": "SCHEDULED" }),
            false,
        ),
        (
            "ON_DEMAND carrying the schedule block",
            json!({
                "ExecutionMode": "ON_DEMAND",
                "ScheduleConfiguration": { "ScheduleExpression": "rate(1 hour)" },
            }),
            false,
        ),
        (
            "SCHEDULED carrying the on-demand block",
            json!({
                "ExecutionMode": "SCHEDULED",
                "OnDemandConfiguration": { "StartTime": "1685400000000" },
            }),
            false,
        ),
        (
            "ON_DEMAND carrying both blocks",
            json!({
                "ExecutionMode": "ON_DEMAND",
                "OnDemandConfiguration": { "StartTime": "1685400000000" },
                "ScheduleConfiguration": { "ScheduleExpression": "rate(1 hour)" },
            }),
            false,
        ),
        (
            "a StartTime that is no epoch time",
            json!({
                "ExecutionMode": "ON_DEMAND",
                "OnDemandConfiguration": { "StartTime": "yesterday" },
            }),
            false,
        ),
        (
            "an EndTime that is no epoch time",
            json!({
                "ExecutionMode": "ON_DEMAND",
                "OnDemandConfiguration": {
                    "StartTime": "1685400000000",
                    "EndTime": "2023-06-01T00:00:00Z",
                },
            }),
            false,
        ),
        (
            "an EndTime before its StartTime",
            json!({
                "ExecutionMode": "ON_DEMAND",
                "OnDemandConfiguration": {
                    "StartTime": "1685400000000",
                    "EndTime": "1685300000000",
                },
            }),
            false,
        ),
        (
            "an EndTime equal to its StartTime",
            json!({
                "ExecutionMode": "ON_DEMAND",
                "OnDemandConfiguration": {
                    "StartTime": "1685400000000",
                    "EndTime": "1685400000000",
                },
            }),
            true,
        ),
    ];

    for (index, (label, configuration, accepted)) in rows.iter().enumerate() {
        let (integration_id, _) =
            create_integration(&emulator, &kms_key, &format!("exec-case-{index}")).await;
        let reply = associate(
            &emulator,
            &integration_id,
            json!({ "ExecutionConfiguration": configuration }),
        )
        .await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {label:?}: expected HTTP 400, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "InvalidRequestException",
                "case {label:?}: expected InvalidRequestException, got {:?} / {}",
                reply.error_type, reply.body
            );
        }

        // Accepted or refused, the store agrees with the answer: a refused create leaves
        // the integration unassociated.
        let page = page_of(&list_associations(&emulator, &integration_id, "").await);
        assert_eq!(
            page.len(),
            usize::from(*accepted),
            "case {label:?}: expected {} stored association(s), got {page:?}",
            usize::from(*accepted)
        );
    }

    // A mode outside the modelled enum is refused at the boundary, before any handler.
    let (integration_id, _) = create_integration(&emulator, &kms_key, "exec-case-mode").await;
    let refused = associate(
        &emulator,
        &integration_id,
        json!({ "ExecutionConfiguration": { "ExecutionMode": "WHENEVER" } }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"ExecutionMode=WHENEVER\": expected HTTP 400, got {} / {}",
        refused.status, refused.body
    );
}

#[tokio::test]
async fn an_update_replaces_the_execution_configuration_it_reports() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Every ordered pair of modes, so the rule is about replacement and not about one
    // direction: each row starts from one mode's configuration and moves to the other's
    // (and to its own).
    for from in EXECUTION_MODES {
        for to in EXECUTION_MODES {
            let label = format!("{from} -> {to}");
            let (integration_id, integration_arn) = create_integration(
                &emulator,
                &kms_key,
                &format!("update-{}-{}", from.to_lowercase(), to.to_lowercase()),
            )
            .await;
            let created = associate(
                &emulator,
                &integration_id,
                json!({ "ExecutionConfiguration": execution_configuration(from) }),
            )
            .await;
            let association_id = created
                .string("DataIntegrationAssociationId")
                .expect("the create reported an association id");
            let association_arn = format!(
                "arn:aws:app-integrations:{REGION}:{ACCOUNT}:\
                 data-integration-association/{integration_id}/{association_id}"
            );

            // Both identifier forms of the association address it.
            for (form, identifier) in [
                ("by id", association_id.clone()),
                ("by arn", association_arn.clone()),
            ] {
                let replacement = execution_configuration(to);
                let updated = update_association(
                    &emulator,
                    &integration_id,
                    &identifier,
                    json!({ "ExecutionConfiguration": replacement }),
                )
                .await;
                assert_eq!(
                    updated.status, 200,
                    "case {label} {form}: expected HTTP 200 from \
                     UpdateDataIntegrationAssociation, got {} / {}",
                    updated.status, updated.body
                );

                let page = page_of(&list_associations(&emulator, &integration_arn, "").await);
                assert_eq!(
                    page.len(),
                    1,
                    "case {label} {form}: an update must not add an association, got {page:?}"
                );
                assert_eq!(
                    page[0].get("ExecutionConfiguration"),
                    Some(&execution_configuration(to)),
                    "case {label} {form}: expected the replacing configuration, got {}",
                    page[0]
                );
                // Identity and the members the update does not carry are untouched.
                assert_eq!(
                    page[0].get("DataIntegrationAssociationArn"),
                    Some(&json!(association_arn)),
                    "case {label} {form}: an update must not change the ARN, got {}",
                    page[0]
                );
            }

            // A configuration the create would refuse is refused here too, and the stored
            // one is left as it was.
            let refused = update_association(
                &emulator,
                &integration_id,
                &association_id,
                json!({ "ExecutionConfiguration": { "ExecutionMode": to } }),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {label}: expected HTTP 400 for a mode without its block, got {} / {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type, "InvalidRequestException",
                "case {label}: expected InvalidRequestException, got {:?}",
                refused.error_type
            );
            let page = page_of(&list_associations(&emulator, &integration_id, "").await);
            assert_eq!(
                page[0].get("ExecutionConfiguration"),
                Some(&execution_configuration(to)),
                "case {label}: a refused update must leave the stored configuration, got {}",
                page[0]
            );
        }
    }
}

#[tokio::test]
async fn an_update_refuses_identifiers_this_service_did_not_mint() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    let (integration_id, _) = create_integration(&emulator, &kms_key, "update-404-target").await;
    let (other_id, _) = create_integration(&emulator, &kms_key, "update-404-other").await;
    let association_id = associate(
        &emulator,
        &integration_id,
        json!({ "ExecutionConfiguration": execution_configuration("SCHEDULED") }),
    )
    .await
    .string("DataIntegrationAssociationId")
    .expect("the create reported an association id");
    let other_association_id = associate(
        &emulator,
        &other_id,
        json!({ "ExecutionConfiguration": execution_configuration("SCHEDULED") }),
    )
    .await
    .string("DataIntegrationAssociationId")
    .expect("the create reported an association id");

    let unminted = "00000000-0000-0000-0000-000000000000".to_string();
    // Rows differ in WHICH of the two identifiers names nothing: neither, the
    // integration, the association, and an association that belongs to another
    // integration (which this one does not reach).
    let rows: Vec<(&str, String, String)> = vec![
        (
            "neither identifier was minted",
            unminted.clone(),
            unminted.clone(),
        ),
        (
            "the integration was not minted",
            unminted.clone(),
            association_id.clone(),
        ),
        (
            "the association was not minted",
            integration_id.clone(),
            unminted,
        ),
        (
            "the association belongs to another integration",
            integration_id.clone(),
            other_association_id,
        ),
    ];

    for (label, integration, association) in &rows {
        let reply = update_association(
            &emulator,
            integration,
            association,
            json!({ "ExecutionConfiguration": execution_configuration("ON_DEMAND") }),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
    }

    // None of those calls touched the association that does exist.
    let page = page_of(&list_associations(&emulator, &integration_id, "").await);
    assert_eq!(
        page[0].get("ExecutionConfiguration"),
        Some(&execution_configuration("SCHEDULED")),
        "a refused update must leave the stored configuration, got {}",
        page[0]
    );
}

#[tokio::test]
async fn a_previously_associated_data_integration_refuses_a_second_association() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in the identifier form the second create addresses the integration by:
    // the rule is about the resource, not about how it was named.
    let (integration_id, integration_arn) =
        create_integration(&emulator, &kms_key, "associated.once").await;
    let first = associate(&emulator, &integration_id, json!({ "ClientId": "first" })).await;
    assert_eq!(
        first.status, 200,
        "the first association must succeed, got {} / {}",
        first.status, first.body
    );

    for (form, identifier) in [
        ("by id", integration_id.clone()),
        ("by arn", integration_arn.clone()),
    ] {
        let second = associate(&emulator, &identifier, json!({ "ClientId": "second" })).await;
        assert_eq!(
            second.status, 400,
            "case {form}: expected HTTP 400 for a previously associated integration, got {} / {}",
            second.status, second.body
        );
        assert_eq!(
            second.error_type, "InvalidRequestException",
            "case {form}: expected InvalidRequestException, got {:?}",
            second.error_type
        );
        let page = page_of(&list_associations(&emulator, &identifier, "").await);
        assert_eq!(
            page.len(),
            1,
            "case {form}: the refused create must add nothing, got {page:?}"
        );
        assert_eq!(
            page[0].get("ClientId"),
            Some(&json!("first")),
            "case {form}: expected the first association to survive, got {}",
            page[0]
        );
    }

    // "Only DataIntegrations that don't have any DataIntegrationAssociations can be
    // deleted" — the rule `DeleteDataIntegration` reads from the same store.
    let refused = delete_integration(&emulator, &integration_id).await;
    assert_eq!(
        refused.status, 400,
        "expected HTTP 400 when deleting an associated integration, got {} / {}",
        refused.status, refused.body
    );

    // Another integration is unaffected: the refusal is about this one's associations.
    let (fresh_id, _) = create_integration(&emulator, &kms_key, "associated.never").await;
    let accepted = associate(&emulator, &fresh_id, json!({ "ClientId": "fresh" })).await;
    assert_eq!(
        accepted.status, 200,
        "a different integration must still be associable, got {} / {}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn an_association_create_repeated_with_its_client_token_reports_the_one_it_minted() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;

    // Rows differ in the token's form and length, including the published bounds of
    // `ClientToken` (len 1..2048).
    for token in ["t", "retry-token-1", &"z".repeat(2048)] {
        let (integration_id, _) =
            create_integration(&emulator, &kms_key, &format!("token-{}", token.len())).await;
        let body = json!({ "ClientId": "idempotent", "ClientToken": token });

        let first = associate(&emulator, &integration_id, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case token len {}: expected HTTP 200, got {} / {}",
            token.len(),
            first.status,
            first.body
        );
        let replay = associate(&emulator, &integration_id, body).await;
        assert_eq!(
            replay.status, 200,
            "case token len {}: expected a retry under the same token to succeed, got {} / {}",
            token.len(),
            replay.status,
            replay.body
        );
        assert_eq!(
            replay.field("DataIntegrationAssociationId"),
            first.field("DataIntegrationAssociationId"),
            "case token len {}: expected the id the first call minted, got {:?}",
            token.len(),
            replay.field("DataIntegrationAssociationId")
        );
        let page = page_of(&list_associations(&emulator, &integration_id, "").await);
        assert_eq!(
            page.len(),
            1,
            "case token len {}: expected one association per token, got {page:?}",
            token.len()
        );

        // The same token aimed at ANOTHER integration is not a retry of this request: it
        // is refused rather than answered with an association of the integration the
        // caller did not name.
        let (other_id, _) =
            create_integration(&emulator, &kms_key, &format!("token-other-{}", token.len())).await;
        let crossed = associate(
            &emulator,
            &other_id,
            json!({ "ClientId": "crossed", "ClientToken": token }),
        )
        .await;
        assert_eq!(
            crossed.status, 400,
            "case token len {}: expected HTTP 400 for a token minted on another \
             integration, got {} / {}",
            token.len(),
            crossed.status,
            crossed.body
        );
        assert_eq!(
            crossed.error_type, "InvalidRequestException",
            "case token len {}: expected InvalidRequestException, got {:?}",
            token.len(),
            crossed.error_type
        );
        assert_eq!(
            page_of(&list_associations(&emulator, &other_id, "").await),
            Vec::<Value>::new(),
            "case token len {}: the refused create must leave the other integration \
             unassociated",
            token.len()
        );
    }
}

#[tokio::test]
async fn the_page_controls_of_the_association_listing_follow_their_published_bounds() {
    let emulator = Emulator::new();
    let kms_key = emulator.create_kms_key(REGION).await;
    let (integration_id, _) = create_integration(&emulator, &kms_key, "paged").await;
    associate(&emulator, &integration_id, json!({ "ClientId": "paged" })).await;

    // One past each published bound of `MaxResults` (range 1..50), then each bound: a
    // page size the model allows reports the stored association and no cursor.
    for (max, expected) in [
        (MAX_RESULTS_MIN - 1, 400),
        (MAX_RESULTS_MAX + 1, 400),
        (MAX_RESULTS_MIN, 200),
        (MAX_RESULTS_MAX, 200),
    ] {
        let reply =
            list_associations(&emulator, &integration_id, &format!("maxResults={max}")).await;
        assert_eq!(
            reply.status, expected,
            "case maxResults={max}: expected HTTP {expected}, got {} / {}",
            reply.status, reply.body
        );
        if expected == 200 {
            assert_eq!(
                page_of(&reply).len(),
                1,
                "case maxResults={max}: expected the stored association, got {}",
                reply.body
            );
            assert_eq!(
                reply.field("NextToken"),
                None,
                "case maxResults={max}: a last page must report no NextToken, got {:?}",
                reply.field("NextToken")
            );
        }
    }

    // Tokens differing in why they are not cursors this service minted: not hex at all,
    // odd-length hex, hex that decodes to bytes that are not text.
    for token in ["not-a-cursor", "abc", "fffe"] {
        let reply =
            list_associations(&emulator, &integration_id, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "InvalidRequestException",
            "case {token:?}: expected InvalidRequestException, got {:?}",
            reply.error_type
        );
    }
}
