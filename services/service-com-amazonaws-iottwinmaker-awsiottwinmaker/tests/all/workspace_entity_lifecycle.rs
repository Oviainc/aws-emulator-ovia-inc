// SPDX-License-Identifier: CC0-1.0

//! The workspace -> entity lifecycle, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and a
//! serialized body are all exercised, not just a handler body.
//!
//! The tests assert RULES over a table of inputs rather than one recorded case:
//! what a create persists is exactly what a read reports, `hasChildEntities` is
//! derived from the stored tree, a page sequence partitions the listing, and a
//! delete is observable to the next caller.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DEFAULT_PAGE_SIZE, MAX_COMPONENTS_RETURNED, MAX_ENTITIES_PER_WORKSPACE, MAX_PAGE_SIZE,
    MAX_WORKSPACES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for; it must show up in every minted ARN.
const REGION: &str = "us-west-2";
/// The account segment `engine::emulator_arn` stamps into every ARN.
const ACCOUNT: &str = "000000000000";

/// How long after `CreateEntity` the entity's `CREATING` -> `ACTIVE` transition
/// falls due, plus a margin for a loaded test host.
const ACTIVATION_WAIT: Duration = Duration::from_millis(900);

/// One dispatched response: status, the restJson1 error code (which this
/// protocol carries in `x-amzn-errortype`), and the parsed JSON body.
#[derive(Debug)]
struct Resp {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Resp {
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

/// One emulator: a shared engine with only this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests send unsigned-but-scoped requests: the `Authorization`
        // header is what routes them and names the region, not what authorises
        // them.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Resp {
        let payload = match &body {
            Some(value) => {
                bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
            }
            None => bytes::Bytes::new(),
        };
        let host = format!("api.iottwinmaker.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iottwinmaker/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(payload)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Resp {
            status,
            error_type,
            body,
        }
    }

    async fn create_workspace(&self, workspace_id: &str) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}"),
            Some(json!({})),
        )
        .await
    }

    async fn get_workspace(&self, id_or_arn: &str) -> Resp {
        self.call("GET", &format!("/workspaces/{id_or_arn}"), None).await
    }

    async fn create_entity(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities"),
            Some(body),
        )
        .await
    }

    async fn get_entity(&self, workspace_id: &str, entity_id: &str) -> Resp {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}"),
            None,
        )
        .await
    }

    async fn list_entities(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities-list"),
            Some(body),
        )
        .await
    }

    async fn delete_entity(&self, workspace_id: &str, entity_id: &str, recursive: bool) -> Resp {
        let query = if recursive { "?isRecursive=true" } else { "" };
        self.call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}{query}"),
            None,
        )
        .await
    }

    async fn delete_workspace(&self, workspace_id: &str) -> Resp {
        self.call("DELETE", &format!("/workspaces/{workspace_id}"), None)
            .await
    }
}

fn workspace_arn(workspace_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}")
}

fn entity_arn(workspace_id: &str, entity_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/entity/{entity_id}")
}

/// Percent-encode a value for use as a non-greedy `@httpLabel`, the way an SDK
/// does: `:` and `/` are reserved in a path segment, so an ARN passed as the
/// `IdOrArn` label arrives encoded.
fn encode_label(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

/// The ids reported by one `ListEntities` page, in the order the page gave them.
fn page_ids(resp: &Resp) -> Vec<String> {
    resp.member("entitySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["entityId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The page size the service actually serves for a requested `maxResults`: the
/// documented default of 25 when the caller names none, and never more than the
/// service's own ceiling.
fn effective_page_size(max_results: Option<u32>) -> usize {
    match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(size) => (size as usize).min(MAX_PAGE_SIZE),
    }
}

/// Walk every page of `ListEntities` for `workspace_id` at page size
/// `max_results`, returning the concatenated ids and the number of pages.
async fn walk_pages(
    emulator: &Emulator,
    workspace_id: &str,
    max_results: Option<u32>,
) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let resp = emulator.list_entities(workspace_id, request).await;
        assert_eq!(
            resp.status, 200,
            "page {pages} of {workspace_id}: expected 200, got {resp:?}"
        );
        pages += 1;
        ids.extend(page_ids(&resp));
        next_token = resp
            .member("nextToken")
            .as_str()
            .map(str::to_string);
        if next_token.is_none() {
            return (ids, pages);
        }
        assert!(
            pages < 64,
            "page {pages} of {workspace_id}: pagination did not terminate"
        );
    }
}

/// `8-4-4-4-12` lowercase hex, the first alternative of the `EntityId` pattern —
/// the shape of an id `CreateEntity` mints when the request names none.
fn looks_like_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

/// RULE: a workspace read back reports exactly the members the create
/// persisted, under its own id and under the ARN the create minted; a second
/// create of the same id conflicts; a delete makes the next read a 404.
///
/// The rows differ along which optional members the create carries, because
/// that is what the rule generalises over.
#[tokio::test]
async fn a_workspace_reads_back_exactly_what_was_created() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, Value)> = vec![
        ("bare", json!({})),
        ("described", json!({"description": "a demo workspace"})),
        (
            "withS3AndRole",
            json!({
                "description": "",
                "s3Location": "arn:aws:s3:::twinmaker_bucket-1",
                "role": "arn:aws:iam::000000000000:role/TwinMakerRole",
            }),
        ),
        (
            "withTags",
            json!({"description": "tagged", "tags": {"team": "plant", "env": "test"}}),
        ),
    ];

    for (workspace_id, request) in &rows {
        let created = emulator
            .call(
                "POST",
                &format!("/workspaces/{workspace_id}"),
                Some(request.clone()),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {workspace_id:?}: expected 200 from CreateWorkspace, got {created:?}"
        );
        assert_eq!(
            created.member("arn").as_str(),
            Some(workspace_arn(workspace_id).as_str()),
            "case {workspace_id:?}: expected the workspace ARN, got {:?}",
            created.member("arn")
        );
        let creation_date_time = created.member("creationDateTime").clone();
        assert!(
            creation_date_time.is_number(),
            "case {workspace_id:?}: expected a creationDateTime timestamp, got {creation_date_time:?}"
        );

        for lookup in [workspace_id.to_string(), workspace_arn(workspace_id)] {
            let read = emulator.get_workspace(&encode_label(&lookup)).await;
            assert_eq!(
                read.status, 200,
                "case {workspace_id:?} read by {lookup:?}: expected 200, got {read:?}"
            );
            assert_eq!(
                read.member("workspaceId").as_str(),
                Some(*workspace_id),
                "case {workspace_id:?} read by {lookup:?}: wrong workspaceId, got {:?}",
                read.member("workspaceId")
            );
            assert_eq!(
                read.member("arn").as_str(),
                Some(workspace_arn(workspace_id).as_str()),
                "case {workspace_id:?} read by {lookup:?}: wrong arn, got {:?}",
                read.member("arn")
            );
            assert_eq!(
                read.member("creationDateTime"),
                &creation_date_time,
                "case {workspace_id:?} read by {lookup:?}: creationDateTime drifted, got {:?}",
                read.member("creationDateTime")
            );
            assert!(
                read.member("updateDateTime").is_number(),
                "case {workspace_id:?} read by {lookup:?}: expected an updateDateTime, got {:?}",
                read.member("updateDateTime")
            );
            for member in ["description", "s3Location", "role"] {
                let expected = request.get(member).cloned();
                let actual = read.body.get(member).cloned();
                assert_eq!(
                    actual, expected,
                    "case {workspace_id:?} read by {lookup:?}: {member} did not round-trip, got {actual:?}"
                );
            }
        }

        let again = emulator
            .call(
                "POST",
                &format!("/workspaces/{workspace_id}"),
                Some(request.clone()),
            )
            .await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {workspace_id:?}: re-creating the id must conflict, got {again:?}"
        );
    }

    for (workspace_id, _) in &rows {
        let deleted = emulator.delete_workspace(workspace_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {workspace_id:?}: expected 200 from DeleteWorkspace, got {deleted:?}"
        );
        let gone = emulator.get_workspace(workspace_id).await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {workspace_id:?}: a deleted workspace must be gone, got {gone:?}"
        );
        let gone_by_arn = emulator
            .get_workspace(&encode_label(&workspace_arn(workspace_id)))
            .await;
        assert_eq!(
            gone_by_arn.status, 404,
            "case {workspace_id:?}: its ARN must stop resolving too, got {gone_by_arn:?}"
        );
    }
}

/// RULE: an entity read back reports exactly what the create persisted — name,
/// description, parent, components and their property definitions and values —
/// and an id the caller omitted is minted as a uuid.
///
/// The rows differ along which optional members the create carries and whether
/// the caller supplies the id, because that is what the rule generalises over.
#[tokio::test]
async fn an_entity_reads_back_exactly_what_was_created() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("ws").await.status, 200);

    let rows: Vec<(&str, Value)> = vec![
        ("minimal", json!({"entityId": "minimal", "entityName": "Minimal"})),
        (
            "described",
            json!({
                "entityId": "described",
                "entityName": "A described entity",
                "description": "a pump",
            }),
        ),
        (
            "withComponent",
            json!({
                "entityId": "withComponent",
                "entityName": "Entity with a component",
                "components": {
                    "sensor": {
                        "componentTypeId": "com.example.sensor",
                        "description": "a sensor component",
                        "properties": {
                            "serial": {
                                "definition": {
                                    "dataType": {"type": "STRING"},
                                    "isExternalId": true,
                                },
                                "value": {"stringValue": "SN-42"},
                            },
                        },
                        "propertyGroups": {
                            "tabular": {
                                "groupType": "TABULAR",
                                "propertyNames": ["serial"],
                            },
                        },
                    },
                },
            }),
        ),
        ("generatedId", json!({"entityName": "No id supplied"})),
    ];

    for (case, request) in &rows {
        let created = emulator.create_entity("ws", request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );
        let entity_id = created
            .member("entityId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        match request.get("entityId").and_then(Value::as_str) {
            Some(expected) => assert_eq!(
                entity_id, expected,
                "case {case:?}: CreateEntity must keep the supplied entityId, got {entity_id:?}"
            ),
            None => assert!(
                looks_like_uuid(&entity_id),
                "case {case:?}: a minted entityId must be a uuid, got {entity_id:?}"
            ),
        }
        assert_eq!(
            created.member("arn").as_str(),
            Some(entity_arn("ws", &entity_id).as_str()),
            "case {case:?}: wrong entity ARN, got {:?}",
            created.member("arn")
        );
        assert_eq!(
            created.member("state").as_str(),
            Some("CREATING"),
            "case {case:?}: a fresh entity is returned CREATING, got {:?}",
            created.member("state")
        );

        let read = emulator.get_entity("ws", &entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetEntity, got {read:?}"
        );
        assert_eq!(
            read.member("entityId").as_str(),
            Some(entity_id.as_str()),
            "case {case:?}: wrong entityId, got {:?}",
            read.member("entityId")
        );
        assert_eq!(
            read.member("entityName").as_str(),
            request.get("entityName").and_then(Value::as_str),
            "case {case:?}: entityName did not round-trip, got {:?}",
            read.member("entityName")
        );
        assert_eq!(
            read.body.get("description").cloned(),
            request.get("description").cloned(),
            "case {case:?}: description did not round-trip, got {:?}",
            read.body.get("description")
        );
        assert_eq!(
            read.member("workspaceId").as_str(),
            Some("ws"),
            "case {case:?}: wrong workspaceId, got {:?}",
            read.member("workspaceId")
        );
        assert_eq!(
            read.member("parentEntityId").as_str(),
            Some("$ROOT"),
            "case {case:?}: an entity created with no parent is a $ROOT child, got {:?}",
            read.member("parentEntityId")
        );
        assert_eq!(
            read.member("hasChildEntities").as_bool(),
            Some(false),
            "case {case:?}: a childless entity must report false, got {:?}",
            read.member("hasChildEntities")
        );
        assert_eq!(
            read.member("creationDateTime"),
            created.member("creationDateTime"),
            "case {case:?}: creationDateTime drifted, got {:?}",
            read.member("creationDateTime")
        );
        assert!(
            read.member("updateDateTime").is_number(),
            "case {case:?}: expected an updateDateTime, got {:?}",
            read.member("updateDateTime")
        );

        let expected_components = request.get("components");
        let read_components = read.member("components");
        match expected_components {
            None => assert_eq!(
                read_components.as_object().map(serde_json::Map::len),
                Some(0),
                "case {case:?}: an entity created with no components must report none, got {read_components:?}"
            ),
            Some(components) => {
                for (name, component) in components.as_object().expect("a components object") {
                    let actual = &read_components[name];
                    assert_eq!(
                        actual["componentName"].as_str(),
                        Some(name.as_str()),
                        "case {case:?}: component {name:?} must report its name, got {actual:?}"
                    );
                    assert_eq!(
                        actual["componentTypeId"], component["componentTypeId"],
                        "case {case:?}: component {name:?} componentTypeId did not round-trip, got {actual:?}"
                    );
                    assert_eq!(
                        actual["description"], component["description"],
                        "case {case:?}: component {name:?} description did not round-trip, got {actual:?}"
                    );
                    for (property, expected) in component["properties"]
                        .as_object()
                        .expect("a properties object")
                    {
                        let got = &actual["properties"][property];
                        assert_eq!(
                            got["value"], expected["value"],
                            "case {case:?}: property {property:?} value did not round-trip, got {got:?}"
                        );
                        assert_eq!(
                            got["definition"]["dataType"]["type"],
                            expected["definition"]["dataType"]["type"],
                            "case {case:?}: property {property:?} dataType did not round-trip, got {got:?}"
                        );
                        assert_eq!(
                            got["definition"]["isExternalId"],
                            expected["definition"]["isExternalId"],
                            "case {case:?}: property {property:?} isExternalId did not round-trip, got {got:?}"
                        );
                    }
                    for (group, expected) in component["propertyGroups"]
                        .as_object()
                        .expect("a propertyGroups object")
                    {
                        let got = &actual["propertyGroups"][group];
                        assert_eq!(
                            got["groupType"], expected["groupType"],
                            "case {case:?}: group {group:?} groupType did not round-trip, got {got:?}"
                        );
                        assert_eq!(
                            got["propertyNames"], expected["propertyNames"],
                            "case {case:?}: group {group:?} propertyNames did not round-trip, got {got:?}"
                        );
                    }
                }
            }
        }
    }
}

/// RULE: a resource the service returns in a non-terminal state owes the clock
/// its transition — a fresh entity is `CREATING` when read straight back and
/// `ACTIVE` once its activation has fallen due — and every read reports the same
/// state for the same entity.
#[tokio::test]
async fn a_fresh_entity_moves_from_creating_to_active_on_the_clock() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("ws").await.status, 200);

    // The rows differ in how many entities are in flight at once: the state is
    // per entity, so each must transition on its own due instant.
    for count in [1usize, 3] {
        let workspace = format!("ws{count}");
        assert_eq!(emulator.create_workspace(&workspace).await.status, 200);
        let ids: Vec<String> = (0..count).map(|i| format!("e{i}")).collect();
        for entity_id in &ids {
            let created = emulator
                .create_entity(
                    &workspace,
                    json!({"entityId": entity_id, "entityName": entity_id}),
                )
                .await;
            assert_eq!(
                created.member("state").as_str(),
                Some("CREATING"),
                "case {count} entity {entity_id:?}: CreateEntity must report CREATING, got {created:?}"
            );
        }

        for entity_id in &ids {
            let read = emulator.get_entity(&workspace, entity_id).await;
            assert_eq!(
                read.member("status")["state"].as_str(),
                Some("CREATING"),
                "case {count} entity {entity_id:?}: a read straight after the create must still \
                 see CREATING, got {read:?}"
            );
        }
        let listed = emulator.list_entities(&workspace, json!({})).await;
        for summary in listed.member("entitySummaries").as_array().expect("summaries") {
            assert_eq!(
                summary["status"]["state"].as_str(),
                Some("CREATING"),
                "case {count}: ListEntities must agree with GetEntity, got {summary:?}"
            );
        }

        tokio::time::sleep(ACTIVATION_WAIT).await;

        for entity_id in &ids {
            let read = emulator.get_entity(&workspace, entity_id).await;
            assert_eq!(
                read.member("status")["state"].as_str(),
                Some("ACTIVE"),
                "case {count} entity {entity_id:?}: the entity must be ACTIVE once its \
                 activation fell due, got {read:?}"
            );
        }
        let listed = emulator.list_entities(&workspace, json!({})).await;
        for summary in listed.member("entitySummaries").as_array().expect("summaries") {
            assert_eq!(
                summary["status"]["state"].as_str(),
                Some("ACTIVE"),
                "case {count}: ListEntities must agree with GetEntity, got {summary:?}"
            );
        }
    }
}

/// RULE: `hasChildEntities` is derived from the stored tree — true exactly while
/// some stored entity names this one as its parent — so it tracks a child being
/// added and removed without ever being written down.
///
/// The rows differ in the number of children, including none.
#[tokio::test]
async fn has_child_entities_is_derived_from_the_stored_tree() {
    let emulator = Emulator::new();

    for children in [0usize, 1, 3] {
        let workspace = format!("tree{children}");
        assert_eq!(emulator.create_workspace(&workspace).await.status, 200);
        assert_eq!(
            emulator
                .create_entity(&workspace, json!({"entityId": "parent", "entityName": "Parent"}))
                .await
                .status,
            200
        );
        let child_ids: Vec<String> = (0..children).map(|i| format!("child{i}")).collect();
        for child in &child_ids {
            let created = emulator
                .create_entity(
                    &workspace,
                    json!({
                        "entityId": child,
                        "entityName": child,
                        "parentEntityId": "parent",
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {children}: creating {child:?} must succeed, got {created:?}"
            );
            let read = emulator.get_entity(&workspace, child).await;
            assert_eq!(
                read.member("parentEntityId").as_str(),
                Some("parent"),
                "case {children}: {child:?} must report its parent, got {read:?}"
            );
            assert_eq!(
                read.member("hasChildEntities").as_bool(),
                Some(false),
                "case {children}: a leaf {child:?} must report false, got {read:?}"
            );
        }

        let parent = emulator.get_entity(&workspace, "parent").await;
        assert_eq!(
            parent.member("hasChildEntities").as_bool(),
            Some(children > 0),
            "case {children}: hasChildEntities must follow the stored children, got {parent:?}"
        );

        // Remove them one at a time: the flag is false again exactly when the
        // last child is gone.
        for (removed, child) in child_ids.iter().enumerate() {
            assert_eq!(
                emulator.delete_entity(&workspace, child, false).await.status,
                200,
                "case {children}: deleting the leaf {child:?} must succeed"
            );
            let parent = emulator.get_entity(&workspace, "parent").await;
            let still_has = removed + 1 < children;
            assert_eq!(
                parent.member("hasChildEntities").as_bool(),
                Some(still_has),
                "case {children}: after removing {} of {children} children, expected \
                 hasChildEntities={still_has}, got {parent:?}",
                removed + 1
            );
        }
    }
}

/// RULE: `ListEntities` lists every entity of the workspace, and the page
/// sequence partitions that listing — concatenating the pages yields each stored
/// entity exactly once, in `entityId` order, and only the last page omits the
/// `nextToken`.
///
/// The rows differ along the entity count and the requested page size, including
/// a page size larger than the listing and no page size at all.
#[tokio::test]
async fn list_entities_pages_partition_the_workspace() {
    let emulator = Emulator::new();

    let rows: Vec<(usize, Option<u32>)> = vec![
        (0, Some(1)),
        (1, Some(1)),
        (5, Some(2)),
        (7, Some(3)),
        (4, Some(200)),
        (4, None),
    ];

    for (total, max_results) in rows {
        let workspace = format!("page{total}x{}", max_results.unwrap_or(0));
        assert_eq!(emulator.create_workspace(&workspace).await.status, 200);
        let mut expected: Vec<String> = (0..total).map(|i| format!("e{i:02}")).collect();
        for entity_id in &expected {
            assert_eq!(
                emulator
                    .create_entity(
                        &workspace,
                        json!({"entityId": entity_id, "entityName": entity_id}),
                    )
                    .await
                    .status,
                200,
                "case ({total}, {max_results:?}): creating {entity_id:?} must succeed"
            );
        }
        expected.sort();

        let (ids, pages) = walk_pages(&emulator, &workspace, max_results).await;
        assert_eq!(
            ids, expected,
            "case ({total}, {max_results:?}): expected {expected:?} across {pages} pages, got {ids:?}"
        );
        let expected_pages = total.div_ceil(effective_page_size(max_results)).max(1);
        assert_eq!(
            pages, expected_pages,
            "case ({total}, {max_results:?}): expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: the page size is the one the service documents and bounds — a request
/// that names no `maxResults` is served the documented default, an explicit
/// value below the documented minimum of 1 is refused, a value above what the
/// service serves is capped at [`MAX_PAGE_SIZE`], and a value past the member's
/// published range is refused outright — with a `nextToken` whenever rows are
/// left over.
///
/// The rows are the published bounds of `maxResults` (1 and 200), one below the
/// bottom (0) and one past the top (201), values inside it, and the request that
/// names none.
#[tokio::test]
async fn the_page_size_is_the_documented_one_and_is_bounded() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("capped").await.status, 200);

    // The documented numbers themselves, pinned as literals so the rows below
    // cannot drift with the implementation: the `maxResults` member documents
    // "The default is 25" and the model's own range tops out at 200.
    assert_eq!(
        DEFAULT_PAGE_SIZE, 25,
        "the documented default page size is 25, got {DEFAULT_PAGE_SIZE}"
    );
    assert_eq!(
        MAX_PAGE_SIZE, 200,
        "the documented maximum page size is 200, got {MAX_PAGE_SIZE}"
    );

    // More entities than the service will serve in one page, so every row below
    // is capped by the service rather than by the stored count.
    let total = MAX_PAGE_SIZE + 5;
    let all_ids: Vec<String> = (0..total).map(|i| format!("e{i:03}")).collect();
    for entity_id in &all_ids {
        assert_eq!(
            emulator
                .create_entity(
                    "capped",
                    json!({"entityId": entity_id, "entityName": entity_id}),
                )
                .await
                .status,
            200,
            "creating {entity_id:?} must succeed"
        );
    }

    // (what the caller asks for, how many summaries the service serves)
    let rows: Vec<(Option<u32>, usize)> = vec![
        (None, DEFAULT_PAGE_SIZE),
        (Some(1), 1),
        (Some(7), 7),
        (Some(DEFAULT_PAGE_SIZE as u32), DEFAULT_PAGE_SIZE),
        (Some(MAX_PAGE_SIZE as u32), MAX_PAGE_SIZE),
    ];

    for (max_results, expected_len) in rows {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        let page = emulator.list_entities("capped", request).await;
        assert_eq!(
            page.status, 200,
            "case {max_results:?}: expected 200, got {page:?}"
        );
        let ids = page_ids(&page);
        assert_eq!(
            ids.len(),
            expected_len,
            "case {max_results:?}: expected {expected_len} summaries, got {}",
            ids.len()
        );
        assert_eq!(
            ids.as_slice(),
            &all_ids[..expected_len],
            "case {max_results:?}: the page must start the listing, got {ids:?}"
        );
        let token = page
            .member("nextToken")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !token.is_empty(),
            "case {max_results:?}: a listing with rows left over must answer a token, got {page:?}"
        );
        let resumed = emulator
            .list_entities("capped", json!({"maxResults": 1, "nextToken": token}))
            .await;
        assert_eq!(
            page_ids(&resumed).as_slice(),
            &all_ids[expected_len..=expected_len],
            "case {max_results:?}: the token must resume where the page stopped, got {resumed:?}"
        );
    }

    // Below the documented minimum of 1, and one past the published top of the
    // range (where the modelled constraint refuses it before the handler runs).
    for past_bound in [0u32, 201] {
        let refused = emulator
            .list_entities("capped", json!({"maxResults": past_bound}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case Some({past_bound}): a maxResults outside the documented range must be refused, \
             got {refused:?}"
        );
    }

    // A token minted for another workspace is not resumable here.
    assert_eq!(emulator.create_workspace("other").await.status, 200);
    for entity_id in ["aa", "bb"] {
        assert_eq!(
            emulator
                .create_entity("other", json!({"entityId": entity_id, "entityName": entity_id}))
                .await
                .status,
            200
        );
    }
    let foreign_token = emulator
        .list_entities("other", json!({"maxResults": 1}))
        .await
        .member("nextToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!foreign_token.is_empty(), "the other listing must paginate");
    let foreign = emulator
        .list_entities("capped", json!({"nextToken": foreign_token}))
        .await;
    assert_eq!(
        (foreign.status, foreign.error_type.as_deref()),
        (400, Some("ValidationException")),
        "a token minted for another workspace must be refused, got {foreign:?}"
    );

    // And every page together still partitions the listing.
    let (ids, pages) = walk_pages(&emulator, "capped", None).await;
    assert_eq!(
        (ids.len(), pages),
        (total, total.div_ceil(DEFAULT_PAGE_SIZE)),
        "the capped pages must still cover the whole listing, got {} ids over {pages} pages",
        ids.len()
    );
    assert_eq!(
        ids, all_ids,
        "the capped pages must cover the listing in order"
    );
}

/// RULE: `GetEntity` truncates the component map at the limit the
/// `areAllComponentsReturned` flag documents, and the flag reports whether it
/// had to.
///
/// The rows straddle the limit: below it, exactly at it, and past it.
#[tokio::test]
async fn get_entity_truncates_components_at_the_documented_limit() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("trunc").await.status, 200);

    // The documented number, pinned as a literal: "The maximum number of
    // components returned is 30" (`areAllComponentsReturned`).
    assert_eq!(
        MAX_COMPONENTS_RETURNED, 30,
        "the documented component limit is 30, got {MAX_COMPONENTS_RETURNED}"
    );

    for count in [1usize, MAX_COMPONENTS_RETURNED, MAX_COMPONENTS_RETURNED + 1] {
        let entity_id = format!("e{count:03}");
        let mut components = serde_json::Map::new();
        for i in 0..count {
            components.insert(
                format!("c{i:03}"),
                json!({"componentTypeId": "com.example.sensor"}),
            );
        }
        let created = emulator
            .create_entity(
                "trunc",
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": Value::Object(components),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {count} components: CreateEntity must succeed, got {created:?}"
        );

        let read = emulator.get_entity("trunc", &entity_id).await;
        let returned = read
            .member("components")
            .as_object()
            .map(serde_json::Map::len)
            .unwrap_or_default();
        let expected = count.min(MAX_COMPONENTS_RETURNED);
        assert_eq!(
            returned, expected,
            "case {count} components: expected {expected} returned, got {returned}"
        );
        assert_eq!(
            read.member("areAllComponentsReturned").as_bool(),
            Some(count <= MAX_COMPONENTS_RETURNED),
            "case {count} components: the completeness flag must report the truncation, got {:?}",
            read.member("areAllComponentsReturned")
        );
        // Truncation is in component-name order, so which ones survive is
        // deterministic.
        for i in 0..expected {
            let name = format!("c{i:03}");
            assert!(
                read.member("components").get(&name).is_some(),
                "case {count} components: {name:?} must be in the returned map, got {read:?}"
            );
        }
    }
}

/// RULE: the store has a declared ceiling — `CreateWorkspace` and `CreateEntity`
/// answer the `ServiceQuotaExceededException` (402) they model once it is
/// reached, rather than growing without bound.
#[tokio::test]
async fn creating_past_the_declared_ceiling_is_a_service_quota_error() {
    let emulator = Emulator::new();

    for i in 0..MAX_WORKSPACES {
        let workspace = format!("ws{i:03}");
        assert_eq!(
            emulator.create_workspace(&workspace).await.status,
            200,
            "creating workspace {i} of {MAX_WORKSPACES} must succeed"
        );
    }
    let past = emulator.create_workspace("oneTooMany").await;
    assert_eq!(
        (past.status, past.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "case \"workspace {} + 1\": expected the modelled quota error, got {past:?}",
        MAX_WORKSPACES
    );

    // The ceiling refuses the create; it does not disturb what is stored.
    assert_eq!(
        emulator.get_workspace("ws000").await.status,
        200,
        "a refused create must leave the store intact"
    );
    assert_eq!(
        emulator.delete_workspace("ws000").await.status,
        200,
        "deleting one must free a slot"
    );
    assert_eq!(
        emulator.create_workspace("oneTooMany").await.status,
        200,
        "the create must succeed once a slot is free"
    );

    // The same rule on the other ceiling: entities within one workspace.
    let crowded = "ws001";
    for i in 0..MAX_ENTITIES_PER_WORKSPACE {
        let entity_id = format!("e{i:04}");
        assert_eq!(
            emulator
                .create_entity(
                    crowded,
                    json!({"entityId": entity_id, "entityName": entity_id}),
                )
                .await
                .status,
            200,
            "creating entity {i} of {MAX_ENTITIES_PER_WORKSPACE} must succeed"
        );
    }
    let past = emulator
        .create_entity(
            crowded,
            json!({"entityId": "oneTooMany", "entityName": "One too many"}),
        )
        .await;
    assert_eq!(
        (past.status, past.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "case \"entity {} + 1\": expected the modelled quota error, got {past:?}",
        MAX_ENTITIES_PER_WORKSPACE
    );
    assert_eq!(
        emulator.get_entity(crowded, "oneTooMany").await.status,
        404,
        "a refused create must store nothing"
    );
    assert_eq!(
        emulator.delete_entity(crowded, "e0000", false).await.status,
        200,
        "deleting one must free a slot"
    );
    assert_eq!(
        emulator
            .create_entity(
                crowded,
                json!({"entityId": "oneTooMany", "entityName": "One too many"}),
            )
            .await
            .status,
        200,
        "the create must succeed once a slot is free"
    );
}

/// RULE: the one accepted filter selects the entities it names — by parent, by
/// the component type one of the entity's components has, or by the external id
/// one of its component properties carries — and a request carrying more than
/// one filter is refused, because the member documents that only one object is
/// a valid input.
#[tokio::test]
async fn list_entities_honours_the_single_accepted_filter() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("filters").await.status, 200);

    assert_eq!(
        emulator
            .create_entity("filters", json!({"entityId": "root1", "entityName": "Root 1"}))
            .await
            .status,
        200
    );
    assert_eq!(
        emulator
            .create_entity(
                "filters",
                json!({
                    "entityId": "child1",
                    "entityName": "Child 1",
                    "parentEntityId": "root1",
                    "components": {
                        "sensor": {
                            "componentTypeId": "com.example.sensor",
                            "properties": {
                                "tag": {
                                    "definition": {"dataType": {"type": "STRING"}, "isExternalId": true},
                                    "value": {"stringValue": "EXT-1"},
                                },
                            },
                        },
                    },
                }),
            )
            .await
            .status,
        200
    );
    assert_eq!(
        emulator
            .create_entity(
                "filters",
                json!({
                    "entityId": "child2",
                    "entityName": "Child 2",
                    "parentEntityId": "root1",
                    "components": {
                        "alarm": {
                            "componentTypeId": "com.example.alarm",
                            "properties": {
                                "tag": {
                                    "definition": {"dataType": {"type": "STRING"}, "isExternalId": true},
                                    "value": {"stringValue": "EXT-2"},
                                },
                            },
                        },
                    },
                }),
            )
            .await
            .status,
        200
    );

    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter", json!({}), vec!["child1", "child2", "root1"]),
        (
            "parentEntityId=$ROOT",
            json!({"filters": [{"parentEntityId": "$ROOT"}]}),
            vec!["root1"],
        ),
        (
            "parentEntityId=root1",
            json!({"filters": [{"parentEntityId": "root1"}]}),
            vec!["child1", "child2"],
        ),
        (
            "parentEntityId names no entity",
            json!({"filters": [{"parentEntityId": "child1"}]}),
            vec![],
        ),
        (
            "componentTypeId=com.example.sensor",
            json!({"filters": [{"componentTypeId": "com.example.sensor"}]}),
            vec!["child1"],
        ),
        (
            "componentTypeId=com.example.alarm",
            json!({"filters": [{"componentTypeId": "com.example.alarm"}]}),
            vec!["child2"],
        ),
        (
            "componentTypeId matches nothing",
            json!({"filters": [{"componentTypeId": "com.example.absent"}]}),
            vec![],
        ),
        (
            "externalId=EXT-2",
            json!({"filters": [{"externalId": "EXT-2"}]}),
            vec!["child2"],
        ),
        (
            "externalId matches nothing",
            json!({"filters": [{"externalId": "EXT-9"}]}),
            vec![],
        ),
    ];

    for (case, request, expected) in rows {
        let listed = emulator.list_entities("filters", request).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {listed:?}"
        );
        assert_eq!(
            page_ids(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    let two = emulator
        .list_entities(
            "filters",
            json!({"filters": [{"parentEntityId": "root1"}, {"componentTypeId": "com.example.sensor"}]}),
        )
        .await;
    assert_eq!(
        (two.status, two.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"two filters\": only one filter object is a valid input, got {two:?}"
    );
}

/// RULE: a delete is observable to the next caller — the entity is gone from
/// `GetEntity` and from `ListEntities` — and an entity that still has children
/// is only removable with `isRecursive`, which removes the whole subtree below
/// it.
#[tokio::test]
async fn deleting_an_entity_is_observable_and_guards_its_children() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("del").await.status, 200);

    // A three-level chain, so `isRecursive` has a grandchild to reach.
    for (entity_id, parent) in [
        ("grandparent", None),
        ("parent", Some("grandparent")),
        ("leaf", Some("parent")),
    ] {
        let mut request = json!({"entityId": entity_id, "entityName": entity_id});
        if let Some(parent) = parent {
            request["parentEntityId"] = json!(parent);
        }
        assert_eq!(
            emulator.create_entity("del", request).await.status,
            200,
            "creating {entity_id:?} must succeed"
        );
    }

    for guarded in ["grandparent", "parent"] {
        let refused = emulator.delete_entity("del", guarded, false).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {guarded:?}: deleting a parent without isRecursive must be refused, got {refused:?}"
        );
        assert_eq!(
            emulator.get_entity("del", guarded).await.status,
            200,
            "case {guarded:?}: a refused delete must change nothing"
        );
    }
    let (ids, _) = walk_pages(&emulator, "del", None).await;
    assert_eq!(
        ids,
        vec![
            "grandparent".to_string(),
            "leaf".to_string(),
            "parent".to_string()
        ],
        "a refused delete must leave the listing untouched, got {ids:?}"
    );

    let deleted = emulator.delete_entity("del", "grandparent", true).await;
    assert_eq!(
        deleted.status, 200,
        "case \"recursive\": expected 200, got {deleted:?}"
    );
    assert_eq!(
        deleted.member("state").as_str(),
        Some("DELETING"),
        "case \"recursive\": DeleteEntity reports the deleted entity's state, got {deleted:?}"
    );
    for entity_id in ["grandparent", "parent", "leaf"] {
        let gone = emulator.get_entity("del", entity_id).await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case \"recursive\": {entity_id:?} must be gone, got {gone:?}"
        );
    }
    let (ids, pages) = walk_pages(&emulator, "del", None).await;
    assert_eq!(
        (ids.as_slice(), pages),
        ([].as_slice(), 1),
        "case \"recursive\": the listing must be empty, got {ids:?}"
    );
}

/// RULE: a workspace's contents are deleted before the workspace — a workspace
/// that still holds entities is refused, and once it is empty the delete takes
/// and the workspace is gone.
///
/// The rows differ in how many entities stand in the way, including none.
#[tokio::test]
async fn a_workspace_is_only_deletable_once_it_is_empty() {
    let emulator = Emulator::new();

    for count in [0usize, 1, 4] {
        assert_eq!(emulator.create_workspace("owned").await.status, 200);
        let entity_ids: Vec<String> = (0..count).map(|i| format!("e{i:02}")).collect();
        for entity_id in &entity_ids {
            assert_eq!(
                emulator
                    .create_entity(
                        "owned",
                        json!({"entityId": entity_id, "entityName": entity_id}),
                    )
                    .await
                    .status,
                200
            );
        }
        let (ids, _) = walk_pages(&emulator, "owned", None).await;
        assert_eq!(
            ids, entity_ids,
            "case {count}: expected {entity_ids:?} before the delete, got {ids:?}"
        );

        if count > 0 {
            let refused = emulator.delete_workspace("owned").await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {count}: a workspace holding entities must not be deletable, got {refused:?}"
            );
            assert_eq!(
                emulator.get_workspace("owned").await.status,
                200,
                "case {count}: a refused delete must change nothing"
            );
            assert_eq!(
                emulator.get_entity("owned", &entity_ids[0]).await.status,
                200,
                "case {count}: a refused delete must leave the entities in place"
            );
            for entity_id in &entity_ids {
                assert_eq!(
                    emulator.delete_entity("owned", entity_id, false).await.status,
                    200,
                    "case {count}: emptying the workspace must succeed"
                );
            }
        }

        assert_eq!(
            emulator.delete_workspace("owned").await.status,
            200,
            "case {count}: an empty workspace must be deletable"
        );
        let gone = emulator.get_workspace("owned").await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {count}: the deleted workspace must be gone, got {gone:?}"
        );

        // Re-created under the same id, it starts empty: nothing of the deleted
        // workspace survived.
        assert_eq!(emulator.create_workspace("owned").await.status, 200);
        let (ids, _) = walk_pages(&emulator, "owned", None).await;
        assert_eq!(
            ids,
            Vec::<String>::new(),
            "case {count}: a re-created workspace must start empty, got {ids:?}"
        );
        assert_eq!(emulator.delete_workspace("owned").await.status, 200);
    }
}

/// One refusal case: a request to make, and the status and error code the model
/// declares for it.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    status: u16,
    error_type: &'static str,
}

impl RefusalCase {
    fn new(
        case: &'static str,
        method: &'static str,
        path: &'static str,
        body: Option<Value>,
        status: u16,
        error_type: &'static str,
    ) -> Self {
        Self {
            case,
            method,
            path,
            body,
            status,
            error_type,
        }
    }
}

/// RULE: every refusal is the status and error code the model declares for that
/// operation — a resource that does not exist is a `ResourceNotFoundException`
/// (404) on the operations that declare one and a `ValidationException` (400) on
/// the ones that do not, a taken id is a `ConflictException` (409), and a request
/// the service cannot honour is a `ValidationException` (400).
#[tokio::test]
async fn refusals_carry_the_status_and_code_the_model_declares() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace("errs").await.status, 200);
    assert_eq!(
        emulator
            .create_entity("errs", json!({"entityId": "taken", "entityName": "Taken"}))
            .await
            .status,
        200
    );

    let rows = vec![
        RefusalCase::new(
            "GetWorkspace on an unknown workspace",
            "GET",
            "/workspaces/absent",
            None,
            404,
            "ResourceNotFoundException",
        ),
        RefusalCase::new(
            "DeleteWorkspace on an unknown workspace",
            "DELETE",
            "/workspaces/absent",
            None,
            404,
            "ResourceNotFoundException",
        ),
        RefusalCase::new(
            "DeleteWorkspace on a workspace that still holds entities",
            "DELETE",
            "/workspaces/errs",
            None,
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "GetEntity on an unknown workspace",
            "GET",
            "/workspaces/absent/entities/taken",
            None,
            404,
            "ResourceNotFoundException",
        ),
        RefusalCase::new(
            "GetEntity on an unknown entity",
            "GET",
            "/workspaces/errs/entities/absent",
            None,
            404,
            "ResourceNotFoundException",
        ),
        RefusalCase::new(
            "DeleteEntity on an unknown entity",
            "DELETE",
            "/workspaces/errs/entities/absent",
            None,
            404,
            "ResourceNotFoundException",
        ),
        RefusalCase::new(
            "CreateEntity on an unknown workspace",
            "POST",
            "/workspaces/absent/entities",
            Some(json!({"entityId": "xx", "entityName": "X"})),
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "CreateEntity with a taken entityId",
            "POST",
            "/workspaces/errs/entities",
            Some(json!({"entityId": "taken", "entityName": "Again"})),
            409,
            "ConflictException",
        ),
        RefusalCase::new(
            "CreateEntity with an unknown parent",
            "POST",
            "/workspaces/errs/entities",
            Some(json!({"entityId": "orphan", "entityName": "Orphan", "parentEntityId": "absent"})),
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "CreateEntity naming itself as its parent",
            "POST",
            "/workspaces/errs/entities",
            Some(json!({"entityId": "selfparent", "entityName": "Self", "parentEntityId": "selfparent"})),
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "CreateEntity with compositeComponents",
            "POST",
            "/workspaces/errs/entities",
            Some(json!({
                "entityId": "composite",
                "entityName": "Composite",
                "compositeComponents": {"sensor/inner": {"description": "nested"}},
            })),
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "CreateEntity with a definition that names no dataType",
            "POST",
            "/workspaces/errs/entities",
            Some(json!({
                "entityId": "undefined",
                "entityName": "Undefined",
                "components": {
                    "sensor": {
                        "componentTypeId": "com.example.sensor",
                        "properties": {"p": {"definition": {"isTimeSeries": true}}},
                    },
                },
            })),
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "ListEntities on an unknown workspace",
            "POST",
            "/workspaces/absent/entities-list",
            Some(json!({})),
            400,
            "ValidationException",
        ),
        RefusalCase::new(
            "ListEntities with a token this service never minted",
            "POST",
            "/workspaces/errs/entities-list",
            Some(json!({"nextToken": "not-a-token"})),
            400,
            "ValidationException",
        ),
    ];

    for row in rows {
        let RefusalCase {
            case,
            method,
            path,
            body,
            status,
            error_type,
        } = row;
        let resp = emulator.call(method, path, body).await;
        assert_eq!(
            (resp.status, resp.error_type.as_deref()),
            (status, Some(error_type)),
            "case {case:?}: expected {status} {error_type}, got {resp:?}"
        );
        assert!(
            resp.member("message").is_string(),
            "case {case:?}: a refusal must say why, got {resp:?}"
        );
    }

    // The entity that each refusal above declined to create must not exist.
    for entity_id in ["orphan", "composite", "undefined", "xx", "selfparent"] {
        let gone = emulator.get_entity("errs", entity_id).await;
        assert_eq!(
            gone.status, 404,
            "a refused CreateEntity must store nothing: {entity_id:?} exists, got {gone:?}"
        );
    }
}
