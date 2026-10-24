// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an `UpdateStage` patch WRITES: the stored stage record
//! itself, rather than one call's answer.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `stage_lifecycle`; the per-member patch
//! tables, the published value sets and the refusals live in the sibling `update_stage`
//! suite.
//!
//! The rules under test here, each over a TABLE of patched members rather than one
//! recorded call (`replace /tracingEnabled`, the patch that turns active X-Ray tracing on
//! for a deployed REST API):
//!
//! * a patch writes the stage RECORD, so a reader other than the one the update answered
//!   reports it too — the `GetStages` collection entry for that stage agrees member for
//!   member with what `UpdateStage` returned, and still does when the collection is read
//!   again after another call changed it, while the stage the patch did not name does not
//!   report the patched value;
//! * a later call that touches the stage for another reason changes only what that call
//!   owns: redeploying the API to the stage re-points its `deploymentId` and leaves every
//!   patched member as it was, which is what makes a setting like active tracing survive
//!   the next deployment.

use serde_json::{json, Value};

use crate::patch_updates::patch_ok;
use crate::rest_api_lifecycle::{
    create_api, create_resource, page_items, string_member, without, Gateway,
};
use crate::stage_collection::get_stages;
use crate::stage_lifecycle::{create_deployment, get_stage};

/// The stage every case patches.
const STAGE: &str = "prod";

/// One row: the case name, the `patchOperations` it sends, the member keys to read in the
/// answered `Stage`, the value that member must then report, and the value a stage the
/// patch did NOT name reports for those same keys — the member's documented default
/// (`tracingEnabled` is `false` by default; every other member here is absent until it is
/// set), which is what makes the isolation assertion fail on a leaked value rather than
/// pass on an absent one.
type Row = (
    &'static str,
    Vec<Value>,
    Vec<&'static str>,
    Value,
    Value,
);

/// The `UpdateStage` URI for the stage every case patches, as the shared
/// [`patch_ok`] helper takes it.
fn stage_uri(api: &str) -> String {
    format!("/restapis/{api}/stages/{STAGE}")
}

/// The value the successive `keys` name in `body`.
fn member_at<'a>(body: &'a Value, keys: &[&str]) -> &'a Value {
    let mut current = body;
    for key in keys {
        current = &current[*key];
    }
    current
}

/// An API with one resource carrying one method, deployed to [`STAGE`]. Answers the API
/// id.
async fn deployed_stage(gateway: &Gateway, case: &str) -> String {
    let (api, root) = create_api(gateway, &format!("update-stage-readers-{case}")).await;
    let resource = create_resource(gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", case);
    let method = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/GET"),
            Some(json!({ "authorizationType": "NONE" })),
        )
        .await;
    assert_eq!(
        method.status, 201,
        "case {case}: expected 201 from PutMethod, got {} with {:?}",
        method.status, method.body
    );
    create_deployment(gateway, &api, case, json!({ "stageName": STAGE })).await;
    api
}

/// The rows every test below spans: one per kind of member a patch writes — a boolean, a
/// string, an entry of the `variables` map, a per-method setting and the
/// `accessLogSettings` structure. The first row is the issue's own patch.
fn rows() -> Vec<Row> {
    vec![
        (
            "replace /tracingEnabled",
            vec![json!({ "op": "replace", "path": "/tracingEnabled", "value": "true" })],
            vec!["tracingEnabled"],
            json!(true),
            json!(false),
        ),
        (
            "replace /description",
            vec![
                json!({ "op": "replace", "path": "/description", "value": "the patched stage" }),
            ],
            vec!["description"],
            json!("the patched stage"),
            Value::Null,
        ),
        (
            "add one stage variable",
            vec![json!({ "op": "add", "path": "/variables/region", "value": "us-east-1" })],
            vec!["variables"],
            json!({ "region": "us-east-1" }),
            Value::Null,
        ),
        (
            "replace /documentationVersion",
            vec![json!({ "op": "replace", "path": "/documentationVersion", "value": "v3" })],
            vec!["documentationVersion"],
            json!("v3"),
            Value::Null,
        ),
        (
            "replace one method setting of every method",
            vec![json!({ "op": "replace", "path": "/*/*/logging/loglevel", "value": "INFO" })],
            vec!["methodSettings", "*/*", "loggingLevel"],
            json!("INFO"),
            Value::Null,
        ),
        (
            "set the access log destination",
            vec![
                json!({ "op": "replace", "path": "/accessLogSettings/destinationArn",
                        "value": "arn:aws:logs:us-east-1:000000000000:log-group:api-access" }),
                json!({ "op": "replace", "path": "/accessLogSettings/format",
                        "value": "$context.requestId" }),
            ],
            vec!["accessLogSettings"],
            json!({
                "destinationArn": "arn:aws:logs:us-east-1:000000000000:log-group:api-access",
                "format": "$context.requestId"
            }),
            Value::Null,
        ),
    ]
}

#[tokio::test]
async fn the_stages_collection_reports_what_the_patch_wrote() {
    for (case, operations, keys, expected, default) in rows() {
        let gateway = Gateway::new();
        let api = deployed_stage(&gateway, case).await;

        let updated = patch_ok(&gateway, case, &stage_uri(&api), operations).await;
        let actual = member_at(&updated, &keys);
        assert_eq!(
            *actual, expected,
            "case {case}: UpdateStage must report {keys:?} as {expected:?}, got {actual:?}"
        );

        // The collection is a second reader of the ONE record the patch wrote, so it may
        // not report it differently from the update that wrote it. (The `GetStage` read of
        // the same record is asserted by the sibling `update_stage` suite, so this test
        // covers the reader that one does not.)
        let collection = get_stages(&gateway, &api, "").await;
        assert_eq!(
            collection.status, 200,
            "case {case}: expected 200 from GetStages, got {} with {:?}",
            collection.status, collection.body
        );
        let items = page_items(&collection.body, case);
        assert_eq!(
            items.len(),
            1,
            "case {case}: expected the API's one stage in the collection, got {items:?}"
        );
        assert_eq!(
            items[0], updated,
            "case {case}: the GetStages entry must report the patched stage, got {:?} after {:?}",
            items[0], updated
        );

        // Read AGAIN after an unrelated call changed the collection, so the entry cannot
        // be one response held from the update: a second stage is created on the same
        // deployment, and the patched stage's entry still reports what the patch wrote,
        // while the stage the patch never named does not.
        let served = string_member(&updated, "deploymentId", case);
        let second = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({ "stageName": "staging", "deploymentId": served })),
            )
            .await;
        assert_eq!(
            second.status, 201,
            "case {case}: expected 201 from CreateStage(staging), got {} with {:?}",
            second.status, second.body
        );
        let reread = get_stages(&gateway, &api, "").await;
        let items = page_items(&reread.body, case);
        assert_eq!(
            items.len(),
            2,
            "case {case}: expected both stages in the collection, got {items:?}"
        );
        let patched_entry = items
            .iter()
            .find(|item| item["stageName"] == json!(STAGE))
            .unwrap_or_else(|| panic!("case {case}: expected a {STAGE} entry, got {items:?}"));
        let actual = member_at(patched_entry, &keys);
        assert_eq!(
            *actual, expected,
            "case {case}: the re-read entry must still report {keys:?} as {expected:?}, got {actual:?}"
        );
        let other = items
            .iter()
            .find(|item| item["stageName"] == json!("staging"))
            .unwrap_or_else(|| panic!("case {case}: expected a staging entry, got {items:?}"));
        let unpatched = member_at(other, &keys);
        assert_eq!(
            *unpatched, default,
            "case {case}: the stage the patch did not name must report {keys:?} as its default {default:?}, got {unpatched:?}"
        );
    }
}

#[tokio::test]
async fn redeploying_the_api_to_the_stage_keeps_what_the_patch_wrote() {
    for (case, operations, keys, expected, _default) in rows() {
        let gateway = Gateway::new();
        let api = deployed_stage(&gateway, case).await;
        let patched = patch_ok(&gateway, case, &stage_uri(&api), operations).await;
        let served = string_member(&patched, "deploymentId", case);

        // A deployment to an existing stage re-points that stage: the record is the same
        // one the patch wrote, so the patched member is still there afterwards.
        let redeployed = create_deployment(
            &gateway,
            &api,
            &format!("{case} / redeploy"),
            json!({ "stageName": STAGE }),
        )
        .await;
        assert_ne!(
            redeployed, served,
            "case {case}: a redeploy creates its own deployment, got {served} twice"
        );

        let read = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            read.body["deploymentId"].as_str(),
            Some(redeployed.as_str()),
            "case {case}: expected the stage to serve the new deployment, got {:?}",
            read.body["deploymentId"]
        );
        let actual = member_at(&read.body, &keys);
        assert_eq!(
            *actual, expected,
            "case {case}: a redeploy must keep {keys:?} as {expected:?}, got {actual:?}"
        );
        // Nothing but the deployment it serves and the instant it last changed may
        // differ: a redeploy is not an update of the stage's settings.
        let moved = ["deploymentId", "lastUpdatedDate"];
        assert_eq!(
            without(&read.body, &moved),
            without(&patched, &moved),
            "case {case}: a redeploy must change only {moved:?}, got {:?} after {:?}",
            read.body,
            patched
        );
    }
}
