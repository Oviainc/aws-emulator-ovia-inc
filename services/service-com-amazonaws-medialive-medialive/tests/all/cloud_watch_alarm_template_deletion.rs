// SPDX-License-Identifier: CC0-1.0

//! When a cloudwatch alarm template group's membership is RELEASED, end to end
//! over the restJson1 wire.
//!
//! Every test drives the service the way a client does — real HTTP requests
//! dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.
//!
//! What a template delete removes, and that it is refused for a template nothing
//! holds or in another region, is the table over all four monitor template
//! resources in `template_deletion.rs`; what this suite adds is the INSTANT the
//! group becomes deletable: `DeleteCloudWatchAlarmTemplateGroup` is refused with
//! its modelled `ConflictException` while the group still holds ANY template and
//! answers its modelled 204 the moment the last one is gone, because the
//! occupancy the refusal reads is derived from the templates that exist. The
//! issue's recorded teardown — the templates deleted, then the group — is ONE row
//! of the table below.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{percent_encoded, GROUPS};
use crate::wire::{call_in_region, error_code, medialive, DEFAULT_REGION};

/// The collection `CreateCloudWatchAlarmTemplate` is bound to, which the delete
/// and the reader address an instance under
/// (`smithy_query(query="operation", operation="CreateCloudWatchAlarmTemplate")`:
/// POST /prod/cloudwatch-alarm-templates).
const TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";

/// The status every monitor template delete of this service is modelled with
/// (`code 204`), and the status its create answers (`code 201`).
const DELETED: u16 = 204;
const CREATED: u16 = 201;

/// Every required member of `CreateCloudWatchAlarmTemplate` besides
/// `GroupIdentifier`, each with one value the model admits, so a create in this
/// suite is a complete request (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`).
fn template_body(name: &str, group: &str) -> Value {
    json!({
        "name": name,
        "groupIdentifier": group,
        "comparisonOperator": "GreaterThanThreshold",
        "evaluationPeriods": 1,
        "metricName": "InputVideoFrameRate",
        "period": 60,
        "statistic": "Average",
        "targetResourceType": "MEDIALIVE_CHANNEL",
        "threshold": 29.97,
        "treatMissingData": "notBreaching",
    })
}

async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, group) = call_in_region(
        router,
        region,
        "POST",
        GROUPS,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        status, CREATED,
        "setup: expected the group {name:?} in {region} to be created, got {status}: {group}"
    );
    group
}

async fn create_template(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    group: &str,
) -> Value {
    let (status, template) = call_in_region(
        router,
        region,
        "POST",
        TEMPLATES,
        Some(template_body(name, group)),
    )
    .await;
    assert_eq!(
        status, CREATED,
        "setup: expected the template {name:?} to be created, got {status}: {template}"
    );
    template
}

async fn get_template(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{TEMPLATES}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

async fn delete_template(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DELETE",
        &format!("{TEMPLATES}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

async fn get_group(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{GROUPS}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

async fn delete_group(router: &MultiServiceRouter, region: &str, identifier: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DELETE",
        &format!("{GROUPS}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

/// A 204 carries no payload: the operation's output has no members.
fn assert_no_body(case: &str, body: &Value) {
    assert!(
        body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: expected the delete to answer no body, got {body}"
    );
}

/// The id a create reported.
fn minted_id(case: &str, created: &Value) -> String {
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no id: {created}"))
        .to_string()
}

/// Rule: a group's occupancy is derived from the templates that exist, so the
/// group delete is refused with the modelled `ConflictException` (409) — removing
/// nothing — while ANY template still names it, and answers the modelled 204 with
/// an empty body the moment the last one has been deleted, after which the group
/// is gone from state by either identifier form.
///
/// The rows span what the rule generalises over: how many templates the group
/// holds, the identifier form the group delete names the group by and the region
/// the resources live in. Within a row, the group delete is tried again after
/// EVERY member delete, so the refusal is pinned at each occupancy down to one
/// rather than only at the full group.
#[tokio::test]
async fn an_alarm_template_group_becomes_deletable_as_its_last_template_goes() {
    // (case, how many templates the group holds, the region, whether the group
    // delete names the group by its name rather than its id)
    let rows: [(&str, usize, &str, bool); 5] = [
        ("one template", 1, DEFAULT_REGION, false),
        ("one template, group deleted by name", 1, DEFAULT_REGION, true),
        ("two templates", 2, DEFAULT_REGION, false),
        ("five templates", 5, DEFAULT_REGION, true),
        ("three templates in another region", 3, "eu-central-1", false),
    ];

    for (case, held, region, by_name) in rows {
        let router = medialive();
        let group_name = "crowded-group";
        let group = create_group(&router, region, group_name).await;
        let group_id = minted_id(case, &group);
        let group_identifier = if by_name {
            group_name
        } else {
            group_id.as_str()
        };

        let mut templates: Vec<String> = Vec::new();
        for index in 0..held {
            let template =
                create_template(&router, region, &format!("member-{index}"), group_name).await;
            templates.push(minted_id(case, &template));
        }

        // Deleting the members ONE at a time: the group stays refused while any
        // member is left, because the refusal counts the templates that exist.
        for (index, id) in templates.iter().enumerate() {
            let left = held - index;
            let (status, refused) = delete_group(&router, region, group_identifier).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected the group delete to be refused with {left} member(s) left, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?}: expected ConflictException with {left} member(s) left, got {refused}"
            );
            // The refused delete removed nothing: the group still reads back.
            let (status, read) = get_group(&router, region, &group_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the refused delete to leave the group, got {status}: {read}"
            );
            assert_eq!(
                read, group,
                "case {case:?}: expected the group unchanged by the refused delete, got {read}"
            );

            let (status, body) = delete_template(&router, region, id).await;
            assert_eq!(
                status, DELETED,
                "case {case:?}: expected member {id} to be deleted, got {status}: {body}"
            );
            assert_no_body(case, &body);

            // Every member not yet deleted is untouched by this delete.
            for survivor in &templates[index + 1..] {
                let (status, read) = get_template(&router, region, survivor).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected member {survivor} to survive the delete of {id}, got {status}: {read}"
                );
            }
        }

        // The group is empty now, so the delete the issue saw refused succeeds.
        let (status, body) = delete_group(&router, region, group_identifier).await;
        assert_eq!(
            status, DELETED,
            "case {case:?}: expected the emptied group to be deleted, got {status}: {body}"
        );
        assert_no_body(case, &body);

        for missed in [group_id.as_str(), group_name] {
            let (status, read) = get_group(&router, region, missed).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected reading the deleted group by {missed:?} to answer 404, got {status}: {read}"
            );
            assert_eq!(
                error_code(status, &read),
                "NotFoundException",
                "case {case:?}: expected NotFoundException for {missed:?}, got {read}"
            );
        }
    }
}
