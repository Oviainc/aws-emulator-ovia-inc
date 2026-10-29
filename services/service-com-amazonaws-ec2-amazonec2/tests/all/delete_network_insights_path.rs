// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteNetworkInsightsPath`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DeleteNetworkInsightsPath`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. the delete TAKES THE PATH THE REQUEST NAMES OUT OF THE STORE and answers that id:
//!    every other path of the account is left where it was, the bytes the removed record
//!    held go back to the store's budget, and the path is gone for every later caller —
//!    `StartNetworkInsightsAnalysis`, which resolves a path id against the same store, no
//!    longer resolves it;
//! 2. an id this account holds no path under is REFUSED — whatever its form, including the
//!    id of a path an earlier call already deleted — with EC2's `InvalidParameterValue`
//!    stating that the `NetworkInsightsPathId` it was given is not a valid one, and nothing
//!    is removed; a request naming no path at all is refused at the boundary;
//! 3. a `DryRun` reports `DryRunOperation` and removes nothing, while `DryRun=false` is an
//!    ordinary delete — and a dry run naming a path the account does not hold is refused
//!    with that reason instead.
//!
//! Each table row is a distinct input along what the rules generalise over — what the stored
//! path names (a resource pair, an address pair, a source alone), which protocol and whether
//! a destination port is set, whether filters scope it at the source or the destination, how
//! many tags it carries and how long their values are, which of the seeded paths is deleted
//! and in which order, and each way a value can fail to name a path of the account — so no
//! one recorded call is the specification.

use ec2::ops::types::{FilterPortRange, PathFilter, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInsightsPathSpec, DRY_RUN_OPERATION,
    INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    NETWORK_INSIGHTS_ANALYSIS_ID_PREFIX, NETWORK_INSIGHTS_PATH_ID_PREFIX,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One `DeleteNetworkInsightsPath` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DeleteNetworkInsightsPath", params).await
}

/// One `DeleteNetworkInsightsPath` call naming `path_id`.
async fn delete_path(router: &MultiServiceRouter, path_id: &str) -> (u16, String) {
    delete(router, &[param("NetworkInsightsPathId", path_id)]).await
}

/// The refusal a `StartNetworkInsightsAnalysis` of `path_id` answers, as its status and
/// error code: the reader this suite proves a delete through. That operation resolves its
/// `NetworkInsightsPathId` against the SAME store the delete removes from, so a path it no
/// longer resolves is a path no later caller can reach. (`DescribeNetworkInsightsPaths` reads
/// that same store, and the suite of its own asserts a deleted path stops being reported.)
async fn analysis_start(
    router: &MultiServiceRouter,
    path_id: &str,
    client_token: &str,
) -> (u16, String) {
    call(
        router,
        REGION,
        "StartNetworkInsightsAnalysis",
        &[
            param("NetworkInsightsPathId", path_id),
            param("ClientToken", client_token),
        ],
    )
    .await
}

/// The `<Message>` of a refusal, read out of the PARSED XML envelope: the body has to be
/// well-formed XML and the element's own text is what is answered, so a caller-controlled
/// value the service wrote into the markup unescaped cannot read back as the message it
/// belongs to.
fn refusal_message(case: &str, body: &str) -> String {
    let document = roxmltree::Document::parse(body).unwrap_or_else(|error| {
        panic!("case {case:?}: a refusal must be well-formed XML ({error}): {body}")
    });
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Message")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {body}"))
        .to_string()
}

/// How many Reachability Analyzer paths the store holds.
fn stored_paths(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().network_insights_paths.len()
}

/// The caller-chosen bytes the store currently holds against its one byte budget.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// `Tag`s from `(key, value)` pairs.
fn tags(pairs: &[(&str, &str)]) -> Vec<Tag> {
    pairs
        .iter()
        .map(|(key, value)| Tag {
            key: Some((*key).to_string()),
            value: Some((*value).to_string()),
        })
        .collect()
}

/// One `PathFilter` scoping a path at one end.
fn path_filter(address: &str, from_port: i32, to_port: i32) -> PathFilter {
    PathFilter {
        source_address: Some(address.to_string()),
        source_port_range: Some(FilterPortRange {
            from_port: Some(from_port),
            to_port: Some(to_port),
        }),
        destination_address: None,
        destination_port_range: None,
    }
}

/// The paths every table here is written against, as `(case, what a create stores for it)`.
///
/// They differ along everything a delete could plausibly be sensitive to: whether the path
/// names resources, addresses or only a source, which protocol and whether a destination
/// port is set, whether a filter scopes it at the source or at the destination, and how many
/// tags it carries and how long their values are (the record's charge against the store's
/// byte budget, which the delete has to give back).
fn fixtures() -> Vec<(&'static str, NetworkInsightsPathSpec)> {
    vec![
        (
            "a resource-to-resource tcp path on port 443, untagged",
            NetworkInsightsPathSpec {
                source: Some("igw-0123456789abcdef0".to_string()),
                destination: Some("i-0123456789abcdef0".to_string()),
                protocol: Some("tcp".to_string()),
                destination_port: Some(443),
                region: REGION.to_string(),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "an address-to-address udp path with no port and one tag",
            NetworkInsightsPathSpec {
                source: Some("eni-0123456789abcdef0".to_string()),
                destination: Some("eni-0fedcba9876543210".to_string()),
                source_ip: Some("10.0.1.4".to_string()),
                destination_ip: Some("10.0.2.7".to_string()),
                protocol: Some("udp".to_string()),
                region: REGION.to_string(),
                tags: tags(&[("Name", "east-west")]),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "a path filtered at both ends with three tags",
            NetworkInsightsPathSpec {
                source: Some("nlb-0123456789abcdef0".to_string()),
                destination: Some("i-0fedcba9876543210".to_string()),
                protocol: Some("tcp".to_string()),
                destination_port: Some(0),
                filter_at_source: Some(path_filter("10.0.0.0/16", 1024, 65535)),
                filter_at_destination: Some(path_filter("192.0.2.0/24", 0, 1023)),
                region: REGION.to_string(),
                tags: tags(&[("Name", "filtered"), ("Owner", "TeamB"), ("Tier", "prod")]),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "a path with a source and a protocol only",
            NetworkInsightsPathSpec {
                source: Some("vpc-0123456789abcdef0".to_string()),
                protocol: Some("tcp".to_string()),
                region: REGION.to_string(),
                ..NetworkInsightsPathSpec::default()
            },
        ),
        (
            "a path whose tag values are long",
            NetworkInsightsPathSpec {
                source: Some("tgw-0123456789abcdef0".to_string()),
                destination: Some("i-00000000000000001".to_string()),
                protocol: Some("udp".to_string()),
                destination_port: Some(65535),
                region: REGION.to_string(),
                tags: vec![Tag {
                    key: Some("Description".to_string()),
                    value: Some("d".repeat(200)),
                }],
                ..NetworkInsightsPathSpec::default()
            },
        ),
    ]
}

/// Write the seeded paths into the engine's store, the way `CreateNetworkInsightsPath` would
/// — through the store's own one path-insertion path, which is where every member a path
/// reports is derived — and answer the ids they were given, in store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    fixtures()
        .into_iter()
        .map(|(case, spec)| {
            guard
                .insert_network_insights_path(spec)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds room for the fixture"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// RULE 1: the store is what is deleted, and the id is what is reported
// ---------------------------------------------------------------------------

/// RULE 1: deleting a path removes exactly it and answers its id; every other path of the
/// account is left where it was, and the path is gone for every later caller — the start
/// that resolved it a moment ago no longer does. One row per seeded path.
#[tokio::test]
async fn deleting_a_path_removes_exactly_it_and_answers_its_id() {
    for (position, (case, _)) in fixtures().into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let path_id = seeded[position].clone();

        // The path resolves BEFORE the delete: the reader below is answering about this
        // call rather than about a path that was never stored.
        let (status, body) = analysis_start(&router, &path_id, "before-the-delete").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the seeded path must resolve before the delete, got {status} with \
             body {body}"
        );

        let (status, body) = delete_path(&router, &path_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = element(&body, "networkInsightsPathId");
        assert_eq!(
            reported.as_deref(),
            Some(path_id.as_str()),
            "case {case:?}: expected the response to name {path_id}, got {reported:?} in {body}"
        );

        assert_eq!(
            stored_paths(&engine),
            seeded.len() - 1,
            "case {case:?}: expected {} paths left in the store, got {}",
            seeded.len() - 1,
            stored_paths(&engine)
        );
        let held: Vec<String> = engine
            .state::<Ec2State>()
            .lock()
            .network_insights_paths
            .keys()
            .cloned()
            .collect();
        let remaining: Vec<String> = seeded
            .iter()
            .filter(|id| **id != path_id)
            .cloned()
            .collect();
        assert_eq!(
            held, remaining,
            "case {case:?}: expected the other paths {remaining:?} to be left alone, got {held:?}"
        );

        // The deleted path is gone for a later caller, and a path that was NOT deleted is
        // still reachable through that same reader.
        let (status, body) = analysis_start(&router, &path_id, "after-the-delete").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the start on the deleted path to be refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_NETWORK_INSIGHTS_PATH_ID_NOT_FOUND} from the \
             start on the deleted path, got {body}"
        );
        let (status, body) = analysis_start(&router, &remaining[0], "another-path").await;
        assert_eq!(
            status, 200,
            "case {case:?}: a path that was not deleted must still resolve, got {status} with \
             body {body}"
        );
    }
}

/// RULE 1: whatever ORDER the paths are deleted in, the store is left holding exactly the
/// ones that were not deleted, in store order. The rows are distinct orders over the seeded
/// positions, plus one that deletes every path.
#[tokio::test]
async fn deleting_in_any_order_leaves_exactly_the_paths_not_deleted() {
    let orders: [&[usize]; 4] = [&[0, 1, 2], &[4, 3, 2], &[3, 0, 4], &[0, 1, 2, 3, 4]];

    for order in orders {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let case = format!("order {order:?}");

        for position in order {
            let path_id = &seeded[*position];
            let (status, body) = delete_path(&router, path_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting {path_id} expected HTTP 200, got {status} with body \
                 {body}"
            );
            let reported = element(&body, "networkInsightsPathId");
            assert_eq!(
                reported.as_deref(),
                Some(path_id.as_str()),
                "case {case:?}: expected the response to name {path_id}, got {reported:?}"
            );
        }

        let expected: Vec<String> = seeded
            .iter()
            .enumerate()
            .filter(|(position, _)| !order.contains(position))
            .map(|(_, id)| id.clone())
            .collect();
        let held: Vec<String> = engine
            .state::<Ec2State>()
            .lock()
            .network_insights_paths
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected the paths not deleted {expected:?}, got {held:?}"
        );
    }
}

/// RULE 1: the bytes a deleted path held go back to the store's byte budget, so a create
/// followed by a delete leaves the store exactly where it started — whatever the path
/// carried. One row per seeded path, which is what makes the charge vary: the tags, the
/// addresses and the filters are the caller-chosen members a record retains.
#[tokio::test]
async fn the_bytes_a_deleted_path_held_go_back_to_the_store() {
    for (case, spec) in fixtures() {
        let (router, engine) = super::ec2_query::emulator();
        let before = stored_bytes(&engine);
        let path_id = engine
            .state::<Ec2State>()
            .lock()
            .insert_network_insights_path(spec)
            .unwrap_or_else(|| panic!("case {case:?}: the store holds room for the fixture"));
        let charged = stored_bytes(&engine);
        assert!(
            charged > before,
            "case {case:?}: storing a path must charge the byte budget, got {charged} after \
             {before}"
        );

        let (status, body) = delete_path(&router, &path_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = stored_bytes(&engine);
        assert_eq!(
            after, before,
            "case {case:?}: expected the delete to give back the {} bytes the path held, got \
             {after} against {before}",
            charged - before
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: an id that names no path of the account
// ---------------------------------------------------------------------------

/// RULE 2: an id this account holds no path under is refused — HTTP 400
/// `InvalidParameterValue`, stating that the `NetworkInsightsPathId` it was given is not a
/// valid one and naming the value the request sent — and nothing is removed. The rows are
/// each way a value can fail to name a path of the account: both published EC2 id suffix
/// lengths with digits nothing was created for, a suffix of neither length, an upper-case
/// suffix, a non-hexadecimal suffix, the id of the analysis resource of the same surface,
/// another resource type's id, free text, and a value carrying the characters that are
/// significant in the XML envelope the refusal is written into.
#[tokio::test]
async fn a_path_id_that_names_no_path_is_refused() {
    let prefix = NETWORK_INSIGHTS_PATH_ID_PREFIX;
    let cases = [
        ("the long form", format!("{prefix}-00000000000000000")),
        ("the short form", format!("{prefix}-00000000")),
        ("neither length", format!("{prefix}-0000000000000000")),
        ("an upper-case suffix", format!("{prefix}-0ABCDEF01")),
        ("a non-hex suffix", format!("{prefix}-zzzzzzzz")),
        (
            "the analysis resource's id",
            format!("{NETWORK_INSIGHTS_ANALYSIS_ID_PREFIX}-00000000000000000"),
        ),
        ("another resource's id", "vpc-00000000000000000".to_string()),
        ("free text", "not-a-network-insights-path".to_string()),
        ("XML metacharacters", "nip-<tag>&'\"end".to_string()),
    ];

    for (case, path_id) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);

        let (status, body) = delete_path(&router, &path_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        // Read the message out of the PARSED envelope rather than by substring: a value
        // whose metacharacters reached the markup verbatim would either stop the body from
        // parsing or cut the element's text short at that character, so this is what pins
        // that a caller-controlled id is written into the envelope escaped.
        let message = refusal_message(case, &body);
        assert_eq!(
            message,
            format!("NetworkInsightsPathId ({path_id}) is not valid"),
            "case {case:?}: expected this operation's one sentence, got {message:?}"
        );
        assert_eq!(
            message,
            error_message(&body),
            "case {case:?}: the parsed message and the one on the wire must agree, got \
             {message:?} and {:?}",
            error_message(&body)
        );
        assert_eq!(
            stored_paths(&engine),
            seeded.len(),
            "case {case:?}: a refused delete must remove nothing, got {} paths",
            stored_paths(&engine)
        );
    }
}

/// RULE 2: a delete is not idempotent — the path is gone after the first call, so a second
/// call naming the same id names no path of the account and gets the same refusal any other
/// unresolvable id gets, naming that id. One row per seeded path.
#[tokio::test]
async fn deleting_the_same_path_twice_refuses_the_second_call() {
    for (position, (case, _)) in fixtures().into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let path_id = seeded[position].clone();

        let (status, body) = delete_path(&router, &path_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete expected HTTP 200, got {status} with body {body}"
        );

        let (status, body) = delete_path(&router, &path_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the second delete expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: the second delete expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("NetworkInsightsPathId ({path_id}) is not valid"),
            "case {case:?}: the second delete must name {path_id}, got {body}"
        );
        assert_eq!(
            stored_paths(&engine),
            seeded.len() - 1,
            "case {case:?}: the refused second call must remove nothing, got {} paths",
            stored_paths(&engine)
        );
    }
}

/// RULE 2: a request that names no `NetworkInsightsPathId` at all names no path to delete,
/// so it is refused rather than answered with an empty success, and nothing is removed.
#[tokio::test]
async fn a_request_that_names_no_path_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = delete(&router, &[]).await;
    assert_eq!(
        status, 400,
        "no NetworkInsightsPathId: expected HTTP 400, got {status} with body {body}"
    );
    // The member is `@required`, so the BOUNDARY validator refuses the request before this
    // operation's handler runs — with the framework's own `MalformedInput`, not with a code
    // the handler chose.
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "no NetworkInsightsPathId: expected the boundary validator's MalformedInput, got {body}"
    );
    assert_eq!(
        stored_paths(&engine),
        seeded.len(),
        "no NetworkInsightsPathId: a refused delete must remove nothing, got {} paths",
        stored_paths(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: DryRun
// ---------------------------------------------------------------------------

/// RULE 3: `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and
/// removes nothing, so the path is still there and still deletable afterwards; `DryRun=false`
/// is an ordinary delete. One row per seeded path.
#[tokio::test]
async fn a_dry_run_reports_the_permitted_outcome_without_deleting() {
    for (position, (case, _)) in fixtures().into_iter().enumerate() {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let path_id = seeded[position].clone();

        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsPathId", &path_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        assert_eq!(
            stored_paths(&engine),
            seeded.len(),
            "case {case:?}: a dry run must delete nothing, got {} paths",
            stored_paths(&engine)
        );

        // `DryRun=false` is no dry run at all: the delete happens.
        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsPathId", &path_id),
                param("DryRun", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DryRun=false expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            stored_paths(&engine),
            seeded.len() - 1,
            "case {case:?}: DryRun=false must delete the path, got {} paths",
            stored_paths(&engine)
        );
    }
}

/// RULE 3: a dry run asks whether the request WOULD have worked, so one naming a path the
/// account does not hold is refused with THAT reason rather than reported as permitted. The
/// rows are the same forms of unresolvable id the refusal table covers.
#[tokio::test]
async fn a_dry_run_naming_no_path_of_the_account_is_refused_with_that_reason() {
    let prefix = NETWORK_INSIGHTS_PATH_ID_PREFIX;
    let cases = [
        ("the long form", format!("{prefix}-00000000000000000")),
        ("the short form", format!("{prefix}-00000000")),
        ("free text", "not-a-network-insights-path".to_string()),
    ];

    for (case, path_id) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);

        let (status, body) = delete(
            &router,
            &[
                param("NetworkInsightsPathId", &path_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} rather than a permitted dry \
             run, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("NetworkInsightsPathId ({path_id}) is not valid"),
            "case {case:?}: expected the refusal to name {path_id}, got {body}"
        );
        assert_eq!(
            stored_paths(&engine),
            seeded.len(),
            "case {case:?}: a refused dry run must remove nothing, got {} paths",
            stored_paths(&engine)
        );
    }
}
