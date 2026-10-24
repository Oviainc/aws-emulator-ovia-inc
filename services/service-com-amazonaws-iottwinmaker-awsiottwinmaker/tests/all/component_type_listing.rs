// SPDX-License-Identifier: CC0-1.0

//! `ListComponentTypes` end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, the `workspaceId` label,
//! the status code and the serialized body are all exercised, not just a handler
//! body.
//!
//! The tests assert RULES over tables of inputs rather than one recorded case:
//!
//! * a listing reports every STORED component type of the workspace it names, in
//!   `componentTypeId` order, each summary carrying the members
//!   `GetComponentType` reports for that same component type — so a create is
//!   visible in the listing, a delete is not, and another workspace's component
//!   types never are;
//! * a page sequence partitions the listing exactly once, whatever the page size
//!   and however many component types are stored;
//! * the page size is the documented one and is bounded, and a `maxResults` or a
//!   `nextToken` outside what the service accepts is refused with the modelled
//!   `ValidationException`;
//! * a single filter object selects exactly the component types it names, and
//!   more than one is refused;
//! * listing writes nothing beyond the lifecycle transition the CLOCK already
//!   made due.

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    COMPONENT_TYPE_SETTLE_SPAN, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, STATE_ACTIVE, STATE_CREATING,
};

use crate::harness::{component_type_arn, Emulator, Resp};

/// The workspace every case that needs only one takes its listing in.
const WORKSPACE: &str = "ctListWorkspace";

/// Block until the `CREATING` -> `ACTIVE` transition a create owes has fallen
/// due. The emulator stores the instant and never sleeps for it, so a test that
/// wants to observe the settled state is the one that waits; the margin keeps
/// the wait off the span's own boundary.
fn wait_until_settled() {
    sleep(COMPONENT_TYPE_SETTLE_SPAN + Duration::from_millis(100));
}

/// The `componentTypeSummaries` of a response: a present array, never an absent
/// member — the model makes it required — even for a workspace holding none.
fn summaries_of(case: &str, resp: &Resp) -> Vec<Value> {
    resp.member("componentTypeSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: componentTypeSummaries must be a present array, got {resp:?}")
        })
}

/// The `componentTypeId`s of a page, in the order the page served them.
fn page_ids(case: &str, resp: &Resp) -> Vec<String> {
    summaries_of(case, resp)
        .iter()
        .map(|summary| {
            summary["componentTypeId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: every summary carries a componentTypeId, got {summary:?}"
                    )
                })
                .to_string()
        })
        .collect()
}

/// The page size the service serves for a requested `maxResults`: the documented
/// default of 25 when the caller names none, and never more than the service's
/// own ceiling.
fn effective_page_size(max_results: Option<u32>) -> usize {
    match max_results {
        None => DEFAULT_PAGE_SIZE,
        Some(size) => (size as usize).min(MAX_PAGE_SIZE),
    }
}

/// Create a workspace, asserting it worked.
async fn create_workspace(emulator: &Emulator, workspace_id: &str) {
    let created = emulator.create_workspace(workspace_id).await;
    assert_eq!(
        created.status, 200,
        "creating workspace {workspace_id:?} must succeed, got {created:?}"
    );
}

/// Create a component type, asserting it worked.
async fn create(emulator: &Emulator, workspace_id: &str, component_type_id: &str, body: Value) {
    let created = emulator
        .create_component_type(workspace_id, component_type_id, body)
        .await;
    assert_eq!(
        created.status, 200,
        "creating {component_type_id:?} in {workspace_id:?} must succeed, got {created:?}"
    );
}

/// Walk every page of the listing at page size `max_results`, returning the ids
/// in the order they were served (so a duplicate is visible) and the page count.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
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
        let resp = emulator.list_component_types(workspace_id, request).await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: page {pages} expected 200, got {resp:?}"
        );
        pages += 1;
        let page = page_ids(case, &resp);
        assert!(
            page.len() <= effective_page_size(max_results),
            "case {case:?}: page {pages} served {} rows, past the page size {}",
            page.len(),
            effective_page_size(max_results)
        );
        ids.extend(page);
        next_token = resp.member("nextToken").as_str().map(str::to_string);
        if next_token.is_none() {
            return (ids, pages);
        }
        assert!(
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
    }
}

/// RULE: a listing reports every stored component type of the workspace it
/// names, ordered by `componentTypeId`, each summary carrying the same `arn`,
/// `description`, `componentTypeName`, `status` and timestamps the
/// single-resource read reports — and nothing for a component type that is not
/// stored in that workspace.
///
/// The rows differ along which optional members the create persisted (none, a
/// name, a description, the empty description, parents, properties) and along
/// the shape of the id, and are created in an order that is NOT id order, so the
/// ordering assertion cannot pass by accident.
#[tokio::test]
async fn a_listing_reports_every_stored_component_type_as_the_single_read_does() {
    let emulator = Emulator::new();
    create_workspace(&emulator, WORKSPACE).await;
    create_workspace(&emulator, "otherWorkspace").await;

    // (componentTypeId, the create's body) — created in this scrambled order.
    let rows: Vec<(&str, Value)> = vec![
        (
            "mid.sensor",
            json!({"componentTypeName": "A sensor", "description": "the mid one"}),
        ),
        ("zz.last", json!({})),
        ("aa.first", json!({"description": ""})),
        (
            "com.example.cooling-pump",
            json!({
                "componentTypeName": "Kühlpumpe — 冷却ポンプ",
                "isSingleton": true,
                "propertyDefinitions": {"serial": {"dataType": {"type": "STRING"}}},
            }),
        ),
        (
            "n0.middle",
            json!({"extendsFrom": ["mid.sensor"], "description": "x"}),
        ),
    ];
    for (component_type_id, body) in &rows {
        create(&emulator, WORKSPACE, component_type_id, body.clone()).await;
    }
    // A component type of ANOTHER workspace, which this listing must never show.
    create(&emulator, "otherWorkspace", "mid.sensor", json!({})).await;

    let mut expected_ids: Vec<String> = rows
        .iter()
        .map(|(component_type_id, _)| component_type_id.to_string())
        .collect();
    expected_ids.sort();

    // Both the listing and the reads below are taken after the settle span, so
    // every record is in its terminal state and the two cannot straddle the
    // transition the clock owes.
    wait_until_settled();

    let case = "allStored";
    let listed = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListComponentTypes, got {listed:?}"
    );
    assert_eq!(
        page_ids(case, &listed),
        expected_ids,
        "case {case:?}: the listing must report every stored component type in id order, got \
         {listed:?}"
    );
    assert_eq!(
        listed.member("workspaceId").as_str(),
        Some(WORKSPACE),
        "case {case:?}: the response names the workspace it listed, got {listed:?}"
    );

    // Every summary says about a component type what the single-resource read
    // says.
    for summary in summaries_of(case, &listed) {
        let component_type_id = summary["componentTypeId"].as_str().unwrap_or_default();
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {component_type_id:?}: expected 200 from GetComponentType, got {read:?}"
        );
        assert_eq!(
            summary["arn"],
            json!(component_type_arn(WORKSPACE, component_type_id)),
            "case {component_type_id:?}: the summary must carry the minted ARN, got {summary:?}"
        );
        for member in [
            "arn",
            "componentTypeName",
            "description",
            "status",
            "creationDateTime",
            "updateDateTime",
        ] {
            assert_eq!(
                summary.get(member),
                read.body.get(member),
                "case {component_type_id:?}: the summary's {member} must be what \
                 GetComponentType reports ({:?}), got {:?}",
                read.body.get(member),
                summary.get(member)
            );
        }
    }

    // A component type that is no longer stored is no longer listed, and the
    // rest of the listing is untouched.
    let deleted = emulator.delete_component_type(WORKSPACE, "zz.last").await;
    assert_eq!(
        deleted.status, 200,
        "deleting the component type under test must succeed, got {deleted:?}"
    );
    expected_ids.retain(|component_type_id| component_type_id != "zz.last");
    let after_delete = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    assert_eq!(
        page_ids("afterDelete", &after_delete),
        expected_ids,
        "case \"afterDelete\": a deleted component type must not be listed, got {after_delete:?}"
    );

    // The other workspace holds exactly its own one.
    let other = emulator
        .list_component_types("otherWorkspace", json!({}))
        .await;
    assert_eq!(
        page_ids("otherWorkspace", &other),
        vec!["mid.sensor".to_string()],
        "case \"otherWorkspace\": a listing is scoped to the workspace it names, got {other:?}"
    );
}

/// RULE: the listing is scoped to a workspace this service holds — a request
/// naming one it does not is refused with the modelled `ValidationException`
/// naming that workspace, whatever page or filter members it carries, because
/// the scope is resolved before the request's own members.
///
/// The rows differ along the request's other members — including ones that are
/// THEMSELVES invalid — because the refusal is the scope's and not theirs.
#[tokio::test]
async fn a_listing_of_an_unknown_workspace_is_refused() {
    let emulator = Emulator::new();
    create_workspace(&emulator, WORKSPACE).await;

    let rows: Vec<(&str, Value)> = vec![
        ("bare", json!({})),
        ("paged", json!({"maxResults": 5})),
        (
            "filtered",
            json!({"filters": [{"namespace": "com.example"}]}),
        ),
        // Below the documented minimum of 1: the scope is still what the
        // refusal names.
        ("pageSizeBelowTheMinimum", json!({"maxResults": 0})),
        // A token this listing never minted: same.
        ("unmintedToken", json!({"nextToken": "not-a-token"})),
    ];
    for (case, request) in rows {
        let refused = emulator.list_component_types("noSuchWs", request).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: listing an unknown workspace must be refused, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains("noSuchWs")),
            "case {case:?}: the refusal must name the workspace it could not resolve, got \
             {refused:?}"
        );
    }

    // An EMPTY workspace, by contrast, lists an empty array and no token.
    let empty = emulator.list_component_types(WORKSPACE, json!({})).await;
    assert_eq!(
        (empty.status, summaries_of("emptyWorkspace", &empty).len()),
        (200, 0),
        "case \"emptyWorkspace\": a workspace with no component type lists an empty array, got \
         {empty:?}"
    );
    assert!(
        empty.member("nextToken").is_null(),
        "case \"emptyWorkspace\": an empty listing carries no token, got {empty:?}"
    );
}

/// RULE: a page sequence partitions the listing — every stored component type is
/// served exactly once, in `componentTypeId` order, no page is larger than the
/// page size, and only the last page carries no `nextToken`.
///
/// The rows span how many component types are stored (none, one, several, more
/// than one page at every size under test) crossed with the page size (the
/// documented default, one, several, and the top of the published range).
#[tokio::test]
async fn page_sequences_partition_the_listing_at_every_page_size() {
    let totals = [0usize, 1, 2, 7, DEFAULT_PAGE_SIZE + 5];
    let page_sizes: [Option<u32>; 6] = [
        None,
        Some(1),
        Some(2),
        Some(5),
        Some(DEFAULT_PAGE_SIZE as u32),
        Some(MAX_PAGE_SIZE as u32),
    ];

    for total in totals {
        let emulator = Emulator::new();
        create_workspace(&emulator, WORKSPACE).await;
        let mut expected_ids: Vec<String> = (0..total).map(|i| format!("ct{i:03}")).collect();
        for component_type_id in &expected_ids {
            create(&emulator, WORKSPACE, component_type_id, json!({})).await;
        }
        expected_ids.sort();

        for max_results in page_sizes {
            let case = format!("total={total} maxResults={max_results:?}");
            let (ids, pages) = walk_pages(&emulator, &case, WORKSPACE, max_results).await;
            assert_eq!(
                ids, expected_ids,
                "case {case:?}: the pages must serve every component type exactly once in id \
                 order, got {ids:?}"
            );
            let size = effective_page_size(max_results);
            let expected_pages = total.div_ceil(size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages at page size {size}, got {pages}"
            );
        }
    }
}

/// RULE: the page size is the one the service documents and bounds — a request
/// naming no `maxResults` is served the documented default of 25, a value inside
/// the published range is served as asked, and a value outside it is refused —
/// with a `nextToken` exactly when rows are left over, and the served page size
/// reported back in the response's own `maxResults`.
///
/// The rows are the published bounds of `maxResults` (1 and 200), one below the
/// documented minimum (0), one past the published maximum (201), values inside
/// the range, and the request that names none.
#[tokio::test]
async fn the_page_size_is_the_documented_one_and_is_bounded() {
    let emulator = Emulator::new();
    create_workspace(&emulator, WORKSPACE).await;

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

    // More component types than the default page serves, so a page is truncated
    // by the page size rather than by the stored count, and fewer than the top
    // of the range, so a page size at the top serves the whole set.
    let total = DEFAULT_PAGE_SIZE + 5;
    let mut all_ids: Vec<String> = (0..total).map(|i| format!("ct{i:03}")).collect();
    for component_type_id in &all_ids {
        create(&emulator, WORKSPACE, component_type_id, json!({})).await;
    }
    all_ids.sort();

    // (what the caller asks for, how many summaries the service serves)
    let rows: Vec<(Option<u32>, usize)> = vec![
        (None, DEFAULT_PAGE_SIZE),
        (Some(1), 1),
        (Some(7), 7),
        (Some(DEFAULT_PAGE_SIZE as u32), DEFAULT_PAGE_SIZE),
        (Some(MAX_PAGE_SIZE as u32), total),
    ];

    for (max_results, expected_len) in rows {
        let case = format!("maxResults={max_results:?}");
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        let page = emulator.list_component_types(WORKSPACE, request).await;
        assert_eq!(page.status, 200, "case {case:?}: expected 200, got {page:?}");
        let ids = page_ids(&case, &page);
        assert_eq!(
            ids.as_slice(),
            &all_ids[..expected_len],
            "case {case:?}: the page must serve the first {expected_len} component types, got \
             {ids:?}"
        );
        assert_eq!(
            page.member("maxResults").as_u64(),
            Some(effective_page_size(max_results) as u64),
            "case {case:?}: the response reports the page size it served, got {page:?}"
        );
        let token = page.member("nextToken").as_str().map(str::to_string);
        if expected_len == total {
            assert_eq!(
                token, None,
                "case {case:?}: a page holding the whole listing carries no token, got {page:?}"
            );
            continue;
        }
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a listing with rows left over must answer a token, got {page:?}")
        });
        let resumed = emulator
            .list_component_types(WORKSPACE, json!({"maxResults": 1, "nextToken": token}))
            .await;
        assert_eq!(
            page_ids(&case, &resumed).as_slice(),
            &all_ids[expected_len..=expected_len],
            "case {case:?}: the token must resume where the page stopped, got {resumed:?}"
        );
    }

    // Below the documented minimum of 1, and one past the published top of the
    // range (where the modelled constraint refuses it before the handler runs).
    for past_bound in [0u32, 201] {
        let refused = emulator
            .list_component_types(WORKSPACE, json!({"maxResults": past_bound}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case Some({past_bound}): a maxResults outside the documented range must be refused, \
             got {refused:?}"
        );
    }
}

/// RULE: a `nextToken` is only resumable as the token THIS listing minted for
/// THIS workspace — an absent or empty one starts at the beginning, and anything
/// else is refused with the modelled `ValidationException` rather than silently
/// resumed.
///
/// The rows differ along how the token is wrong: not a token at all, one mangled
/// in transit, one carrying no envelope, ones minted by this service's OTHER
/// listings, one minted by this listing for another workspace, and one whose
/// envelope is right but whose resume key is empty.
#[tokio::test]
async fn only_a_token_this_listing_minted_resumes_it() {
    let emulator = Emulator::new();
    create_workspace(&emulator, WORKSPACE).await;
    create_workspace(&emulator, "otherWorkspace").await;
    for component_type_id in ["ct.a", "ct.b", "ct.c"] {
        create(&emulator, WORKSPACE, component_type_id, json!({})).await;
        create(&emulator, "otherWorkspace", component_type_id, json!({})).await;
    }

    // An absent and an empty token both mean "the first page".
    let first = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": 2}))
        .await;
    let from_empty = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": 2, "nextToken": ""}))
        .await;
    assert_eq!(
        page_ids("emptyToken", &from_empty),
        page_ids("absentToken", &first),
        "case \"emptyToken\": an empty nextToken starts the listing, got {from_empty:?}"
    );

    // The token this listing minted for the OTHER workspace: a real token, for
    // the wrong scope.
    let other_workspace_token = emulator
        .list_component_types("otherWorkspace", json!({"maxResults": 2}))
        .await
        .member("nextToken")
        .as_str()
        .expect("the other workspace's first page leaves a row over, so it answers a token")
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("notATokenAtAll", "not-a-token".to_string()),
        // One of this listing's own tokens, mangled in transit so it is no
        // longer decodable at all.
        ("mangledToken", {
            let token = first
                .member("nextToken")
                .as_str()
                .expect("the first page leaves a row over, so it answers a token")
                .to_string();
            token[..token.len() - 1].to_string()
        }),
        // Decodable, but carrying no envelope: a bare component type id, which a
        // caller must not be able to pass as a resume key.
        ("hexWithoutEnvelope", hex::encode("ct.a")),
        // The envelopes this service's other listings mint; a token from one
        // listing is not a token for another.
        (
            "entityListingToken",
            hex::encode(format!("iottwinmaker:entities:1:pump\n{WORKSPACE}")),
        ),
        ("workspaceListingToken", hex::encode("iottwinmaker:workspaces:1:ws_a")),
        // This listing's envelope, minted for another workspace.
        ("otherWorkspaceToken", other_workspace_token),
        // Our own envelope and scope, with no resume key inside it.
        (
            "emptyResumeKey",
            hex::encode(format!("iottwinmaker:componentTypes:1:\n{WORKSPACE}")),
        ),
    ];

    for (case, token) in &rows {
        let refused = emulator
            .list_component_types(WORKSPACE, json!({"maxResults": 2, "nextToken": token}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a token this listing did not mint must be refused, got {refused:?}"
        );
    }
}

/// RULE: ONE filter object selects exactly the stored component types it names —
/// `extendsFrom` those whose own declared parents name the id it names,
/// `namespace` those whose id lies in the namespace it names, `isAbstract` those
/// whose abstractness matches — and more than one object is refused, because
/// "only one object is accepted as a valid input".
///
/// The rows span every member of the filter union, each on a value the stored
/// set both matches and misses: a parent with two children, one with two, one
/// with none, and a GRANDparent (whose grandchild is not selected, since a
/// component type's stored parents are the ones its create declared); a
/// namespace at three depths, a whole id and one no id lies in; and both
/// booleans.
#[tokio::test]
async fn one_filter_object_selects_the_component_types_it_names() {
    let emulator = Emulator::new();
    create_workspace(&emulator, WORKSPACE).await;

    // The stored set: ids at two namespace depths and one with no namespace at
    // all, with a declared `extendsFrom` on some of them.
    let stored: Vec<(&str, Value)> = vec![
        ("com.example.base", json!({})),
        (
            "com.example.sensors.pump",
            json!({"extendsFrom": ["com.example.base"]}),
        ),
        (
            "com.example.sensors.valve",
            json!({"extendsFrom": ["com.example.base", "com.example.sensors.pump"]}),
        ),
        // A GRANDchild of `com.example.base`: its own create declared only the
        // pump as a parent.
        (
            "com.example.sensors.deep",
            json!({"extendsFrom": ["com.example.sensors.pump"]}),
        ),
        ("com.other.widget", json!({})),
        ("plainType", json!({})),
    ];
    for (component_type_id, body) in &stored {
        create(&emulator, WORKSPACE, component_type_id, body.clone()).await;
    }
    let mut every_id: Vec<String> = stored
        .iter()
        .map(|(component_type_id, _)| component_type_id.to_string())
        .collect();
    every_id.sort();

    // (case, the single filter object, the ids it must select in id order)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "extendsFromWithTwoChildren",
            json!({"extendsFrom": "com.example.base"}),
            // The grandchild is NOT here: this service resolves no inheritance
            // chain beyond the parents a create declared.
            vec!["com.example.sensors.pump", "com.example.sensors.valve"],
        ),
        (
            "extendsFromWithTwoDirectChildren",
            json!({"extendsFrom": "com.example.sensors.pump"}),
            vec!["com.example.sensors.deep", "com.example.sensors.valve"],
        ),
        (
            "extendsFromWithNoChild",
            json!({"extendsFrom": "plainType"}),
            vec![],
        ),
        (
            "namespaceAtTheTop",
            json!({"namespace": "com"}),
            vec![
                "com.example.base",
                "com.example.sensors.deep",
                "com.example.sensors.pump",
                "com.example.sensors.valve",
                "com.other.widget",
            ],
        ),
        (
            "namespaceOneDeeper",
            json!({"namespace": "com.example"}),
            vec![
                "com.example.base",
                "com.example.sensors.deep",
                "com.example.sensors.pump",
                "com.example.sensors.valve",
            ],
        ),
        (
            "namespaceTwoDeeper",
            json!({"namespace": "com.example.sensors"}),
            vec![
                "com.example.sensors.deep",
                "com.example.sensors.pump",
                "com.example.sensors.valve",
            ],
        ),
        (
            // A whole id is not a namespace of itself: nothing continues it
            // with a further segment.
            "namespaceThatIsAWholeId",
            json!({"namespace": "com.example.base"}),
            vec![],
        ),
        (
            // An id carrying no dotted prefix lies in no namespace.
            "namespaceOfANamespacelessId",
            json!({"namespace": "plainType"}),
            vec![],
        ),
        (
            // Every component type created through this API is concrete, which
            // is what `GetComponentType` reports for each of them.
            "isAbstractFalse",
            json!({"isAbstract": false}),
            every_id.iter().map(String::as_str).collect(),
        ),
        ("isAbstractTrue", json!({"isAbstract": true}), vec![]),
    ];

    for (case, filter, expected) in &rows {
        let listed = emulator
            .list_component_types(
                WORKSPACE,
                json!({"maxResults": MAX_PAGE_SIZE, "filters": [filter]}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from a filtered listing, got {listed:?}"
        );
        assert_eq!(
            page_ids(case, &listed),
            *expected,
            "case {case:?}: the filter must select exactly {expected:?}, got {listed:?}"
        );
    }

    // A filtered listing still paginates: the filter picks the rows, the page
    // size cuts them.
    let paged = emulator
        .list_component_types(
            WORKSPACE,
            json!({"maxResults": 1, "filters": [{"namespace": "com.example"}]}),
        )
        .await;
    assert_eq!(
        page_ids("filteredFirstPage", &paged),
        vec!["com.example.base".to_string()],
        "case \"filteredFirstPage\": a filtered page is cut by maxResults, got {paged:?}"
    );
    let token = paged
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("a filtered page with rows left over answers a token, got {paged:?}"))
        .to_string();
    let resumed = emulator
        .list_component_types(
            WORKSPACE,
            json!({
                "maxResults": MAX_PAGE_SIZE,
                "nextToken": token,
                "filters": [{"namespace": "com.example"}],
            }),
        )
        .await;
    assert_eq!(
        page_ids("filteredResumed", &resumed),
        vec![
            "com.example.sensors.deep".to_string(),
            "com.example.sensors.pump".to_string(),
            "com.example.sensors.valve".to_string(),
        ],
        "case \"filteredResumed\": the token resumes the filtered listing, got {resumed:?}"
    );

    // More than one filter object is refused.
    let refused = emulator
        .list_component_types(
            WORKSPACE,
            json!({"filters": [{"namespace": "com.example"}, {"isAbstract": false}]}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"twoFilters\": only one filter object is accepted, got {refused:?}"
    );

    // An empty filter list filters nothing.
    let unfiltered = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE, "filters": []}))
        .await;
    assert_eq!(
        page_ids("emptyFilterList", &unfiltered),
        every_id,
        "case \"emptyFilterList\": no filter object selects everything, got {unfiltered:?}"
    );
}

/// RULE: listing is a READ — the only thing it writes is the lifecycle
/// transition the CLOCK already made due, so a component type inside its settle
/// span lists as `CREATING`, the same component type lists as `ACTIVE` once the
/// span has elapsed whether or not anybody polled, and nothing else a later
/// caller sees changes.
///
/// The rows are the page sizes and resume points a caller can drive the listing
/// at; after every one, the whole listing and each component type's
/// single-resource read must be what they were before.
#[tokio::test]
async fn listing_writes_only_the_transition_the_clock_owed() {
    let emulator = Emulator::new();
    create_workspace(&emulator, WORKSPACE).await;

    // Inside the settle span the listing reports the non-terminal state the
    // create answered with — the same state `GetComponentType` reports.
    create(&emulator, WORKSPACE, "ct.settling", json!({})).await;
    let during = emulator.list_component_types(WORKSPACE, json!({})).await;
    assert_eq!(
        summaries_of("duringSettleSpan", &during)
            .first()
            .and_then(|summary| summary["status"]["state"].as_str()),
        Some(STATE_CREATING),
        "case \"duringSettleSpan\": a listing inside the settle span reports CREATING, got \
         {during:?}"
    );

    for component_type_id in ["ct.a", "ct.b", "ct.c"] {
        create(
            &emulator,
            WORKSPACE,
            component_type_id,
            json!({"description": component_type_id}),
        )
        .await;
    }
    let component_type_ids = ["ct.a", "ct.b", "ct.c", "ct.settling"];

    wait_until_settled();

    let snapshot = |resp: &Resp| resp.body.clone();
    let settled = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": 50}))
        .await;
    let before_listing = snapshot(&settled);
    for summary in summaries_of("settled", &settled) {
        assert_eq!(
            summary["status"]["state"].as_str(),
            Some(STATE_ACTIVE),
            "case \"settled\": the span has elapsed, so every listed state is ACTIVE, got \
             {summary:?}"
        );
    }
    let mut before_reads: BTreeMap<&str, Value> = BTreeMap::new();
    for component_type_id in component_type_ids {
        before_reads.insert(
            component_type_id,
            snapshot(
                &emulator
                    .get_component_type(WORKSPACE, component_type_id)
                    .await,
            ),
        );
    }

    for max_results in [None, Some(1u32), Some(2), Some(50)] {
        let case = format!("maxResults={max_results:?}");
        let (ids, _) = walk_pages(&emulator, &case, WORKSPACE, max_results).await;
        assert_eq!(
            ids.len(),
            component_type_ids.len(),
            "case {case:?}: the walk must serve every component type, got {ids:?}"
        );

        let again = snapshot(
            &emulator
                .list_component_types(WORKSPACE, json!({"maxResults": 50}))
                .await,
        );
        assert_eq!(
            again, before_listing,
            "case {case:?}: listing must not change what a later listing reports, got {again:?}"
        );
        for component_type_id in component_type_ids {
            let read = snapshot(
                &emulator
                    .get_component_type(WORKSPACE, component_type_id)
                    .await,
            );
            assert_eq!(
                read, before_reads[component_type_id],
                "case {case:?}: listing must not change what GetComponentType reports for \
                 {component_type_id:?}, got {read:?}"
            );
        }
    }
}
