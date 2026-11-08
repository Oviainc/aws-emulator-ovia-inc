// SPDX-License-Identifier: CC0-1.0

//! `ListSourceApiAssociations` end to end over HTTP: the associations of the API the
//! path addresses, paged.
//!
//! The rules asserted here, each over a TABLE or a class of inputs rather than over a
//! single request:
//!
//! 1. the listing reports every association the addressed API takes part in, on EITHER
//!    side — the path addresses a plain `apis/{apiId}` collection with no side
//!    qualifier, and each summary projects the same stored association
//!    `GetSourceApiAssociation` reads back — asserted over an association graph
//!    spanning a Merged API with several source APIs, a source API in several
//!    associations, APIs in none, both sides a link can be created from, and
//!    `description` present and absent;
//! 2. the page is cut in creation order: `maxResults` bounds its size, `nextToken`
//!    resumes exactly after the last item returned, and the pages of one listing
//!    concatenate to the whole set with nothing repeated or dropped — asserted over
//!    page sizes from one to past the set and over both published bounds of the
//!    modelled range, with one value past each refused;
//! 3. membership follows the CLOCK: an association whose deletion the clock has made
//!    due is gone from the listing with no call in between, while one whose deletion is
//!    merely scheduled is still listed;
//! 4. an `apiId` that cannot name an API is a 400 `BadRequestException`, one that names
//!    no API this account holds a 404 `NotFoundException`, and a `nextToken` the listing
//!    cannot resume from a 400.
//!
//! The harness (`router`, `call`, `create_api`, `get_path`) is the one the association
//! suites already carry, so this suite drives the same router the production emulator
//! mounts: routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, create_api, get_path, router, Api};

/// How long a row that must observe the far side of the scheduled deletion waits: the
/// service's own span plus slack. Only the TEST waits — the handler stores the due
/// instant.
const PAST_THE_DELETION: Duration = Duration::from_millis(750);

/// `ListSourceApiAssociations`' path for `api_id`, with `query` appended verbatim.
fn list_path(api_id: &str, query: &str) -> String {
    format!("/v1/apis/{api_id}/sourceApiAssociations{query}")
}

/// `DisassociateSourceGraphqlApi`'s path, i.e. one association OF a Merged API.
fn disassociate_path(merged_api_identifier: &str, association_id: &str) -> String {
    format!("/v1/mergedApis/{merged_api_identifier}/sourceApiAssociations/{association_id}")
}

/// One association the fixture creates: the pair it links, the side the create
/// addressed, and the `description` it carries.
#[derive(Debug)]
struct Link {
    /// Index into the fixture's source APIs.
    source: usize,
    /// Index into the fixture's Merged APIs.
    merged: usize,
    /// Whether the link was created from the SOURCE API's side
    /// (`AssociateMergedGraphqlApi`) rather than from the Merged API's: AppSync creates
    /// one and the same association either way.
    from_the_source_side: bool,
    description: Option<&'static str>,
}

/// One created association: the ids a listing of either of its sides must report.
#[derive(Debug, Clone)]
struct Created {
    association_id: String,
    /// The Merged API `GetSourceApiAssociation` reads this association back under.
    merged_api_id: String,
}

/// Associate the pair `link` names and answer the association the service minted.
async fn associate(
    router: &MultiServiceRouter,
    link: &Link,
    source: &Api,
    merged: &Api,
) -> Created {
    let (path, mut body) = if link.from_the_source_side {
        (
            format!("/v1/sourceApis/{}/mergedApiAssociations", source.id),
            json!({"mergedApiIdentifier": merged.id}),
        )
    } else {
        (
            format!("/v1/mergedApis/{}/sourceApiAssociations", merged.id),
            json!({"sourceApiIdentifier": source.id}),
        )
    };
    if let Some(description) = link.description {
        body["description"] = json!(description);
    }

    let answer = call(router, "POST", &path, Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case associate {link:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    Created {
        association_id: answer.body["sourceApiAssociation"]["associationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        merged_api_id: merged.id.clone(),
    }
}

/// The association graph rule 1 is asserted over: a Merged API holding three source
/// APIs, a second Merged API holding one of its own (a source API belongs to ONE Merged
/// API, so no source API appears twice), a Merged API and a source API in no association
/// at all, both sides a link can be created from, and `description` present and absent.
fn links() -> Vec<Link> {
    vec![
        Link {
            source: 0,
            merged: 0,
            from_the_source_side: true,
            description: Some("the first source API of this Merged API"),
        },
        Link {
            source: 1,
            merged: 0,
            from_the_source_side: false,
            description: None,
        },
        Link {
            source: 2,
            merged: 0,
            from_the_source_side: true,
            description: Some("troisième — déjà associée"),
        },
        Link {
            source: 3,
            merged: 1,
            from_the_source_side: false,
            description: Some("a source API of its own, under a second Merged API"),
        },
    ]
}

/// Create `sources` standard APIs and `mergeds` Merged APIs, labelled for the row.
async fn create_apis(
    router: &MultiServiceRouter,
    label: &str,
    sources: usize,
    mergeds: usize,
) -> (Vec<Api>, Vec<Api>) {
    let mut source_apis = Vec::new();
    for index in 0..sources {
        source_apis.push(
            create_api(
                router,
                &format!("{label}-source-{index}"),
                state::GRAPHQL_API_TYPE,
            )
            .await,
        );
    }
    let mut merged_apis = Vec::new();
    for index in 0..mergeds {
        merged_apis.push(
            create_api(
                router,
                &format!("{label}-merged-{index}"),
                state::MERGED_API_TYPE,
            )
            .await,
        );
    }
    (source_apis, merged_apis)
}

/// The `associationId`s one listing reports, read page by page through `nextToken` with
/// pages of `max_results` (`None` naming no `maxResults` at all).
async fn list_ids(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<i32>,
    case: &str,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded by the number of associations the fixture created, so a `nextToken` that
    // failed to advance ends the loop with a failing assertion rather than hanging.
    for page_number in 0..64 {
        let mut query = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("maxResults={size}"));
        }
        if let Some(cursor) = token.as_deref() {
            query.push(format!("nextToken={cursor}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };

        let answer = call(router, "GET", &list_path(api_id, &query), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case} page {page_number}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let page = answer.body["sourceApiAssociationSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case} page {page_number}: expected the summary list, got {:?}", answer.body));
        if let Some(size) = max_results.filter(|size| *size > 0) {
            assert!(
                page.len() <= size as usize,
                "case {case} page {page_number}: expected at most {size} summaries, got {}",
                page.len()
            );
        }
        for summary in &page {
            ids.push(
                summary["associationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }

        token = answer.body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            return ids;
        }
    }
    panic!("case {case}: the listing never reached its last page (ids so far: {ids:?})");
}

/// Rule 1: the listing reports every association the addressed API takes part in, on
/// either side, and each summary projects the stored association the reader reports.
#[tokio::test]
async fn a_listing_reports_every_association_of_the_api_it_addresses() {
    let router = router();
    let (sources, mergeds) = create_apis(&router, "graph", 5, 3).await;

    let links = links();
    let mut created: Vec<Created> = Vec::new();
    for link in &links {
        created.push(associate(&router, link, &sources[link.source], &mergeds[link.merged]).await);
    }

    // Every API of the fixture, with the associations it takes part in — in creation
    // order, which is the order the fixture created the links in.
    let mut expectations: Vec<(&Api, Vec<Created>)> = Vec::new();
    for (index, api) in sources.iter().enumerate() {
        let mine = links
            .iter()
            .zip(&created)
            .filter(|(link, _)| link.source == index)
            .map(|(_, association)| association.clone())
            .collect();
        expectations.push((api, mine));
    }
    for (index, api) in mergeds.iter().enumerate() {
        let mine = links
            .iter()
            .zip(&created)
            .filter(|(link, _)| link.merged == index)
            .map(|(_, association)| association.clone())
            .collect();
        expectations.push((api, mine));
    }

    for (api, expected) in expectations {
        let case = format!("listing {}", api.id);
        let listed = list_ids(&router, &api.id, Some(25), &case).await;
        let expected_ids: Vec<String> = expected
            .iter()
            .map(|association| association.association_id.clone())
            .collect();
        assert_eq!(
            listed, expected_ids,
            "case {case}: expected the associations this API takes part in, {expected_ids:?}, \
             got {listed:?}"
        );
    }

    // Each summary reports the SAME stored association the reader reports, member for
    // member — the summary's members are the ids, the ARNs and the description.
    for association in &created {
        let case = format!("summary of {}", association.association_id);
        let answer = call(
            &router,
            "GET",
            &list_path(&association.merged_api_id, "?maxResults=25"),
            None,
        )
        .await;
        let summary = answer.body["sourceApiAssociationSummaries"]
            .as_array()
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| {
                        summary["associationId"].as_str() == Some(&association.association_id)
                    })
                    .cloned()
            })
            .unwrap_or_else(|| {
                panic!("case {case}: expected the listing to hold it, got {:?}", answer.body)
            });

        let read = call(
            &router,
            "GET",
            &get_path(&association.merged_api_id, &association.association_id),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetSourceApiAssociation 200, got {} {:?}",
            read.status, read.body
        );
        let stored = read.body["sourceApiAssociation"].clone();

        for member in [
            "associationId",
            "associationArn",
            "sourceApiId",
            "sourceApiArn",
            "mergedApiId",
            "mergedApiArn",
            "description",
        ] {
            assert_eq!(
                summary[member], stored[member],
                "case {case}: expected {member} {:?} (what the reader reports), got {:?}",
                stored[member], summary[member]
            );
        }
        // The summary carries the ids, ARNs and description only: the merge
        // configuration and status belong to the full shape
        // (`smithy_query(query="shape", shape="SourceApiAssociationSummary")`).
        for member in [
            "sourceApiAssociationConfig",
            "sourceApiAssociationStatus",
            "sourceApiAssociationStatusDetail",
        ] {
            assert_eq!(
                summary[member],
                Value::Null,
                "case {case}: expected the summary not to carry {member}, got {:?}",
                summary[member]
            );
        }
    }
}

/// Rule 2: the page is cut in creation order, `maxResults` bounds it and `nextToken`
/// resumes after it, so the pages of one listing concatenate to the whole set.
#[tokio::test]
async fn a_listing_pages_through_its_associations_in_creation_order() {
    let router = router();
    let (sources, mergeds) = create_apis(&router, "paged", 5, 1).await;
    let merged = &mergeds[0];

    let mut expected: Vec<String> = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let link = Link {
            source: index,
            merged: 0,
            from_the_source_side: index % 2 == 0,
            description: None,
        };
        expected.push(
            associate(&router, &link, source, merged)
                .await
                .association_id,
        );
    }

    // Page sizes from one item to past the whole set, the modelled maximum, and no
    // `maxResults` at all — every one of which must page over the same set in the same
    // order. Source for the bounds: `smithy_query(query="shape", shape="MaxResults")`
    // — range 0..25, default 0 ("unspecified").
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(25), Some(0), None] {
        let case = format!("maxResults={max_results:?}");
        let listed = list_ids(&router, &merged.id, max_results, &case).await;
        assert_eq!(
            listed, expected,
            "case {case}: expected every association once, in creation order, {expected:?}, \
             got {listed:?}"
        );
    }

    // Both published bounds of the modelled range are accepted and one value past each
    // is refused, with the limit and the range the refusal states.
    for value in [0, 25, -1, 26] {
        let case = format!("maxResults at {value}");
        let answer = call(
            &router,
            "GET",
            &list_path(&merged.id, &format!("?maxResults={value}")),
            None,
        )
        .await;
        if (0..=25).contains(&value) {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected a published bound to be accepted, got {} {:?}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case}: expected status 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {case}: expected error type BadRequestException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some("Invalid maxResults given. Must fall within range 0 to 25"),
            "case {case}: expected the limit and its modelled range, got {:?}",
            answer.body
        );
    }
}

/// Rule 3: membership follows the clock — a scheduled deletion is still listed, and the
/// association is gone from the listing once that deletion has come due, with no call in
/// between.
#[tokio::test]
async fn the_listing_drops_an_association_once_its_deletion_comes_due() {
    let router = router();
    let (sources, mergeds) = create_apis(&router, "clock", 2, 1).await;
    let merged = &mergeds[0];

    let mut created: Vec<Created> = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let link = Link {
            source: index,
            merged: 0,
            from_the_source_side: index % 2 == 0,
            description: None,
        };
        created.push(associate(&router, &link, source, merged).await);
    }
    let doomed = created[0].association_id.clone();
    let kept = created[1].association_id.clone();

    let disassociated = call(
        &router,
        "DELETE",
        &disassociate_path(&merged.id, &doomed),
        None,
    )
    .await;
    assert_eq!(
        disassociated.status, 200,
        "case disassociate: expected 200, got {} {:?}",
        disassociated.status, disassociated.body
    );

    // While the deletion is merely SCHEDULED the association still exists, so the
    // listing still reports it — on both sides of it.
    for api_id in [merged.id.as_str(), sources[0].id.as_str()] {
        let case = format!("scheduled deletion, listing {api_id}");
        let listed = list_ids(&router, api_id, Some(25), &case).await;
        assert!(
            listed.contains(&doomed),
            "case {case}: expected the scheduled-for-deletion {doomed:?} to still be listed, \
             got {listed:?}"
        );
    }

    tokio::time::sleep(PAST_THE_DELETION).await;

    let listed = list_ids(&router, &merged.id, Some(25), "past the deletion").await;
    assert_eq!(
        listed,
        vec![kept.clone()],
        "case past the deletion: expected only the association that was not deleted, \
         [{kept:?}], got {listed:?}"
    );
    let listed = list_ids(&router, &sources[0].id, Some(25), "past the deletion, source side").await;
    assert!(
        listed.is_empty(),
        "case past the deletion, source side: expected no association left, got {listed:?}"
    );
}

/// Rule 4: an `apiId` that cannot name an API is a 400, one that names no API this
/// account holds a 404, and a `nextToken` the listing cannot resume from a 400.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_resolve() {
    let router = router();
    let (sources, mergeds) = create_apis(&router, "refused", 1, 1).await;
    let merged = &mergeds[0];
    let link = Link {
        source: 0,
        merged: 0,
        from_the_source_side: true,
        description: None,
    };
    let association = associate(&router, &link, &sources[0], merged).await;

    // An id longer than an AppSync API id names no API at all, so the request is
    // malformed; one of the right shape that names nothing is a missing resource.
    let too_long = "z".repeat(state::API_ID_LEN + 1);
    let absent = "a".repeat(state::API_ID_LEN);
    let rows: Vec<(&str, &str, u16, &str, String)> = vec![
        (
            "an apiId longer than an API id",
            too_long.as_str(),
            400,
            "BadRequestException",
            state::invalid_api_id_message(&too_long),
        ),
        (
            "an apiId of the right shape naming no API",
            absent.as_str(),
            404,
            "NotFoundException",
            state::API_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a single-character apiId",
            "x",
            404,
            "NotFoundException",
            state::API_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];
    for (case, api_id, status, error_type, message) in rows {
        let answer = call(&router, "GET", &list_path(api_id, ""), None).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected status {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, error_type,
            "case {case:?}: expected error type {error_type}, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(message.as_str()),
            "case {case:?}: expected message {message:?}, got {:?}",
            answer.body
        );
    }

    // A `nextToken` that names no item of THIS listing cannot be resumed from —
    // including one that names an association of another API's listing.
    let other = create_api(&router, "refused-other-merged", state::MERGED_API_TYPE).await;
    for (case, token) in [
        ("a token naming nothing", "nope".to_string()),
        (
            "a token naming an association of another API",
            association.association_id.clone(),
        ),
    ] {
        let answer = call(
            &router,
            "GET",
            &list_path(&other.id, &format!("?nextToken={token}")),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected status 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {case:?}: expected error type BadRequestException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(state::INVALID_PAGINATION_TOKEN_MESSAGE),
            "case {case:?}: expected the fixed pagination-token refusal, got {:?}",
            answer.body
        );
    }
}
