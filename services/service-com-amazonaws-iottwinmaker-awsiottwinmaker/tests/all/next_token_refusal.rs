// SPDX-License-Identifier: CC0-1.0

//! How every paginated operation of this service refuses a `nextToken` it did
//! not mint, end to end over HTTP.
//!
//! RULE under test: a paginated operation refuses a `nextToken` it cannot
//! recognise with a 400 `ValidationException` whose message names the member and
//! what is wrong with it — ONE sentence, the same whichever listing was asked —
//! and the token's own bytes never reach the response: the value is opaque to
//! the caller and modelled up to 17880 characters of any bytes
//! (`smithy_query(query="shape",shape="NextToken")`: `@length(0..17880)`,
//! `@pattern(".*")`), so copying it into an error body would hand a caller an
//! echo channel and make one failure read a different way per operation.
//!
//! The rows differ along what the rule generalises over: WHICH paginated
//! operation is asked (every one this service serves, account-level,
//! workspace-level, entity-level and data-plane), and WHAT the forged token is —
//! text that is not hex at all, hex of the wrong length, hex that decodes to
//! other bytes, hex that decodes to no UTF-8, punctuation and markup, non-ASCII
//! text, a token at the member's published maximum length, and a REAL token
//! another listing minted. A token one past that maximum is not a row: the
//! modelled length constraint refuses it before any handler reads it, which is a
//! different rule.
//!
//! The companion case pins the other side of the rule, so the refusal cannot be
//! satisfied by refusing everything: a token a listing DID mint, and an empty
//! one, are still served.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::SYNC_SOURCE_SITEWISE;

use crate::harness::{seed_sync_job, workspace_arn, Emulator, Resp};

/// The workspace every paginated call below is taken over.
const WORKSPACE: &str = "tokenRefusalWs";
/// A second workspace, so the account-level listing has a page to leave over.
const OTHER_WORKSPACE: &str = "tokenRefusalWsTwo";
/// The component type the seeded entities instantiate: a time-series property
/// for the history read, two valued properties in a `TABULAR` group for the
/// latest-value and table reads.
const SENSOR: &str = "com.example.sensorToken";

/// The ONE sentence every paginated operation of this service reports for a
/// token it did not mint. Written out here rather than read from the crate, so
/// this asserts the wording instead of restating the implementation.
const REFUSAL: &str = "nextToken is not a token this listing minted";

/// The workspaces, component type, entities, scene, tags and sync job the
/// paginated calls below are taken over.
async fn seed(emulator: &Emulator) {
    for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
        let created = emulator.create_workspace(workspace_id).await;
        assert_eq!(
            created.status, 200,
            "CreateWorkspace {workspace_id:?}: expected 200, got {created:?}"
        );
    }

    let created = emulator
        .create_component_type(
            WORKSPACE,
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    "Tag": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset-tag"},
                    },
                    "Threshold": {
                        "dataType": {"type": "DOUBLE"},
                        "defaultValue": {"doubleValue": 1.5},
                    },
                },
                "propertyGroups": {
                    "Readings": {"groupType": "TABULAR", "propertyNames": ["Tag", "Threshold"]},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {SENSOR:?}: expected 200, got {created:?}"
    );

    // Two entities, so the entity listing has a page to leave over as well.
    for entity_id in ["unit-1", "unit-2"] {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {"Sensor": {"componentTypeId": SENSOR}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }

    let created = emulator
        .create_scene(
            WORKSPACE,
            json!({"sceneId": "scene-1", "contentLocation": "s3://bucket/scene-1.json"}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateScene: expected 200, got {created:?}"
    );

    let tagged = emulator
        .tag_resource(&workspace_arn(WORKSPACE), json!({"env": "test"}))
        .await;
    assert_eq!(
        tagged.status, 200,
        "TagResource: expected 200, got {tagged:?}"
    );

    // A sync job, so the sync-job and sync-resource listings have a scope to be
    // taken over.
    seed_sync_job(emulator, WORKSPACE, SYNC_SOURCE_SITEWISE);
}

/// One paginated request: the route it is served on and the members it carries
/// besides `nextToken`.
struct Paginated {
    /// The operation, for the assertion messages.
    operation: &'static str,
    path: String,
    body: Value,
}

/// Every paginated request this service serves, each otherwise valid, so the
/// only thing wrong with it is the token a case puts in it.
fn paginated_requests() -> Vec<Paginated> {
    let paginated = |operation: &'static str, path: String, body: Value| Paginated {
        operation,
        path,
        body,
    };
    vec![
        paginated("ListWorkspaces", "/workspaces-list".to_string(), json!({})),
        paginated(
            "ListEntities",
            format!("/workspaces/{WORKSPACE}/entities-list"),
            json!({}),
        ),
        paginated(
            "ListComponentTypes",
            format!("/workspaces/{WORKSPACE}/component-types-list"),
            json!({}),
        ),
        paginated(
            "ListComponents",
            format!("/workspaces/{WORKSPACE}/entities/unit-1/components-list"),
            json!({}),
        ),
        paginated(
            "ListProperties",
            format!("/workspaces/{WORKSPACE}/properties-list"),
            json!({"entityId": "unit-1", "componentName": "Sensor"}),
        ),
        paginated(
            "ListScenes",
            format!("/workspaces/{WORKSPACE}/scenes-list"),
            json!({}),
        ),
        paginated(
            "ListSyncJobs",
            format!("/workspaces/{WORKSPACE}/sync-jobs-list"),
            json!({}),
        ),
        paginated(
            "ListSyncResources",
            format!("/workspaces/{WORKSPACE}/sync-jobs/{SYNC_SOURCE_SITEWISE}/resources-list"),
            json!({}),
        ),
        paginated(
            "ListMetadataTransferJobs",
            "/metadata-transfer-jobs-list".to_string(),
            json!({"sourceType": "s3", "destinationType": "iottwinmaker"}),
        ),
        paginated(
            "ListTagsForResource",
            "/tags-list".to_string(),
            json!({"resourceARN": workspace_arn(WORKSPACE)}),
        ),
        paginated(
            "ExecuteQuery",
            "/queries/execution".to_string(),
            json!({
                "workspaceId": WORKSPACE,
                "queryStatement": "SELECT e FROM EntityGraph MATCH (e)",
            }),
        ),
        paginated(
            "GetPropertyValue (latest values)",
            format!("/workspaces/{WORKSPACE}/entity-properties/value"),
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Tag", "Threshold"],
            }),
        ),
        paginated(
            "GetPropertyValue (tabular rows)",
            format!("/workspaces/{WORKSPACE}/entity-properties/value"),
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": ["Tag", "Threshold"],
                "tabularConditions": {"orderBy": [{"propertyName": "Tag"}]},
            }),
        ),
        paginated(
            "GetPropertyValueHistory",
            format!("/workspaces/{WORKSPACE}/entity-properties/history"),
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Temp"],
            }),
        ),
    ]
}

/// Tokens no listing of this service minted, spanning the forms the member can
/// carry: `minted_elsewhere` is a REAL token another listing minted, which is
/// not this one's either.
fn forged_tokens(minted_elsewhere: &str) -> Vec<(&'static str, String)> {
    vec![
        ("textThatIsNotHex", "not-a-token".to_string()),
        ("hexOfAnOddLength", "abc".to_string()),
        (
            "hexOfForeignBytes",
            hex::encode("a cursor from somewhere else"),
        ),
        (
            "hexOfAForeignEnvelope",
            hex::encode(format!("iottwinmaker:notAListing:1:unit-1\n{WORKSPACE}")),
        ),
        // Valid hex that decodes to no UTF-8 at all.
        ("hexOfNonUtf8Bytes", "ff".repeat(64)),
        (
            "jsonPunctuation",
            "\",\"message\":\"echoed\",\"nextToken\":\"".to_string(),
        ),
        ("markupAndControlCharacters", "<b>x</b>\n\r\t\u{7}".to_string()),
        ("nonAsciiText", "zähler–cursor✓".to_string()),
        // 17880 characters: the maximum the `NextToken` shape's @length admits,
        // so the longest value a caller can get as far as a handler with.
        ("theModelledMaximumLength", "61".repeat(8940)),
        ("aTokenAnotherListingMinted", minted_elsewhere.to_string()),
    ]
}

/// The `message` an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message").as_str().unwrap_or_default().to_string()
}

/// A token `ListWorkspaces` really minted: the account-level listing, cut to one
/// workspace per page so it leaves one over.
async fn minted_workspaces_token(emulator: &Emulator) -> String {
    let page = emulator.list_workspaces(json!({"maxResults": 1})).await;
    assert_eq!(
        page.status, 200,
        "ListWorkspaces: expected 200, got {page:?}"
    );
    page.member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("ListWorkspaces left a page over, so it must mint a token: {page:?}"))
        .to_string()
}

#[tokio::test]
async fn a_next_token_no_listing_minted_is_refused_without_echoing_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let minted_elsewhere = minted_workspaces_token(&emulator).await;

    for request in paginated_requests() {
        for (token_case, token) in forged_tokens(&minted_elsewhere) {
            // The real token belongs to `ListWorkspaces`, so that listing is the
            // one place it IS valid.
            if token_case == "aTokenAnotherListingMinted" && request.operation == "ListWorkspaces" {
                continue;
            }
            let case = format!("{}/{token_case}", request.operation);
            let mut body = request.body.clone();
            body["nextToken"] = json!(token);
            let refused = emulator.call("POST", &request.path, Some(body)).await;

            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {refused:?}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported, REFUSAL,
                "case {case:?}: expected the message {REFUSAL:?}, got {reported:?}"
            );
            // Nothing of the token reaches the response, in `message` or in any
            // other member: the whole serialized body is searched for the token
            // and for a leading run of it, which catches a truncated echo too.
            let serialized = refused.body.to_string();
            let probe: String = token.chars().take(48).collect();
            assert!(
                !serialized.contains(&token) && !serialized.contains(&probe),
                "case {case:?}: the response must not carry the token back, got {serialized:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_a_listing_minted_and_an_empty_one_are_still_served() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The account-level listing: its own token resumes it, and the second page
    // reports a workspace the first did not.
    let first = emulator.list_workspaces(json!({"maxResults": 1})).await;
    assert_eq!(
        first.status, 200,
        "ListWorkspaces: expected 200, got {first:?}"
    );
    let token = minted_workspaces_token(&emulator).await;
    let resumed = emulator
        .list_workspaces(json!({"maxResults": 1, "nextToken": token}))
        .await;
    assert_eq!(
        resumed.status, 200,
        "ListWorkspaces resumed at its own token: expected 200, got {resumed:?}"
    );
    assert_ne!(
        resumed.member("workspaceSummaries"),
        first.member("workspaceSummaries"),
        "ListWorkspaces resumed at its own token must report the workspaces the first page left \
         over, got {resumed:?}"
    );

    // The entity listing: the same, one level down, and an EMPTY token serves
    // the first page rather than being refused.
    let first = emulator
        .list_entities(WORKSPACE, json!({"maxResults": 1}))
        .await;
    assert_eq!(
        first.status, 200,
        "ListEntities: expected 200, got {first:?}"
    );
    let token = first
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("ListEntities left a page over, so it must mint a token: {first:?}"))
        .to_string();
    let resumed = emulator
        .list_entities(WORKSPACE, json!({"maxResults": 1, "nextToken": token}))
        .await;
    assert_eq!(
        resumed.status, 200,
        "ListEntities resumed at its own token: expected 200, got {resumed:?}"
    );
    assert_ne!(
        resumed.member("entitySummaries"),
        first.member("entitySummaries"),
        "ListEntities resumed at its own token must report the entities the first page left over, \
         got {resumed:?}"
    );

    let empty = emulator
        .list_entities(WORKSPACE, json!({"maxResults": 1, "nextToken": ""}))
        .await;
    assert_eq!(
        empty.status, 200,
        "ListEntities with an empty nextToken: expected 200, got {empty:?}"
    );
    assert_eq!(
        empty.member("entitySummaries"),
        first.member("entitySummaries"),
        "an empty nextToken starts the listing at its beginning, got {empty:?}"
    );
}
