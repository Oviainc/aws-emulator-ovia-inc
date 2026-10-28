// SPDX-License-Identifier: CC0-1.0

//! What an explicit `maxResults` of zero means, driven end to end over HTTP
//! through every paginated operation of this service it can be sent to.
//!
//! RULE under test: the bounds of a paginated operation's `maxResults` are the
//! ones THAT member's own documentation publishes — an operation whose member
//! documents no minimum (or a minimum of zero) serves the zero its modelled range
//! admits as the page of no rows it literally asks for, and only an operation
//! whose member documents "Minimum value of 1." refuses it, with HTTP 400
//! `ValidationException`. Two operations whose members document the same thing
//! therefore answer the same request the same way, whatever each listing is over.
//!
//! The rows span the operations the rule holds for — an account-wide listing, the
//! ones scoped to a workspace, the one scoped to an entity, the one scoped to a
//! component and the one scoped to a sync job — and, for each, how much is stored
//! behind it: nothing, one row, and more rows than a default page serves. A page
//! of no rows must not DEPEND on the listing being empty, and must not consume
//! the rows it did not serve: each case resumes the token the zero page answered
//! and reads the whole listing back through it.
//!
//! Each case drives the real request path (`MultiServiceRouter::dispatch`, the
//! entry the served emulator uses), so the route, the labels, the status code and
//! the serialized body are all exercised, not just a handler body.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    SyncResource, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, SYNC_SOURCE_SITEWISE,
};

use crate::harness::{seed_sync_job, workspace_arn, Emulator, Resp, REGION};

/// The workspace every scoped case below pages inside.
const WORKSPACE: &str = "zeroPages";
/// The entity `ListComponents` and `ListProperties` are scoped to.
const ENTITY: &str = "unit";
/// The component on [`ENTITY`] whose properties `ListProperties` pages over.
const COMPONENT: &str = "Spec";
/// The component type the seeded components instantiate and the seeded properties
/// are defined on.
const COMPONENT_TYPE: &str = "com.example.zeroed";
/// The `contentLocation` every seeded scene carries: the modelled `S3Url` pattern
/// `^[sS]3://[A-Za-z0-9._/-]+$`.
const SCENE_CONTENT: &str = "s3://bucket/scene.json";

/// How much is stored behind a listing when the zero page is asked for: nothing,
/// one row, and more rows than a default page serves — so a page of no rows is
/// not the accident of an empty or a short listing.
const STORED: [usize; 3] = [0, 1, DEFAULT_PAGE_SIZE + 2];

/// One paginated operation whose `maxResults` member documents no minimum of 1,
/// and so serves an explicit zero as a page of no rows.
///
/// Read off the member docs with
/// `smithy_query(query="doc",shape="<operation>Request",member="maxResults")`:
/// `ListScenes` ("Specifies the maximum number of results to display."),
/// `ListComponents` and `ListProperties` ("The maximum number of results
/// returned at one time. The default is 25."),
/// `ListMetadataTransferJobs` ("The maximum number of results to return at one
/// time.") publish no minimum at all, and `ListSyncJobs` and `ListSyncResources`
/// publish "Valid Range: Minimum value of 0."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServesZero {
    MetadataTransferJobs,
    Scenes,
    SyncJobs,
    SyncResources,
    Components,
    Properties,
}

impl ServesZero {
    const ALL: [Self; 6] = [
        Self::MetadataTransferJobs,
        Self::Scenes,
        Self::SyncJobs,
        Self::SyncResources,
        Self::Components,
        Self::Properties,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::MetadataTransferJobs => "ListMetadataTransferJobs",
            Self::Scenes => "ListScenes",
            Self::SyncJobs => "ListSyncJobs",
            Self::SyncResources => "ListSyncResources",
            Self::Components => "ListComponents",
            Self::Properties => "ListProperties",
        }
    }

    /// The member a page carries its rows in.
    fn rows_member(self) -> &'static str {
        match self {
            Self::MetadataTransferJobs => "metadataTransferJobSummaries",
            Self::Scenes => "sceneSummaries",
            Self::SyncJobs => "syncJobSummaries",
            Self::SyncResources => "syncResources",
            Self::Components => "componentSummaries",
            Self::Properties => "propertySummaries",
        }
    }

    /// Store `rows` rows for this listing to page over, through the operations
    /// that create them where there are any (a sync job and a sync resource are
    /// written by a synchronisation this emulator does not perform, so those are
    /// seeded straight into the store the handlers read).
    async fn seed(self, emulator: &Emulator, rows: usize) {
        match self {
            Self::MetadataTransferJobs => {
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
            Self::Scenes => {
                create_workspace(emulator).await;
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
            Self::SyncJobs => {
                create_workspace(emulator).await;
                for i in 0..rows {
                    seed_sync_job(emulator, WORKSPACE, &format!("SRC_{i:03}"));
                }
            }
            Self::SyncResources => {
                create_workspace(emulator).await;
                // The job the listing ADDRESSES exists whatever it holds, so a
                // zero-row case is a job holding no resource rather than no job.
                seed_sync_job(emulator, WORKSPACE, SYNC_SOURCE_SITEWISE);
                seed_sync_resources(emulator, rows);
            }
            Self::Components => {
                create_workspace(emulator).await;
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
            Self::Properties => {
                create_workspace(emulator).await;
                let mut definitions = json!({});
                for i in 0..rows {
                    definitions[format!("P{i:03}")] = json!({"dataType": {"type": "STRING"}});
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

    /// One page of this operation at the page size `max_results`, resumed at
    /// `next_token` when there is one.
    async fn page(
        self,
        emulator: &Emulator,
        max_results: Option<u32>,
        next_token: Option<&str>,
    ) -> Resp {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(next_token) = next_token {
            body["nextToken"] = json!(next_token);
        }
        match self {
            Self::MetadataTransferJobs => {
                // Both end types are required members, and they are the ends the
                // seeded jobs carry.
                body["sourceType"] = json!("s3");
                body["destinationType"] = json!("iotsitewise");
                emulator.list_metadata_transfer_jobs(body).await
            }
            Self::Scenes => emulator.list_scenes(WORKSPACE, body).await,
            Self::SyncJobs => emulator.list_sync_jobs(WORKSPACE, body).await,
            Self::SyncResources => {
                emulator
                    .list_sync_resources(WORKSPACE, SYNC_SOURCE_SITEWISE, body)
                    .await
            }
            Self::Components => emulator.list_components(WORKSPACE, ENTITY, body).await,
            Self::Properties => {
                body["entityId"] = json!(ENTITY);
                body["componentName"] = json!(COMPONENT);
                emulator.list_properties(WORKSPACE, body).await
            }
        }
    }

    /// How many rows a page served.
    fn rows(self, case: &str, response: &Resp) -> usize {
        rows_of(case, self.rows_member(), response)
    }
}

/// One paginated operation whose `maxResults` member publishes "Valid Range:
/// Minimum value of 1.", and so refuses an explicit zero — the control side of
/// the rule, so a zero served where the documentation is silent is not a zero
/// served everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusesZero {
    Workspaces,
    Entities,
    ComponentTypes,
    TagsForResource,
}

impl RefusesZero {
    const ALL: [Self; 4] = [
        Self::Workspaces,
        Self::Entities,
        Self::ComponentTypes,
        Self::TagsForResource,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Workspaces => "ListWorkspaces",
            Self::Entities => "ListEntities",
            Self::ComponentTypes => "ListComponentTypes",
            Self::TagsForResource => "ListTagsForResource",
        }
    }

    /// The member a served page of this operation carries its rows in, so a case
    /// can assert the refusal reported none of them.
    fn rows_member(self) -> &'static str {
        match self {
            Self::Workspaces => "workspaceSummaries",
            Self::Entities => "entitySummaries",
            Self::ComponentTypes => "componentTypeSummaries",
            Self::TagsForResource => "tags",
        }
    }

    /// Store something for this listing to have reported, so the refusal is the
    /// page size's and not an empty listing's.
    async fn seed(self, emulator: &Emulator) {
        create_workspace(emulator).await;
        match self {
            Self::Workspaces | Self::ComponentTypes => {}
            Self::Entities => {
                ok(
                    emulator
                        .create_entity(
                            WORKSPACE,
                            json!({"entityId": ENTITY, "entityName": ENTITY}),
                        )
                        .await,
                    "CreateEntity",
                );
            }
            Self::TagsForResource => {
                ok(
                    emulator
                        .tag_resource(&workspace_arn(WORKSPACE), json!({"owner": "tests"}))
                        .await,
                    "TagResource",
                );
            }
        }
    }

    /// One page of this operation at the page size `max_results`.
    async fn page(self, emulator: &Emulator, max_results: Option<u32>) -> Resp {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        match self {
            Self::Workspaces => emulator.list_workspaces(body).await,
            Self::Entities => emulator.list_entities(WORKSPACE, body).await,
            Self::ComponentTypes => emulator.list_component_types(WORKSPACE, body).await,
            Self::TagsForResource => {
                emulator
                    .list_tags_for_resource(&workspace_arn(WORKSPACE), body)
                    .await
            }
        }
    }

    fn rows(self, case: &str, response: &Resp) -> usize {
        rows_of(case, self.rows_member(), response)
    }
}

/// How many rows `member` carries: a listing answers them as a list, the tag
/// listing as a map.
fn rows_of(case: &str, member: &str, response: &Resp) -> usize {
    match response.member(member) {
        Value::Array(rows) => rows.len(),
        Value::Object(rows) => rows.len(),
        other => {
            panic!("case {case:?}: the page must carry its rows in {member:?}, got {other:?}")
        }
    }
}

/// Create [`WORKSPACE`], which every scoped case needs.
async fn create_workspace(emulator: &Emulator) {
    ok(
        emulator.create_workspace(WORKSPACE).await,
        &format!("CreateWorkspace {WORKSPACE}"),
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

/// Store `count` sync resources on [`WORKSPACE`]'s sync job, written into the
/// very store the handlers read: a synchronisation is what writes one, and this
/// emulator performs none, so no operation of it can.
fn seed_sync_resources(emulator: &Emulator, count: usize) {
    let state = emulator.state();
    let mut state = state.lock();
    let state = state.region_mut(REGION);
    let workspace = state
        .workspaces
        .get_mut(WORKSPACE)
        .unwrap_or_else(|| panic!("the workspace {WORKSPACE:?} must exist before it is seeded"));
    let job = workspace
        .sync_jobs
        .get_mut(SYNC_SOURCE_SITEWISE)
        .unwrap_or_else(|| panic!("the sync job {SYNC_SOURCE_SITEWISE:?} must exist"));
    for index in 0..count {
        let external_id = format!("sitewiseAsset{index:03}");
        job.sync_resources.insert(
            external_id.clone(),
            SyncResource {
                resource_type: "ENTITY".to_string(),
                external_id,
                resource_id: None,
                state: "INITIALIZING".to_string(),
                error: None,
                update_date_time: SystemTime::now(),
            },
        );
    }
}

/// Assert a seeding call was served, so a case that pages is paging rows that
/// really are stored.
fn ok(response: Resp, what: &str) {
    assert_eq!(
        response.status, 200,
        "{what}: expected 200 while seeding, got {response:?}"
    );
}

/// RULE: an operation whose `maxResults` documents no minimum serves an explicit
/// zero as the page of no rows it asks for — HTTP 200, no rows, a `nextToken`
/// exactly when the listing has rows left over — whatever the listing is over and
/// however much is stored behind it.
#[tokio::test]
async fn a_member_documenting_no_minimum_serves_an_explicit_zero_as_a_page_of_no_rows() {
    for serves_zero in ServesZero::ALL {
        for stored in STORED {
            let emulator = Emulator::new();
            serves_zero.seed(&emulator, stored).await;
            let case = format!("{} over {stored} stored rows", serves_zero.name());

            // What this listing has to report, read off the service itself with
            // the largest page the published range admits.
            let whole = serves_zero
                .page(&emulator, Some(MAX_PAGE_SIZE as u32), None)
                .await;
            assert_eq!(
                whole.status, 200,
                "case {case:?}: expected 200 for the whole listing, got {whole:?}"
            );
            let total = serves_zero.rows(&case, &whole);
            assert!(
                total >= stored,
                "case {case:?}: every stored row must be reported, got {total}"
            );

            let zero = serves_zero.page(&emulator, Some(0), None).await;
            assert_eq!(
                zero.status, 200,
                "case {case:?}: a member documenting no minimum serves maxResults=0 as a page of \
                 no rows, got {zero:?}"
            );
            let rows = serves_zero.rows(&case, &zero);
            assert_eq!(
                rows, 0,
                "case {case:?}: expected a page of no rows, got {rows}"
            );

            // The token a zero-row page answers names the position the call
            // STARTED from, so it comes back exactly when rows are left over.
            let token = zero.member("nextToken").as_str().map(str::to_string);
            assert_eq!(
                token.is_some(),
                total > 0,
                "case {case:?}: a token comes back exactly when rows are left over (0 of {total} \
                 served), got {token:?}"
            );

            // A page of no rows consumed none: resuming at its token reads the
            // whole listing back.
            if let Some(token) = token {
                let resumed = serves_zero
                    .page(&emulator, Some(MAX_PAGE_SIZE as u32), Some(&token))
                    .await;
                assert_eq!(
                    resumed.status, 200,
                    "case {case:?}: expected 200 resuming at the zero page's token, got {resumed:?}"
                );
                let resumed_rows = serves_zero.rows(&case, &resumed);
                assert_eq!(
                    resumed_rows, total,
                    "case {case:?}: a page of no rows consumes none, so resuming at its token \
                     serves all {total} rows, got {resumed_rows}"
                );
            }

            // And the zero page changed nothing a later caller sees.
            let again = serves_zero
                .page(&emulator, Some(MAX_PAGE_SIZE as u32), None)
                .await;
            let again_rows = serves_zero.rows(&case, &again);
            assert_eq!(
                again_rows, total,
                "case {case:?}: the listing still reports all {total} rows after a zero page, got \
                 {again_rows}"
            );
        }
    }
}

/// RULE (the control side): an operation whose `maxResults` publishes a minimum
/// of 1 refuses an explicit zero with HTTP 400 `ValidationException`, naming the
/// member, that minimum and the default the member can be omitted for, and
/// reports no row.
#[tokio::test]
async fn a_member_documenting_a_minimum_of_one_refuses_an_explicit_zero() {
    for refuses_zero in RefusesZero::ALL {
        let emulator = Emulator::new();
        refuses_zero.seed(&emulator).await;
        let case = refuses_zero.name();

        let refused = refuses_zero.page(&emulator, Some(0)).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a maxResults below the documented minimum of 1 must be refused, got \
             {refused:?}"
        );
        let expected =
            format!("maxResults must be at least 1; omit it for the default of {DEFAULT_PAGE_SIZE}");
        let message = refused.member("message").as_str().unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );

        // The same request with the smallest value that member DOES document is
        // served, so the refusal is about the zero and not about the listing.
        let served = refuses_zero.page(&emulator, Some(1)).await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: expected 200 at the documented minimum of 1, got {served:?}"
        );
        let rows = refuses_zero.rows(case, &served);
        assert_eq!(
            rows, 1,
            "case {case:?}: expected the one row a page of one serves, got {rows}"
        );
    }
}
