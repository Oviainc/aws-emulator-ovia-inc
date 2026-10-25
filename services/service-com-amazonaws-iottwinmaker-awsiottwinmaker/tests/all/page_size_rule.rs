// SPDX-License-Identifier: CC0-1.0

//! The page-size rule EVERY paginated operation of this service shares, driven
//! end to end over HTTP through each of them.
//!
//! RULE under test: a request naming no `maxResults` is served the page size that
//! operation's own `maxResults` member documents as its default — never every row
//! it could report — a value below the minimum that member documents is refused
//! with HTTP 400 `ValidationException` (and served as a page of no rows where the
//! member documents a minimum of zero), and any other value is served as asked
//! for, capped at the most this service ever serves; a `nextToken` comes back
//! exactly when rows are left over.
//!
//! The rows span the operations the rule holds for — the account-wide listings,
//! the ones scoped to a workspace, the one scoped to an entity, the tag listing
//! and the property read — and, for each, the page sizes the rule generalises
//! over: none asked for, the published bounds of `maxResults` (0 and 200), one
//! past that top (201), the operation's own default, and sizes inside the range.
//!
//! Each case drives the real request path (`MultiServiceRouter::dispatch`, the
//! entry the served emulator uses), so the route, the labels, the status code and
//! the serialized body are all exercised, not just a handler body.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::list_sync_jobs::DEFAULT_PAGE_SIZE as SYNC_JOB_DEFAULT_PAGE_SIZE;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

use crate::harness::{seed_sync_job, workspace_arn, Emulator, Resp};

/// The workspace every scoped operation below is paged inside.
const WORKSPACE: &str = "pageSizes";
/// The entity `ListComponents` and `GetPropertyValue` are scoped to.
const ENTITY: &str = "unit";
/// The component on [`ENTITY`] whose properties `GetPropertyValue` reads.
const COMPONENT: &str = "Spec";
/// The component type the seeded components instantiate.
const COMPONENT_TYPE: &str = "com.example.paged";
/// The `contentLocation` every seeded scene carries: the modelled `S3Url`
/// pattern `^[sS]3://[A-Za-z0-9._/-]+$`.
const SCENE_CONTENT: &str = "s3://bucket/scene.json";

/// The property names [`COMPONENT_TYPE`] defines, and so the ones a property read
/// pages over.
///
/// Ten of them, because `selectedProperties` is modelled `@length(min: 1, max:
/// 10)` (`smithy_query(query="shape",shape="SelectedPropertyList")`): that is the
/// most rows one property read can have, which is why its page is compared
/// against the total the service itself reports rather than against a default
/// page it cannot fill.
fn property_names() -> Vec<String> {
    (0..10).map(|i| format!("P{i:02}")).collect()
}

/// One paginated operation of this service, with the two numbers its own
/// `maxResults` member documents and the request path that pages it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paged {
    ListWorkspaces,
    ListEntities,
    ListComponentTypes,
    ListScenes,
    ListSyncJobs,
    ListComponents,
    ListTagsForResource,
    ListMetadataTransferJobs,
    GetPropertyValue,
}

impl Paged {
    /// Every paginated operation of this service, each of which applies the rule.
    const ALL: [Self; 9] = [
        Self::ListWorkspaces,
        Self::ListEntities,
        Self::ListComponentTypes,
        Self::ListScenes,
        Self::ListSyncJobs,
        Self::ListComponents,
        Self::ListTagsForResource,
        Self::ListMetadataTransferJobs,
        Self::GetPropertyValue,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::ListWorkspaces => "ListWorkspaces",
            Self::ListEntities => "ListEntities",
            Self::ListComponentTypes => "ListComponentTypes",
            Self::ListScenes => "ListScenes",
            Self::ListSyncJobs => "ListSyncJobs",
            Self::ListComponents => "ListComponents",
            Self::ListTagsForResource => "ListTagsForResource",
            Self::ListMetadataTransferJobs => "ListMetadataTransferJobs",
            Self::GetPropertyValue => "GetPropertyValue",
        }
    }

    /// The page size this operation's `maxResults` member documents as its
    /// default: 50 for `ListSyncJobs` ("The maximum number of results to return
    /// at one time. The default is 50.",
    /// `smithy_query(query="doc",shape="ListSyncJobsRequest",member="maxResults")`),
    /// the service's own 25 for the rest.
    fn documented_default(self) -> usize {
        match self {
            Self::ListSyncJobs => SYNC_JOB_DEFAULT_PAGE_SIZE,
            _ => DEFAULT_PAGE_SIZE,
        }
    }

    /// The smallest `maxResults` this operation's member documents: zero where
    /// the member publishes a minimum of 0 or none at all (so a zero is a page of
    /// no rows), one where it publishes "Minimum value of 1" — read off the member
    /// docs with
    /// `smithy_query(query="doc",shape="<operation>Request",member="maxResults")`.
    fn documented_minimum(self) -> u32 {
        match self {
            Self::ListScenes | Self::ListSyncJobs | Self::ListComponents => 0,
            _ => 1,
        }
    }

    /// The member a page carries its rows in.
    fn rows_member(self) -> &'static str {
        match self {
            Self::ListWorkspaces => "workspaceSummaries",
            Self::ListEntities => "entitySummaries",
            Self::ListComponentTypes => "componentTypeSummaries",
            Self::ListScenes => "sceneSummaries",
            Self::ListSyncJobs => "syncJobSummaries",
            Self::ListComponents => "componentSummaries",
            Self::ListTagsForResource => "tags",
            Self::ListMetadataTransferJobs => "metadataTransferJobSummaries",
            Self::GetPropertyValue => "propertyValues",
        }
    }

    /// Fill the store with more rows than this operation's documented default
    /// page serves, so a page is truncated by the page size and not by what is
    /// stored.
    async fn seed(self, emulator: &Emulator, rows: usize) {
        match self {
            Self::ListWorkspaces => {
                for i in 0..rows {
                    create_workspace(emulator, &format!("ws{i:03}")).await;
                }
            }
            Self::ListEntities => {
                create_workspace(emulator, WORKSPACE).await;
                for i in 0..rows {
                    let entity_id = format!("ent{i:03}");
                    ok(
                        emulator
                            .create_entity(
                                WORKSPACE,
                                json!({"entityId": entity_id, "entityName": entity_id}),
                            )
                            .await,
                        &format!("CreateEntity {entity_id}"),
                    );
                }
            }
            Self::ListComponentTypes => {
                create_workspace(emulator, WORKSPACE).await;
                for i in 0..rows {
                    let component_type_id = format!("com.example.t{i:03}");
                    ok(
                        emulator
                            .create_component_type(WORKSPACE, &component_type_id, json!({}))
                            .await,
                        &format!("CreateComponentType {component_type_id}"),
                    );
                }
            }
            Self::ListScenes => {
                create_workspace(emulator, WORKSPACE).await;
                for i in 0..rows {
                    let scene_id = format!("scene{i:03}");
                    ok(
                        emulator
                            .create_scene(
                                WORKSPACE,
                                json!({"sceneId": scene_id, "contentLocation": SCENE_CONTENT}),
                            )
                            .await,
                        &format!("CreateScene {scene_id}"),
                    );
                }
            }
            Self::ListSyncJobs => {
                create_workspace(emulator, WORKSPACE).await;
                // A sync job is identified inside its workspace by its source and
                // the create path supports one source, so the rows a page needs
                // are written into the very store the handlers read.
                for i in 0..rows {
                    seed_sync_job(emulator, WORKSPACE, &format!("SRC_{i:03}"));
                }
            }
            Self::ListComponents => {
                create_workspace(emulator, WORKSPACE).await;
                seed_component_type(emulator, json!({})).await;
                let mut components = json!({});
                for i in 0..rows {
                    components[format!("C{i:03}")] = json!({"componentTypeId": COMPONENT_TYPE});
                }
                ok(
                    emulator
                        .create_entity(
                            WORKSPACE,
                            json!({
                                "entityId": ENTITY,
                                "entityName": ENTITY,
                                "components": components,
                            }),
                        )
                        .await,
                    "CreateEntity",
                );
            }
            Self::ListTagsForResource => {
                create_workspace(emulator, WORKSPACE).await;
                let mut tags = json!({});
                for i in 0..rows {
                    tags[format!("key{i:03}")] = json!("value");
                }
                ok(
                    emulator.tag_resource(&workspace_arn(WORKSPACE), tags).await,
                    "TagResource",
                );
            }
            Self::ListMetadataTransferJobs => {
                for i in 0..rows {
                    let id = format!("job{i:03}");
                    ok(
                        emulator
                            .create_metadata_transfer_job(json!({
                                "metadataTransferJobId": id,
                                "sources": [{
                                    "type": "s3",
                                    "s3Configuration": {
                                        "location": "arn:aws:s3:::metadata/entities.json",
                                    },
                                }],
                                "destination": {"type": "iotsitewise"},
                            }))
                            .await,
                        &format!("CreateMetadataTransferJob {id}"),
                    );
                }
            }
            Self::GetPropertyValue => {
                create_workspace(emulator, WORKSPACE).await;
                let mut definitions = json!({});
                for property_name in property_names() {
                    definitions[property_name] = json!({"dataType": {"type": "STRING"}});
                }
                seed_component_type(emulator, json!({"propertyDefinitions": definitions})).await;
                ok(
                    emulator
                        .create_entity(
                            WORKSPACE,
                            json!({
                                "entityId": ENTITY,
                                "entityName": ENTITY,
                                "components": {
                                    COMPONENT: {"componentTypeId": COMPONENT_TYPE},
                                },
                            }),
                        )
                        .await,
                    "CreateEntity",
                );
            }
        }
    }

    /// One page of this operation at the page size `max_results` asks for, or at
    /// none when it names none.
    async fn page(self, emulator: &Emulator, max_results: Option<u32>) -> Resp {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        match self {
            Self::ListWorkspaces => emulator.list_workspaces(body).await,
            Self::ListEntities => emulator.list_entities(WORKSPACE, body).await,
            Self::ListComponentTypes => emulator.list_component_types(WORKSPACE, body).await,
            Self::ListScenes => emulator.list_scenes(WORKSPACE, body).await,
            Self::ListSyncJobs => emulator.list_sync_jobs(WORKSPACE, body).await,
            Self::ListComponents => emulator.list_components(WORKSPACE, ENTITY, body).await,
            Self::ListTagsForResource => {
                emulator
                    .list_tags_for_resource(&workspace_arn(WORKSPACE), body)
                    .await
            }
            Self::ListMetadataTransferJobs => {
                // Both end types are required members, and they are the ends the
                // seeded jobs carry.
                body["sourceType"] = json!("s3");
                body["destinationType"] = json!("iotsitewise");
                emulator.list_metadata_transfer_jobs(body).await
            }
            Self::GetPropertyValue => {
                body["entityId"] = json!(ENTITY);
                body["componentName"] = json!(COMPONENT);
                body["selectedProperties"] = json!(property_names());
                emulator.get_property_value(WORKSPACE, body).await
            }
        }
    }

    /// How many rows a page served: a listing answers them as a list, the tag
    /// listing and the property read as a map.
    fn rows(self, case: &str, response: &Resp) -> usize {
        match response.member(self.rows_member()) {
            Value::Array(rows) => rows.len(),
            Value::Object(rows) => rows.len(),
            other => panic!(
                "case {case:?}: the page must carry its rows in {:?}, got {other:?}",
                self.rows_member()
            ),
        }
    }
}

/// Create the workspace `workspace_id`, which every scoped case needs.
async fn create_workspace(emulator: &Emulator, workspace_id: &str) {
    ok(
        emulator.create_workspace(workspace_id).await,
        &format!("CreateWorkspace {workspace_id}"),
    );
}

/// Create [`COMPONENT_TYPE`] in [`WORKSPACE`] with `body`'s definitions.
async fn seed_component_type(emulator: &Emulator, body: Value) {
    ok(
        emulator
            .create_component_type(WORKSPACE, COMPONENT_TYPE, body)
            .await,
        "CreateComponentType",
    );
}

/// Assert a seeding call was served, so a case that pages is paging rows that
/// really are stored.
fn ok(response: Resp, what: &str) {
    assert_eq!(
        response.status, 200,
        "{what}: expected 200 while seeding, got {response:?}"
    );
}

/// RULE: every paginated operation serves the page size its own `maxResults`
/// documents — the documented default when a request names none, otherwise what
/// it asks for, never more than the most this service serves and never more than
/// is stored — with a `nextToken` exactly when rows are left over.
#[tokio::test]
async fn every_paginated_operation_serves_the_page_size_its_member_documents() {
    // The documented numbers themselves, pinned as literals so the rows below
    // cannot drift with the implementation: "The default is 25" on the listings,
    // "The default is 50." on `ListSyncJobs`, and the top of the `maxResults`
    // range (0..200) as the most the service serves.
    assert_eq!(
        DEFAULT_PAGE_SIZE, 25,
        "the documented default page size is 25, got {DEFAULT_PAGE_SIZE}"
    );
    assert_eq!(
        SYNC_JOB_DEFAULT_PAGE_SIZE, 50,
        "ListSyncJobs documents a default page size of 50, got {SYNC_JOB_DEFAULT_PAGE_SIZE}"
    );
    assert_eq!(
        MAX_PAGE_SIZE, 200,
        "the documented maximum page size is 200, got {MAX_PAGE_SIZE}"
    );

    for paged in Paged::ALL {
        let emulator = Emulator::new();
        let default = paged.documented_default();
        paged.seed(&emulator, default + 2).await;

        // What the operation has to report, read off the service itself with the
        // largest page the published range admits: a workspace carries the
        // service's built-in component types beside the seeded ones, and a
        // property read can have at most ten rows, so the total is the service's
        // to state, not this suite's.
        let case = format!("{} whole listing", paged.name());
        let whole = paged.page(&emulator, Some(MAX_PAGE_SIZE as u32)).await;
        assert_eq!(
            whole.status, 200,
            "case {case:?}: expected 200, got {whole:?}"
        );
        let total = paged.rows(&case, &whole);
        assert!(
            total > 0,
            "case {case:?}: the seeded rows must be reported, got {whole:?}"
        );

        for max_results in [
            None,
            Some(1u32),
            Some(2),
            Some(7),
            Some(default as u32),
            Some(MAX_PAGE_SIZE as u32),
        ] {
            let case = format!("{} maxResults={max_results:?}", paged.name());
            let expected = max_results
                .map_or(default, |asked| asked as usize)
                .min(MAX_PAGE_SIZE)
                .min(total);
            let page = paged.page(&emulator, max_results).await;
            assert_eq!(page.status, 200, "case {case:?}: expected 200, got {page:?}");
            let rows = paged.rows(&case, &page);
            assert_eq!(
                rows, expected,
                "case {case:?}: expected {expected} of the {total} rows, got {rows}"
            );
            let token = page.member("nextToken").as_str().map(str::to_string);
            assert_eq!(
                token.is_some(),
                expected < total,
                "case {case:?}: a token comes back exactly when rows are left over \
                 ({expected} of {total} served), got {token:?}"
            );
        }
    }
}

/// RULE: the bounds of `maxResults` are the ones its member publishes — a value
/// below the documented minimum is refused with HTTP 400 `ValidationException`
/// naming the member (where that minimum is zero, a zero is instead served as the
/// page of no rows it asks for), and a value past the published maximum is
/// refused the same way.
#[tokio::test]
async fn every_paginated_operation_bounds_max_results_the_same_way() {
    for paged in Paged::ALL {
        let emulator = Emulator::new();
        paged.seed(&emulator, paged.documented_default() + 2).await;

        // The bottom of the published range, which the member's own documented
        // minimum either admits or refuses.
        let case = format!("{} maxResults=Some(0)", paged.name());
        let answered = paged.page(&emulator, Some(0)).await;
        if paged.documented_minimum() > 0 {
            assert_eq!(
                (answered.status, answered.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: a maxResults below the documented minimum of {} must be refused, \
                 got {answered:?}",
                paged.documented_minimum()
            );
            let message = answered.member("message").as_str().unwrap_or_default();
            assert!(
                message.contains("maxResults"),
                "case {case:?}: the refusal must name the member it is about, got {message:?}"
            );
        } else {
            assert_eq!(
                answered.status, 200,
                "case {case:?}: a member documenting a minimum of 0 serves a zero as a page of no \
                 rows, got {answered:?}"
            );
            let rows = paged.rows(&case, &answered);
            assert_eq!(
                rows, 0,
                "case {case:?}: expected a page of no rows, got {rows}"
            );
        }

        // One past the published top of the range, refused by the modelled
        // constraint before the page-size rule is reached.
        let case = format!("{} maxResults=Some({})", paged.name(), MAX_PAGE_SIZE + 1);
        let refused = paged.page(&emulator, Some(MAX_PAGE_SIZE as u32 + 1)).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a maxResults past the published maximum must be refused, got \
             {refused:?}"
        );
    }
}
