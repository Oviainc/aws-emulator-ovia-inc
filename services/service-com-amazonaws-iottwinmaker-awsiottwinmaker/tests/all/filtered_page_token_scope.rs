// SPDX-License-Identifier: CC0-1.0

//! What a FILTERED listing's `nextToken` resumes, end to end over HTTP.
//!
//! RULE under test: a page resumes INSIDE the one sequence its resume key was
//! taken from, and a request's `filters` are what decide which rows that
//! sequence holds — a row no filter selects is not in it — so a listing binds
//! its token to the filters it was minted under: a request carrying those
//! filters, however it spells them, resumes the walk, and a request whose
//! filters decide another sequence is refused with a 400 `ValidationException`
//! rather than resumed past every row the new filters select ahead of that key.
//!
//! The rows differ along what the rule generalises over: WHICH filtered listing
//! is paged (the entity, component-type, sync-resource and metadata-transfer-job
//! listings — every operation of this service whose `filters` select the rows it
//! pages), which filter MINTED the token, what the resuming request changes (the
//! value filtered on, the member filtered on, how many filters there are,
//! dropping them altogether) and, on the other side of the rule, which SPELLING
//! of the same filters still resumes (the whole request repeated, an empty
//! `filters` list where the minting request named none, the filters listed in
//! another order, a filter repeated, another `maxResults`) — because a rule that
//! refused every difference between two requests would break the paginator that
//! resumes by repeating the request.
//!
//! What that refusal protects is pinned too, over the page sizes a listing
//! distinguishes: a caller whose filters changed is sent back to the START of
//! the sequence it now asks for, and walking that sequence reports every row it
//! holds — including the rows that sort BEFORE the id a token of the other
//! sequence resumes at, which are the ones a silently resumed token leaves out
//! of the walk.

use std::thread::sleep;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    SYNC_RESOURCE_STATE_VALUES, SYNC_RESOURCE_TYPE_VALUES,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    SyncResource, METADATA_TRANSFER_JOB_SETTLE_SPAN, METADATA_TRANSFER_JOB_STATE_COMPLETED,
    METADATA_TRANSFER_JOB_STATE_ERROR, ROOT_PARENT_ENTITY_ID, SYNC_SOURCE_SITEWISE,
};

use crate::harness::{seed_sync_job, workspace_arn, Emulator, Resp, REGION};

/// The ONE sentence every paginated operation of this service reports for a
/// token it did not mint. Written out here rather than read from the crate, so
/// this asserts the wording instead of restating the implementation.
const REFUSAL: &str = "nextToken is not a token this listing minted";

/// The page size a resume asks for unless a row names another: larger than any
/// sequence seeded here, so a resumed page carries every row the minting page
/// left over.
const PAGE: u32 = 25;

/// One resume of a filtered page: the `filters` the resuming request carries,
/// the page size it asks for, and whether those filters are the ones the token
/// was minted under.
#[derive(Debug, Clone)]
struct Resume {
    /// What this row is about, quoted by every assertion it makes.
    what: String,
    /// `None` is the `filters` member omitted altogether.
    filters: Option<Value>,
    max_results: u32,
    /// `true` where these filters decide the sequence the token's resume key was
    /// taken from — however they are spelled — so the page resumes at the row
    /// after the minting page's last; `false` where they decide another
    /// sequence, in which that key names no position, so the token is refused
    /// instead.
    resumes: bool,
}

impl Resume {
    /// A resume carrying `filters`.
    fn under(what: &str, filters: Value, resumes: bool) -> Self {
        Self {
            what: what.to_string(),
            filters: Some(filters),
            max_results: PAGE,
            resumes,
        }
    }

    /// A resume naming no `filters` at all.
    fn unfiltered(what: &str, resumes: bool) -> Self {
        Self {
            what: what.to_string(),
            filters: None,
            max_results: PAGE,
            resumes,
        }
    }

    /// The same resume asking for a page of `max_results` rows.
    fn at_page_size(mut self, max_results: u32) -> Self {
        self.max_results = max_results;
        self
    }
}

/// The keys the rows of a page carry: `key` read off each entry of `member`.
fn reported(page: &Resp, member: &str, key: &str) -> Vec<String> {
    page.member(member)
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row[key].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Drive one minting-and-resuming walk of a filtered listing.
///
/// `page` takes the `filters`, the page size and the `nextToken` a request
/// carries and answers what the listing served; `member` and `key` say where the
/// response carries its rows' keys. The minting page is cut to ONE row, so every
/// further row of the sequence is one a resume has to report — and a resume
/// under filters deciding another sequence is refused instead of silently
/// walking past the rows those filters select ahead of the resume key.
async fn check_resumes<F, Fut>(
    listing: &str,
    page: F,
    member: &str,
    key: &str,
    minting_filters: Option<Value>,
    resumes: Vec<Resume>,
) where
    F: Fn(Option<Value>, u32, Option<String>) -> Fut,
    Fut: std::future::Future<Output = Resp>,
{
    // The whole sequence the minting filters decide, read in ONE page, so the
    // rows a resumed page has to carry are that sequence's own rather than a
    // literal written down here.
    let whole = page(minting_filters.clone(), PAGE, None).await;
    assert_eq!(
        whole.status, 200,
        "{listing}: the whole sequence expected 200, got {whole:?}"
    );
    let sequence = reported(&whole, member, key);
    assert!(
        sequence.len() >= 3,
        "{listing}: the minting filters must select at least three rows, so a one-row page leaves \
         more than one over, got {sequence:?}"
    );

    let minted = page(minting_filters.clone(), 1, None).await;
    assert_eq!(
        minted.status, 200,
        "{listing}: the minting page expected 200, got {minted:?}"
    );
    assert_eq!(
        reported(&minted, member, key),
        sequence[..1].to_vec(),
        "{listing}: the minting page expected the sequence's first row, got {minted:?}"
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("{listing}: the minting page left rows over, so it must mint a token: {minted:?}")
        })
        .to_string();
    let left_over = sequence[1..].to_vec();

    for resume in resumes {
        let case = format!("{listing}, resumed by {}", resume.what);
        let served = page(
            resume.filters.clone(),
            resume.max_results,
            Some(token.clone()),
        )
        .await;
        if resume.resumes {
            assert_eq!(
                served.status, 200,
                "case {case:?}: expected 200, got {served:?}"
            );
            let expected: Vec<String> = left_over
                .iter()
                .take(resume.max_results as usize)
                .cloned()
                .collect();
            assert_eq!(
                reported(&served, member, key),
                expected,
                "case {case:?}: expected the rows the minting page left over, got {served:?}"
            );
            assert_eq!(
                !served.member("nextToken").is_null(),
                expected.len() < left_over.len(),
                "case {case:?}: expected a token exactly when rows are still left over, got \
                 {served:?}"
            );
        } else {
            assert_eq!(
                served.status, 400,
                "case {case:?}: expected 400, got {served:?}"
            );
            assert_eq!(
                served.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {served:?}"
            );
            let message = served.member("message").as_str().unwrap_or_default();
            assert_eq!(
                message, REFUSAL,
                "case {case:?}: expected the message {REFUSAL:?}, got {served:?}"
            );
            assert!(
                reported(&served, member, key).is_empty(),
                "case {case:?}: expected no rows at all, got {served:?}"
            );
        }
    }
}

/// Every row a listing taken under `filters` reports, walked page by page at
/// `page_size` through the `nextToken`s it mints: what a caller starting that
/// sequence from the beginning — the only thing a refused token leaves it able
/// to do — gets to see.
///
/// Bounded, so a listing that never stops handing out tokens fails the case
/// instead of looping: no case here seeds more than a handful of rows.
async fn walk<F, Fut>(
    listing: &str,
    page: &F,
    member: &str,
    key: &str,
    filters: Option<Value>,
    page_size: u32,
) -> Vec<String>
where
    F: Fn(Option<Value>, u32, Option<String>) -> Fut,
    Fut: std::future::Future<Output = Resp>,
{
    let mut rows = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let served = page(filters.clone(), page_size, token.clone()).await;
        assert_eq!(
            served.status, 200,
            "{listing} at page size {page_size}: expected 200, got {served:?}"
        );
        let reported_rows = reported(&served, member, key);
        assert!(
            reported_rows.len() <= page_size as usize,
            "{listing} at page size {page_size}: a page may not exceed the size asked for, got \
             {served:?}"
        );
        rows.extend(reported_rows);
        token = served.member("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            return rows;
        }
    }
    panic!("{listing} at page size {page_size}: the listing kept handing out nextTokens");
}

/// The workspace the `ListEntities` cases page over.
const ENTITY_WORKSPACE: &str = "filterScopeEntities";
/// The component type whose external-id property the `externalId` filter reads.
const SENSOR: &str = "com.example.sensorFilterScope";
/// A second component type, so a `componentTypeId` filter has something to
/// EXCLUDE.
const PUMP: &str = "com.example.pumpFilterScope";
/// The entity every child entity below names as its parent.
const PARENT_ENTITY: &str = "alpha";

/// The workspace, component types and entities the `ListEntities` cases page
/// over.
///
/// The ids are seeded so that each filtered sequence holds three rows and the
/// row a filtered token resumes at has rows of OTHER sequences both before and
/// after it: `bravo`/`charlie`/`delta` are the children of `alpha`,
/// `bravo`/`delta`/`echo` are the entities holding a sensor component, and the
/// unfiltered listing holds all five.
async fn seed_entity_workspace(emulator: &Emulator) {
    let created = emulator.create_workspace(ENTITY_WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace {ENTITY_WORKSPACE:?}: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            SENSOR,
            json!({"propertyDefinitions": {
                "serial": {"dataType": {"type": "STRING"}, "isExternalId": true},
            }}),
        ),
        (PUMP, json!({})),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(ENTITY_WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    // (entityId, the parent it names, the components it holds)
    let entities: Vec<(&str, Option<&str>, Value)> = vec![
        (PARENT_ENTITY, None, json!({})),
        (
            "bravo",
            Some(PARENT_ENTITY),
            json!({"Sensor": {
                "componentTypeId": SENSOR,
                "properties": {"serial": {"value": {"stringValue": "ext-bravo"}}},
            }}),
        ),
        (
            "charlie",
            Some(PARENT_ENTITY),
            json!({"Pump": {"componentTypeId": PUMP}}),
        ),
        (
            "delta",
            Some(PARENT_ENTITY),
            json!({"Sensor": {
                "componentTypeId": SENSOR,
                "properties": {"serial": {"value": {"stringValue": "ext-delta"}}},
            }}),
        ),
        (
            "echo",
            None,
            json!({"Sensor": {
                "componentTypeId": SENSOR,
                "properties": {"serial": {"value": {"stringValue": "ext-echo"}}},
            }}),
        ),
    ];
    for (entity_id, parent_entity_id, components) in entities {
        let mut body = json!({"entityId": entity_id, "entityName": entity_id});
        if let Some(parent_entity_id) = parent_entity_id {
            body["parentEntityId"] = json!(parent_entity_id);
        }
        if components.as_object().is_some_and(|holds| !holds.is_empty()) {
            body["components"] = components;
        }
        let created = emulator.create_entity(ENTITY_WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
}

#[tokio::test]
async fn a_list_entities_token_is_bound_to_the_filter_that_selected_its_page() {
    let emulator = Emulator::new();
    seed_entity_workspace(&emulator).await;

    let served = &emulator;
    let page = move |filters: Option<Value>, max_results: u32, token: Option<String>| async move {
        let mut body = json!({"maxResults": max_results});
        if let Some(filters) = filters {
            body["filters"] = filters;
        }
        if let Some(token) = token {
            body["nextToken"] = json!(token);
        }
        served.list_entities(ENTITY_WORKSPACE, body).await
    };

    check_resumes(
        "ListEntities under a parentEntityId filter",
        &page,
        "entitySummaries",
        "entityId",
        Some(json!([{"parentEntityId": PARENT_ENTITY}])),
        vec![
            Resume::under(
                "the whole request repeated, which is what an SDK paginator does",
                json!([{"parentEntityId": PARENT_ENTITY}]),
                true,
            ),
            Resume::under(
                "the same filter at another page size, which decides no row",
                json!([{"parentEntityId": PARENT_ENTITY}]),
                true,
            )
            .at_page_size(1),
            Resume::unfiltered(
                "no filters at all, whose sequence holds every entity of the workspace",
                false,
            ),
            Resume::under(
                "an empty filter list, which is that whole-workspace sequence too",
                json!([]),
                false,
            ),
            Resume::under(
                "the same member filtered on another parent",
                json!([{"parentEntityId": ROOT_PARENT_ENTITY_ID}]),
                false,
            ),
            Resume::under(
                "another member filtered on",
                json!([{"componentTypeId": SENSOR}]),
                false,
            ),
            Resume::under(
                "the external-id filter",
                json!([{"externalId": "ext-bravo"}]),
                false,
            ),
        ],
    )
    .await;

    check_resumes(
        "ListEntities under a componentTypeId filter",
        &page,
        "entitySummaries",
        "entityId",
        Some(json!([{"componentTypeId": SENSOR}])),
        vec![
            Resume::under(
                "the same filter repeated",
                json!([{"componentTypeId": SENSOR}]),
                true,
            ),
            Resume::under(
                "another component type filtered on",
                json!([{"componentTypeId": PUMP}]),
                false,
            ),
            Resume::under(
                "a parentEntityId filter",
                json!([{"parentEntityId": PARENT_ENTITY}]),
                false,
            ),
            Resume::unfiltered("no filters at all", false),
        ],
    )
    .await;

    check_resumes(
        "ListEntities with no filter",
        &page,
        "entitySummaries",
        "entityId",
        None,
        vec![
            Resume::unfiltered("no filters again, as the minting request named none", true),
            Resume::under(
                "an empty filter list, which selects the same whole workspace",
                json!([]),
                true,
            ),
            Resume::under(
                "a parentEntityId filter, whose sequence is a shorter one",
                json!([{"parentEntityId": PARENT_ENTITY}]),
                false,
            ),
            Resume::under(
                "a componentTypeId filter",
                json!([{"componentTypeId": SENSOR}]),
                false,
            ),
        ],
    )
    .await;

    // What the refusal protects: a caller that drops its filter is sent back to
    // the START of the unfiltered listing, where every entity of the workspace
    // is still reported — none of them lost to a resume key counted in another
    // sequence.
    let whole = page(None, PAGE, None).await;
    assert_eq!(
        whole.status, 200,
        "the unfiltered listing: expected 200, got {whole:?}"
    );
    assert_eq!(
        reported(&whole, "entitySummaries", "entityId"),
        vec![PARENT_ENTITY, "bravo", "charlie", "delta", "echo"],
        "the unfiltered listing must report every entity of the workspace, got {whole:?}"
    );
}

/// The workspace the `ListComponentTypes` cases page over.
const TYPE_WORKSPACE: &str = "filterScopeTypes";
/// The component type the paged sequence's members extend.
const TYPE_PARENT: &str = "com.example.parentFilterScope";
/// A second parent, so an `extendsFrom` filter has something to EXCLUDE.
const TYPE_OTHER_PARENT: &str = "com.other.parentFilterScope";

/// The workspace and component types the `ListComponentTypes` cases page over.
///
/// Three heirs of one parent, so the filtered sequence holds three rows, plus a
/// second parent with an heir of its own; the workspace also carries the
/// built-in component types the service provisions it with, which sort ahead of
/// these ids and are exactly the rows an unfiltered resume would walk past.
async fn seed_type_workspace(emulator: &Emulator) {
    let created = emulator.create_workspace(TYPE_WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace {TYPE_WORKSPACE:?}: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (TYPE_PARENT, json!({})),
        (TYPE_OTHER_PARENT, json!({})),
        (
            "com.example.heirAlpha",
            json!({"extendsFrom": [TYPE_PARENT]}),
        ),
        (
            "com.example.heirBravo",
            json!({"extendsFrom": [TYPE_PARENT]}),
        ),
        (
            "com.example.heirCharlie",
            json!({"extendsFrom": [TYPE_PARENT]}),
        ),
        (
            "com.other.heirDelta",
            json!({"extendsFrom": [TYPE_OTHER_PARENT]}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(TYPE_WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }
}

#[tokio::test]
async fn a_list_component_types_token_is_bound_to_the_filter_that_selected_its_page() {
    let emulator = Emulator::new();
    seed_type_workspace(&emulator).await;

    let served = &emulator;
    let page = move |filters: Option<Value>, max_results: u32, token: Option<String>| async move {
        let mut body = json!({"maxResults": max_results});
        if let Some(filters) = filters {
            body["filters"] = filters;
        }
        if let Some(token) = token {
            body["nextToken"] = json!(token);
        }
        served.list_component_types(TYPE_WORKSPACE, body).await
    };

    check_resumes(
        "ListComponentTypes under an extendsFrom filter",
        &page,
        "componentTypeSummaries",
        "componentTypeId",
        Some(json!([{"extendsFrom": TYPE_PARENT}])),
        vec![
            Resume::under(
                "the whole request repeated",
                json!([{"extendsFrom": TYPE_PARENT}]),
                true,
            ),
            Resume::under(
                "the same filter at another page size",
                json!([{"extendsFrom": TYPE_PARENT}]),
                true,
            )
            .at_page_size(1),
            Resume::unfiltered(
                "no filters at all, whose sequence holds the workspace's built-ins too",
                false,
            ),
            Resume::under(
                "an empty filter list, which is that whole-workspace sequence too",
                json!([]),
                false,
            ),
            Resume::under(
                "another parent filtered on",
                json!([{"extendsFrom": TYPE_OTHER_PARENT}]),
                false,
            ),
            Resume::under(
                "a namespace filtered on",
                json!([{"namespace": "com.example"}]),
                false,
            ),
            Resume::under(
                "abstractness filtered on",
                json!([{"isAbstract": false}]),
                false,
            ),
        ],
    )
    .await;

    check_resumes(
        "ListComponentTypes under a namespace filter",
        &page,
        "componentTypeSummaries",
        "componentTypeId",
        Some(json!([{"namespace": "com.example"}])),
        vec![
            Resume::under(
                "the same namespace filtered on again",
                json!([{"namespace": "com.example"}]),
                true,
            ),
            Resume::under(
                "another namespace filtered on",
                json!([{"namespace": "com.other"}]),
                false,
            ),
            Resume::under(
                "an extendsFrom filter",
                json!([{"extendsFrom": TYPE_PARENT}]),
                false,
            ),
            Resume::unfiltered("no filters at all", false),
        ],
    )
    .await;
}

/// The workspace whose sync job the `ListSyncResources` cases page over.
const SYNC_WORKSPACE: &str = "filterScopeSync";
/// The `SyncResourceState` the paged sequence's resources are in, and the one a
/// resume names instead. Both are values of the model's own
/// `SyncResourceState` (`ops::types::SYNC_RESOURCE_STATE_VALUES`, asserted
/// below).
const SYNCED: &str = "IN_SYNC";
const SYNCING: &str = "PROCESSING";
/// The `SyncResourceType` of the resources seeded below, a value of the model's
/// own `SyncResourceType` (`ops::types::SYNC_RESOURCE_TYPE_VALUES`).
const SYNCED_ENTITY: &str = "ENTITY";

/// One seeded sync resource: its id in the source, its type, its id in the
/// workspace and the state of its synchronisation.
struct SeededResource {
    external_id: &'static str,
    resource_type: &'static str,
    resource_id: &'static str,
    state: &'static str,
}

/// The resources the `ListSyncResources` cases page over.
///
/// `resCharlie` sits BETWEEN two rows of the filtered sequence in external-id
/// order and is in another state, so it is exactly the row an unfiltered resume
/// would have to report and the filtered walk must not.
const SEEDED_RESOURCES: &[SeededResource] = &[
    SeededResource {
        external_id: "resAlpha",
        resource_type: SYNCED_ENTITY,
        resource_id: "alphaEntity",
        state: SYNCED,
    },
    SeededResource {
        external_id: "resBravo",
        resource_type: SYNCED_ENTITY,
        resource_id: "bravoEntity",
        state: SYNCED,
    },
    SeededResource {
        external_id: "resCharlie",
        resource_type: SYNCED_ENTITY,
        resource_id: "charlieEntity",
        state: SYNCING,
    },
    SeededResource {
        external_id: "resDelta",
        resource_type: SYNCED_ENTITY,
        resource_id: "deltaEntity",
        state: SYNCED,
    },
];

/// The workspace, sync job and sync resources the `ListSyncResources` cases
/// page over.
///
/// A sync resource is written by a synchronisation, which this emulator does not
/// perform, so the rows are seeded straight into the store the handlers read —
/// the seam `harness::Emulator::state` exists for, as `sync_resource_listing`
/// seeds the rows its own cases are about.
async fn seed_sync_workspace(emulator: &Emulator) {
    let created = emulator.create_workspace(SYNC_WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace {SYNC_WORKSPACE:?}: expected 200, got {created:?}"
    );
    seed_sync_job(emulator, SYNC_WORKSPACE, SYNC_SOURCE_SITEWISE);

    let state = emulator.state();
    let mut state = state.lock();
    // Seeded into the store of the Region every request from this harness is
    // signed for, the one the handlers serve those requests out of.
    let state = state.region_mut(REGION);
    let job = state
        .workspaces
        .get_mut(SYNC_WORKSPACE)
        .expect("the seeded workspace")
        .sync_jobs
        .get_mut(SYNC_SOURCE_SITEWISE)
        .expect("the seeded sync job");
    for row in SEEDED_RESOURCES {
        job.sync_resources.insert(
            row.external_id.to_string(),
            SyncResource {
                resource_type: row.resource_type.to_string(),
                external_id: row.external_id.to_string(),
                resource_id: Some(row.resource_id.to_string()),
                state: row.state.to_string(),
                error: None,
                update_date_time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            },
        );
    }
}

#[tokio::test]
async fn a_list_sync_resources_token_is_bound_to_the_filters_that_selected_its_page() {
    // The states and types the rows above are seeded in are the model's own, so
    // a filter naming one is a filter the listing admits.
    for state in [SYNCED, SYNCING] {
        assert!(
            SYNC_RESOURCE_STATE_VALUES.contains(&state),
            "the state {state:?} must be one the model admits, which are \
             {SYNC_RESOURCE_STATE_VALUES:?}"
        );
    }
    assert!(
        SYNC_RESOURCE_TYPE_VALUES.contains(&SYNCED_ENTITY),
        "the resource type {SYNCED_ENTITY:?} must be one the model admits, which are \
         {SYNC_RESOURCE_TYPE_VALUES:?}"
    );

    let emulator = Emulator::new();
    seed_sync_workspace(&emulator).await;

    let served = &emulator;
    let page = move |filters: Option<Value>, max_results: u32, token: Option<String>| async move {
        let mut body = json!({"maxResults": max_results});
        if let Some(filters) = filters {
            body["filters"] = filters;
        }
        if let Some(token) = token {
            body["nextToken"] = json!(token);
        }
        served
            .list_sync_resources(SYNC_WORKSPACE, SYNC_SOURCE_SITEWISE, body)
            .await
    };

    check_resumes(
        "ListSyncResources under a state filter",
        &page,
        "syncResources",
        "externalId",
        Some(json!([{"state": SYNCED}])),
        vec![
            Resume::under(
                "the whole request repeated",
                json!([{"state": SYNCED}]),
                true,
            ),
            Resume::under(
                "the same filter at another page size",
                json!([{"state": SYNCED}]),
                true,
            )
            .at_page_size(1),
            Resume::unfiltered(
                "no filters at all, whose sequence holds every resource the job synchronises",
                false,
            ),
            Resume::under(
                "an empty filter list, which is that whole-job sequence too",
                json!([]),
                false,
            ),
            Resume::under(
                "another state filtered on",
                json!([{"state": SYNCING}]),
                false,
            ),
            Resume::under(
                "a type-and-id combination filtered on instead",
                json!([{"resourceType": SYNCED_ENTITY}, {"resourceId": "alphaEntity"}]),
                false,
            ),
            Resume::under(
                "a type-and-external-id combination, listed id first",
                json!([{"externalId": "resAlpha"}, {"resourceType": SYNCED_ENTITY}]),
                false,
            ),
        ],
    )
    .await;

    // The rule in the other direction too: a token minted with NO filters names
    // a position in the whole-job sequence, so a request that NARROWS the
    // listing asks about another sequence and is refused — while either
    // spelling of "no filters" is that same whole-job sequence and resumes.
    check_resumes(
        "ListSyncResources with no filter",
        &page,
        "syncResources",
        "externalId",
        None,
        vec![
            Resume::unfiltered("no filters again, as the minting request named none", true),
            Resume::under(
                "an empty filter list, which selects the whole job just as well",
                json!([]),
                true,
            ),
            Resume::under(
                "a state filter, whose sequence leaves out the resources in other states",
                json!([{"state": SYNCED}]),
                false,
            ),
            Resume::under(
                "another state filtered on",
                json!([{"state": SYNCING}]),
                false,
            ),
            Resume::under(
                "a type-and-id combination, whose sequence holds a single resource",
                json!([{"resourceType": SYNCED_ENTITY}, {"resourceId": "alphaEntity"}]),
                false,
            ),
        ],
    )
    .await;

    // What the refusal protects: a caller that changes its filters is sent back
    // to the START of the sequence it now asks for, where every resource that
    // sequence holds is still reported — none of them lost to a resume key
    // taken in another sequence. Both sequences are read off the seeded rows,
    // so this states what the job holds rather than restating a page.
    let mut whole_job: Vec<String> = SEEDED_RESOURCES
        .iter()
        .map(|row| row.external_id.to_string())
        .collect();
    whole_job.sort();
    let mut synced: Vec<String> = SEEDED_RESOURCES
        .iter()
        .filter(|row| row.state == SYNCED)
        .map(|row| row.external_id.to_string())
        .collect();
    synced.sort();
    assert!(
        synced.len() < whole_job.len() && synced.first() == whole_job.first(),
        "the seeded rows must make the two sequences differ while starting at the same resource, \
         so a resume key of one names no position in the other: got {synced:?} of {whole_job:?}"
    );

    let whole = page(None, PAGE, None).await;
    assert_eq!(
        reported(&whole, "syncResources", "externalId"),
        whole_job,
        "the unfiltered listing must report every resource the job holds, got {whole:?}"
    );
    let filtered = page(Some(json!([{"state": SYNCED}])), PAGE, None).await;
    assert_eq!(
        reported(&filtered, "syncResources", "externalId"),
        synced,
        "the filtered listing must report every resource in that state, got {filtered:?}"
    );
}

/// The workspace the metadata transfer jobs below name at an end.
const JOB_WORKSPACE: &str = "filterScopeJobs";
/// A second workspace, so a `workspaceId` filter has something to EXCLUDE.
const JOB_OTHER_WORKSPACE: &str = "filterScopeJobsTwo";
/// The ends every job below transfers between: the pair the listing is taken
/// over.
const JOB_SOURCE_TYPE: &str = "s3";
const JOB_DESTINATION_TYPE: &str = "iottwinmaker";

/// One seeded metadata transfer job: its id, and the workspace the IoT
/// TwinMaker end of its transfer names.
struct SeededJob {
    metadata_transfer_job_id: &'static str,
    workspace_id: &'static str,
}

/// The metadata transfer jobs the `ListMetadataTransferJobs` cases page over:
/// three naming one workspace at their destination and three naming the other.
///
/// `jobAbacus` sorts BEFORE every job of the first workspace and `jobAnchor`
/// sits between two of them in `metadataTransferJobId` order, so each is a row
/// the whole-account sequence holds ahead of the id a one-row page of the
/// filtered sequence resumes at — exactly the rows a token resumed under other
/// filters would walk past.
const SEEDED_JOBS: &[SeededJob] = &[
    SeededJob {
        metadata_transfer_job_id: "jobAbacus",
        workspace_id: JOB_OTHER_WORKSPACE,
    },
    SeededJob {
        metadata_transfer_job_id: "jobAlpha",
        workspace_id: JOB_WORKSPACE,
    },
    SeededJob {
        metadata_transfer_job_id: "jobAnchor",
        workspace_id: JOB_OTHER_WORKSPACE,
    },
    SeededJob {
        metadata_transfer_job_id: "jobBravo",
        workspace_id: JOB_WORKSPACE,
    },
    SeededJob {
        metadata_transfer_job_id: "jobCharlie",
        workspace_id: JOB_WORKSPACE,
    },
    SeededJob {
        metadata_transfer_job_id: "jobDelta",
        workspace_id: JOB_OTHER_WORKSPACE,
    },
];

/// The workspaces and the metadata transfer jobs the
/// `ListMetadataTransferJobs` cases page over.
async fn seed_transfer_jobs(emulator: &Emulator) {
    for workspace_id in [JOB_WORKSPACE, JOB_OTHER_WORKSPACE] {
        let created = emulator.create_workspace(workspace_id).await;
        assert_eq!(
            created.status, 200,
            "CreateWorkspace {workspace_id:?}: expected 200, got {created:?}"
        );
    }

    for &SeededJob {
        metadata_transfer_job_id,
        workspace_id,
    } in SEEDED_JOBS
    {
        let created = emulator
            .create_metadata_transfer_job(json!({
                "metadataTransferJobId": metadata_transfer_job_id,
                "sources": [{
                    "type": JOB_SOURCE_TYPE,
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/entities.json"},
                }],
                "destination": {
                    "type": JOB_DESTINATION_TYPE,
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(workspace_id)},
                },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateMetadataTransferJob {metadata_transfer_job_id:?}: expected 200, got {created:?}"
        );
    }

    // Every created job is owed an outcome by the clock, and the emulator stores
    // the instant rather than sleeping for it, so the cases that filter on a
    // job's state are the ones that wait for it; the margin absorbs scheduling
    // jitter.
    sleep(METADATA_TRANSFER_JOB_SETTLE_SPAN + Duration::from_millis(100));
}

#[tokio::test]
async fn a_list_metadata_transfer_jobs_token_is_bound_to_the_filters_that_selected_its_page() {
    let emulator = Emulator::new();
    seed_transfer_jobs(&emulator).await;

    let served = &emulator;
    let page = move |filters: Option<Value>, max_results: u32, token: Option<String>| async move {
        let mut body = json!({
            "sourceType": JOB_SOURCE_TYPE,
            "destinationType": JOB_DESTINATION_TYPE,
            "maxResults": max_results,
        });
        if let Some(filters) = filters {
            body["filters"] = filters;
        }
        if let Some(token) = token {
            body["nextToken"] = json!(token);
        }
        served.list_metadata_transfer_jobs(body).await
    };

    // A listing taken under TWO filters, so a resume can change the conjunction
    // without changing which rows it selects — and can spell the same
    // conjunction another way.
    check_resumes(
        "ListMetadataTransferJobs under a workspace and a state filter",
        &page,
        "metadataTransferJobSummaries",
        "metadataTransferJobId",
        Some(json!([
            {"workspaceId": JOB_WORKSPACE},
            {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
        ])),
        vec![
            Resume::under(
                "the whole request repeated",
                json!([
                    {"workspaceId": JOB_WORKSPACE},
                    {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
                ]),
                true,
            ),
            Resume::under(
                "the two filters listed in the other order, which is the same conjunction",
                json!([
                    {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
                    {"workspaceId": JOB_WORKSPACE},
                ]),
                true,
            ),
            Resume::under(
                "the same two filters at another page size",
                json!([
                    {"workspaceId": JOB_WORKSPACE},
                    {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
                ]),
                true,
            )
            .at_page_size(1),
            Resume::under(
                "the state filter dropped, which is another conjunction even though it keeps the \
                 same rows",
                json!([{"workspaceId": JOB_WORKSPACE}]),
                false,
            ),
            Resume::under(
                "the workspace filter dropped",
                json!([{"state": METADATA_TRANSFER_JOB_STATE_ERROR}]),
                false,
            ),
            Resume::under(
                "another workspace filtered on",
                json!([
                    {"workspaceId": JOB_OTHER_WORKSPACE},
                    {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
                ]),
                false,
            ),
            Resume::under(
                "another state filtered on",
                json!([
                    {"workspaceId": JOB_WORKSPACE},
                    {"state": METADATA_TRANSFER_JOB_STATE_COMPLETED},
                ]),
                false,
            ),
            Resume::unfiltered(
                "no filters at all, whose sequence holds the jobs of both workspaces",
                false,
            ),
            Resume::under(
                "an empty filter list, which is that whole-account sequence too",
                json!([]),
                false,
            ),
        ],
    )
    .await;

    check_resumes(
        "ListMetadataTransferJobs under a workspace filter",
        &page,
        "metadataTransferJobSummaries",
        "metadataTransferJobId",
        Some(json!([{"workspaceId": JOB_WORKSPACE}])),
        vec![
            Resume::under(
                "the same filter repeated",
                json!([{"workspaceId": JOB_WORKSPACE}]),
                true,
            ),
            Resume::under(
                "that filter named twice, which leaves the conjunction the rows pass unchanged",
                json!([{"workspaceId": JOB_WORKSPACE}, {"workspaceId": JOB_WORKSPACE}]),
                true,
            ),
            Resume::under(
                "a state filter added, which narrows the conjunction",
                json!([
                    {"workspaceId": JOB_WORKSPACE},
                    {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
                ]),
                false,
            ),
            Resume::unfiltered("no filters at all", false),
        ],
    )
    .await;

    // What the refusal protects: a caller that changes its filters is sent back
    // to the START of the sequence it now asks for, where every job that
    // sequence holds is still reported — none of them lost to a resume key
    // taken in another sequence. Both sequences are read off the seeded jobs,
    // so this states what the account holds rather than restating a page.
    let mut whole_account: Vec<String> = SEEDED_JOBS
        .iter()
        .map(|job| job.metadata_transfer_job_id.to_string())
        .collect();
    whole_account.sort();
    let mut of_workspace: Vec<String> = SEEDED_JOBS
        .iter()
        .filter(|job| job.workspace_id == JOB_WORKSPACE)
        .map(|job| job.metadata_transfer_job_id.to_string())
        .collect();
    of_workspace.sort();
    // Where a one-row page of the filtered sequence resumes: its second job.
    let resume_key = &of_workspace[1];
    assert!(
        whole_account
            .iter()
            .any(|id| id < resume_key && !of_workspace.contains(id)),
        "the seeded jobs must put a job of the whole-account sequence before the id a filtered \
         page resumes at ({resume_key:?}), so resuming that token unfiltered would walk past it: \
         got {whole_account:?}, of which {of_workspace:?} are the workspace's"
    );

    // The walks the refusal keeps whole, at the page sizes this listing
    // distinguishes over these jobs: one per page, a size that divides the set,
    // one that does not, exactly the set and more than the set.
    for page_size in [1u32, 2, 4, 6, PAGE] {
        let walked = walk(
            "ListMetadataTransferJobs unfiltered",
            &page,
            "metadataTransferJobSummaries",
            "metadataTransferJobId",
            None,
            page_size,
        )
        .await;
        assert_eq!(
            walked, whole_account,
            "page size {page_size}: the unfiltered walk must report every job of the account \
             exactly once in id order, got {walked:?}"
        );

        let walked = walk(
            "ListMetadataTransferJobs under a workspace filter",
            &page,
            "metadataTransferJobSummaries",
            "metadataTransferJobId",
            Some(json!([{"workspaceId": JOB_WORKSPACE}])),
            page_size,
        )
        .await;
        assert_eq!(
            walked, of_workspace,
            "page size {page_size}: the filtered walk must report every job the filter selects \
             exactly once in id order, got {walked:?}"
        );
    }
}
