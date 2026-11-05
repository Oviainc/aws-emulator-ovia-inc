// SPDX-License-Identifier: CC0-1.0

//! Submitting a Multi-Region Access Point's routing configuration, driven end
//! to end over HTTP.
//!
//! Three rules are under test here.
//!
//! * A submitted route update names the routes it changes by the members it
//!   carries — a bucket, a Region, or both — and sets their traffic dial; every
//!   route no update names keeps the dial it had.
//! * At least one Region must be active at all times, so a submit that would
//!   leave every associated Region passive is refused, and a refused submit
//!   changes nothing at all.
//! * A routing submit resolves its `Mrap` the way the routing READ does — as
//!   the service-assigned alias, scoped to the account that owns it — so a
//!   value that resolves to no access point is a request this service will not
//!   carry out.
//!
//! Every row below differs along something those rules generalise over: how
//! many buckets the access point associates, which member identifies the route
//! (bucket, Region, both, neither), how many updates one submit carries, the
//! dial submitted, and the order the updates arrive in.
//!
//! The router, the request builder, the create and the routing read are the
//! sibling suites', reused rather than copied.

use crate::access_point_lifecycle::{
    call_in_region, xml_escape, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::multi_region_access_point_lifecycle::{create, settled_wait, CreateCase};
use crate::multi_region_access_point_sub_resources::{
    alias_of, get_routes, leaf, route_entries,
};
use crate::storage_lens_configuration::leaves_of;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    MAX_ROUTE_UPDATES, TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_PASSIVE,
};

/// The code and message a submit this service will not carry out is refused
/// with.
const INVALID_REQUEST_CODE: &str = "InvalidRequest";
const INVALID_REQUEST_MESSAGE: &str = "Request invalid";

/// A Region no create below associates a bucket in, so an update naming it
/// names no route of the access point under test.
const UNASSOCIATED_REGION: &str = "eu-central-1";

/// A traffic dial between the two states the model documents, admitted by
/// `MultiRegionAccessPointRoute$TrafficDialPercentage` (range min 0, max 100)
/// and not equal to [`TRAFFIC_DIAL_PASSIVE`], so the Region it is submitted for
/// still takes traffic.
const TRAFFIC_DIAL_BETWEEN: i32 = 50;

/// One submitted `MultiRegionAccessPointRoute`: the two members that identify
/// the routes it changes, and the dial it submits for them.
#[derive(Clone, Copy, Debug)]
struct Update {
    bucket: Option<&'static str>,
    region: Option<&'static str>,
    dial: i32,
}

impl Update {
    /// An update identifying its routes by bucket alone.
    const fn bucket(bucket: &'static str, dial: i32) -> Self {
        Self {
            bucket: Some(bucket),
            region: None,
            dial,
        }
    }

    /// An update identifying its routes by Region alone.
    const fn region(region: &'static str, dial: i32) -> Self {
        Self {
            bucket: None,
            region: Some(region),
            dial,
        }
    }

    /// An update identifying one route by both members.
    const fn both(bucket: &'static str, region: &'static str, dial: i32) -> Self {
        Self {
            bucket: Some(bucket),
            region: Some(region),
            dial,
        }
    }

    /// An update carrying neither member, which identifies nothing.
    const fn neither(dial: i32) -> Self {
        Self {
            bucket: None,
            region: None,
            dial,
        }
    }
}

/// `updates` as the request body of a `SubmitMultiRegionAccessPointRoutes`
/// call: a `RouteList` of `<Route>` entries, the member name the model gives
/// the list's member.
fn submit_body(updates: &[Update]) -> String {
    let mut xml = format!("<SubmitMultiRegionAccessPointRoutesRequest xmlns=\"{XMLNS}\">");
    xml.push_str("<RouteUpdates>");
    for update in updates {
        xml.push_str("<Route>");
        if let Some(bucket) = update.bucket {
            xml.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(bucket)));
        }
        if let Some(region) = update.region {
            xml.push_str(&format!("<Region>{}</Region>", xml_escape(region)));
        }
        xml.push_str(&format!(
            "<TrafficDialPercentage>{}</TrafficDialPercentage>",
            update.dial
        ));
        xml.push_str("</Route>");
    }
    xml.push_str("</RouteUpdates></SubmitMultiRegionAccessPointRoutesRequest>");
    xml
}

async fn submit(
    emulator: &Emulator,
    region: &str,
    account: &str,
    mrap: &str,
    updates: &[Update],
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "PATCH",
        &format!("/v20180820/mrap/instances/{mrap}/routes"),
        account,
        &submit_body(updates),
    )
    .await
}

/// The routing configuration `mrap` reports, as (bucket, Region, traffic dial)
/// in document order — read back through `GetMultiRegionAccessPointRoutes`, so
/// every assertion below is about what a LATER caller sees.
async fn routes_of(
    case: &str,
    emulator: &Emulator,
    region: &str,
    account: &str,
    mrap: &str,
) -> Vec<(String, String, String)> {
    let reply = get_routes(emulator, region, account, mrap).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected the routing read to be served, got {} with body {}",
        reply.status, reply.body
    );
    let leaves = leaves_of(&reply.body);
    assert_eq!(
        leaf(&leaves, "Mrap"),
        Some(mrap.to_string()),
        "case {case:?}: expected the read to report the access point it was asked about, \
         got body {}",
        reply.body
    );
    route_entries(&leaves)
}

/// The entries a routing read must report for `buckets`: each bucket in the
/// Region its create was signed for, carrying the dial it is expected to have.
fn expected_routes(buckets: &[&str], dials: &[i32]) -> Vec<(String, String, String)> {
    buckets
        .iter()
        .zip(dials)
        .map(|(bucket, dial)| {
            (
                (*bucket).to_string(),
                REGION.to_string(),
                dial.to_string(),
            )
        })
        .collect()
}

/// Assert that `reply` is the refusal this service answers a submit it will not
/// carry out with.
fn assert_refused(case: &str, reply: &Reply) {
    assert_eq!(
        (
            reply.status,
            reply.code(),
            reply.text("Message").unwrap_or_default()
        ),
        (
            400,
            INVALID_REQUEST_CODE.to_string(),
            INVALID_REQUEST_MESSAGE.to_string()
        ),
        "case {case:?}: expected the submit to be refused as invalid, got body {}",
        reply.body
    );
}

/// Assert that `reply` is an accepted submit: the modelled success status, and
/// no refusal envelope.
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected the submit to be accepted, got {} with body {}",
        reply.status, reply.body
    );
    assert!(
        !reply.body.contains("<Error"),
        "case {case:?}: an accepted submit carries no error envelope, got body {}",
        reply.body
    );
}

/// One accepted submit: the access point it is made on, the updates it carries,
/// and the dial each associated bucket is left with.
struct AcceptedCase {
    case: &'static str,
    name: &'static str,
    client_token: &'static str,
    buckets: &'static [&'static str],
    updates: Vec<Update>,
    /// One dial per associated bucket, in the order the create listed them.
    expected: &'static [i32],
}

/// The accepted submits every assertion below is run over. No row is merely a
/// rename: each differs in how many buckets the access point associates, which
/// member identifies the route, how many updates the submit carries, the dial
/// submitted, or the order the updates arrive in.
fn accepted_cases() -> Vec<AcceptedCase> {
    vec![
        AcceptedCase {
            case: "one bucket made passive, named by bucket alone",
            name: "submit-bucket-mrap",
            client_token: "t-submit-bucket",
            buckets: &["reports", "finance", "partner"],
            updates: vec![Update::bucket("finance", TRAFFIC_DIAL_PASSIVE)],
            expected: &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_PASSIVE, TRAFFIC_DIAL_ACTIVE],
        },
        AcceptedCase {
            case: "one bucket made passive, named by bucket and its Region",
            name: "submit-both-mrap",
            client_token: "t-submit-both",
            buckets: &["reports", "finance", "partner"],
            updates: vec![Update::both("finance", REGION, TRAFFIC_DIAL_PASSIVE)],
            expected: &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_PASSIVE, TRAFFIC_DIAL_ACTIVE],
        },
        AcceptedCase {
            case: "a Region names every bucket in it, then one is re-activated",
            name: "submit-region-mrap",
            client_token: "t-submit-region",
            buckets: &["reports", "finance", "partner"],
            updates: vec![
                Update::region(REGION, TRAFFIC_DIAL_PASSIVE),
                Update::bucket("reports", TRAFFIC_DIAL_ACTIVE),
            ],
            expected: &[
                TRAFFIC_DIAL_ACTIVE,
                TRAFFIC_DIAL_PASSIVE,
                TRAFFIC_DIAL_PASSIVE,
            ],
        },
        AcceptedCase {
            case: "two buckets made passive in one submit",
            name: "submit-two-mrap",
            client_token: "t-submit-two",
            buckets: &["reports", "finance", "partner"],
            updates: vec![
                Update::bucket("reports", TRAFFIC_DIAL_PASSIVE),
                Update::bucket("partner", TRAFFIC_DIAL_PASSIVE),
            ],
            expected: &[
                TRAFFIC_DIAL_PASSIVE,
                TRAFFIC_DIAL_ACTIVE,
                TRAFFIC_DIAL_PASSIVE,
            ],
        },
        AcceptedCase {
            case: "a dial between the two documented states",
            name: "submit-half-mrap",
            client_token: "t-submit-half",
            buckets: &["reports", "finance", "partner"],
            updates: vec![Update::bucket("finance", TRAFFIC_DIAL_BETWEEN)],
            expected: &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_BETWEEN, TRAFFIC_DIAL_ACTIVE],
        },
        AcceptedCase {
            case: "a later update of the same submit overrides an earlier one",
            name: "submit-override-mrap",
            client_token: "t-submit-override",
            buckets: &["reports", "finance", "partner"],
            updates: vec![
                Update::bucket("finance", TRAFFIC_DIAL_PASSIVE),
                Update::both("finance", REGION, TRAFFIC_DIAL_ACTIVE),
            ],
            expected: &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_ACTIVE],
        },
        AcceptedCase {
            case: "the only associated bucket, named by its Region, left active",
            name: "submit-solo-mrap",
            client_token: "t-submit-solo",
            buckets: &["reports"],
            updates: vec![Update::region(REGION, TRAFFIC_DIAL_ACTIVE)],
            expected: &[TRAFFIC_DIAL_ACTIVE],
        },
        AcceptedCase {
            case: "the only associated bucket, dialled down but still active",
            name: "submit-solo-half-mrap",
            client_token: "t-submit-solo-half",
            buckets: &["reports"],
            updates: vec![Update::bucket("reports", TRAFFIC_DIAL_BETWEEN)],
            expected: &[TRAFFIC_DIAL_BETWEEN],
        },
    ]
}

#[tokio::test]
async fn a_submit_sets_the_routes_it_names_and_leaves_every_other_one_alone() {
    let emulator = Emulator::start();

    for row in accepted_cases() {
        let create_case = CreateCase::new(row.case, row.name, row.client_token, row.buckets);
        let created = create(&emulator, &create_case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the create to be accepted, got {} with body {}",
            row.case, created.status, created.body
        );
        let alias = alias_of(&emulator, &create_case).await;

        // Every associated bucket starts out active, so the submit below is the
        // only thing that can have changed a dial.
        let before = routes_of(row.case, &emulator, REGION, ACCOUNT, &alias).await;
        let all_active: Vec<i32> = row.buckets.iter().map(|_| TRAFFIC_DIAL_ACTIVE).collect();
        assert_eq!(
            before,
            expected_routes(row.buckets, &all_active),
            "case {:?}: expected one active route per associated bucket before the submit, \
             got {before:?}",
            row.case
        );

        let submitted = submit(&emulator, REGION, ACCOUNT, &alias, &row.updates).await;
        assert_accepted(row.case, &submitted);

        let after = routes_of(row.case, &emulator, REGION, ACCOUNT, &alias).await;
        assert_eq!(
            after,
            expected_routes(row.buckets, row.expected),
            "case {:?}: expected the submitted dials on the routes the updates named and the \
             old dial on every other, got {after:?}",
            row.case
        );
    }
}

/// One submit this service will not carry out, and why.
struct RefusedCase {
    case: &'static str,
    name: &'static str,
    client_token: &'static str,
    updates: Vec<Update>,
}

/// The two buckets every refused submit below is run against.
const REFUSED_BUCKETS: &[&str] = &["reports", "finance"];

/// The refused submits. Each differs in WHY it names no routing configuration
/// this service can store: the update identifies nothing, it identifies a
/// bucket or Region the access point does not associate, it pairs a bucket with
/// a Region it is not in, or it would leave every Region passive.
fn refused_cases() -> Vec<RefusedCase> {
    vec![
        RefusedCase {
            case: "an update naming neither a bucket nor a Region",
            name: "refuse-neither-mrap",
            client_token: "t-refuse-neither",
            updates: vec![Update::neither(TRAFFIC_DIAL_PASSIVE)],
        },
        RefusedCase {
            case: "an update naming a bucket the access point does not associate",
            name: "refuse-bucket-mrap",
            client_token: "t-refuse-bucket",
            updates: vec![Update::bucket("never-associated", TRAFFIC_DIAL_PASSIVE)],
        },
        RefusedCase {
            case: "an update naming a bucket with a Region it is not in",
            name: "refuse-pair-mrap",
            client_token: "t-refuse-pair",
            updates: vec![Update::both(
                "reports",
                UNASSOCIATED_REGION,
                TRAFFIC_DIAL_PASSIVE,
            )],
        },
        RefusedCase {
            case: "an update naming a Region the access point associates nothing in",
            name: "refuse-region-mrap",
            client_token: "t-refuse-region",
            updates: vec![Update::region(UNASSOCIATED_REGION, TRAFFIC_DIAL_PASSIVE)],
        },
        RefusedCase {
            case: "every associated bucket made passive, one update each",
            name: "refuse-all-mrap",
            client_token: "t-refuse-all",
            updates: vec![
                Update::bucket("reports", TRAFFIC_DIAL_PASSIVE),
                Update::bucket("finance", TRAFFIC_DIAL_PASSIVE),
            ],
        },
        RefusedCase {
            case: "every associated bucket made passive through its Region",
            name: "refuse-all-region-mrap",
            client_token: "t-refuse-all-region",
            updates: vec![Update::region(REGION, TRAFFIC_DIAL_PASSIVE)],
        },
        RefusedCase {
            case: "one update this service can carry out beside one it cannot",
            name: "refuse-partial-mrap",
            client_token: "t-refuse-partial",
            updates: vec![
                Update::bucket("reports", TRAFFIC_DIAL_PASSIVE),
                Update::neither(TRAFFIC_DIAL_ACTIVE),
            ],
        },
    ]
}

#[tokio::test]
async fn a_submit_this_service_refuses_changes_no_route_at_all() {
    let emulator = Emulator::start();
    let unchanged: Vec<i32> = REFUSED_BUCKETS
        .iter()
        .map(|_| TRAFFIC_DIAL_ACTIVE)
        .collect();

    for row in refused_cases() {
        let create_case = CreateCase::new(row.case, row.name, row.client_token, REFUSED_BUCKETS);
        let created = create(&emulator, &create_case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the create to be accepted, got {} with body {}",
            row.case, created.status, created.body
        );
        let alias = alias_of(&emulator, &create_case).await;

        let submitted = submit(&emulator, REGION, ACCOUNT, &alias, &row.updates).await;
        assert_refused(row.case, &submitted);

        let after = routes_of(row.case, &emulator, REGION, ACCOUNT, &alias).await;
        assert_eq!(
            after,
            expected_routes(REFUSED_BUCKETS, &unchanged),
            "case {:?}: a refused submit must leave every route as it was, got {after:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_last_active_region_cannot_be_made_passive_but_may_be_traded_for_another() {
    let emulator = Emulator::start();
    let case = CreateCase::new(
        "the access point the submits are made on",
        "last-active-mrap",
        "t-last-active",
        &["reports", "finance"],
    );
    let created = create(&emulator, &case).await;
    assert_eq!(
        created.status, 200,
        "expected the create to be accepted, got {} with body {}",
        created.status, created.body
    );
    // Read after the create's span has run out, so the submits below are made
    // on a settled access point as well as on a creating one.
    ::tokio::time::sleep(settled_wait()).await;
    let alias = alias_of(&emulator, &case).await;

    let first = submit(
        &emulator,
        REGION,
        ACCOUNT,
        &alias,
        &[Update::bucket("finance", TRAFFIC_DIAL_PASSIVE)],
    )
    .await;
    assert_accepted("the first bucket made passive", &first);
    let after_first = routes_of("the first bucket made passive", &emulator, REGION, ACCOUNT, &alias)
        .await;
    assert_eq!(
        after_first,
        expected_routes(&["reports", "finance"], &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_PASSIVE]),
        "expected only the named bucket to go passive, got {after_first:?}"
    );

    // The one active Region left cannot be made passive on its own.
    let refused = submit(
        &emulator,
        REGION,
        ACCOUNT,
        &alias,
        &[Update::bucket("reports", TRAFFIC_DIAL_PASSIVE)],
    )
    .await;
    assert_refused("the last active bucket made passive", &refused);
    let after_refusal = routes_of(
        "the last active bucket made passive",
        &emulator,
        REGION,
        ACCOUNT,
        &alias,
    )
    .await;
    assert_eq!(
        after_refusal, after_first,
        "expected the refused submit to change nothing, got {after_refusal:?}"
    );

    // It can be made passive in the SAME submit that activates another.
    let traded = submit(
        &emulator,
        REGION,
        ACCOUNT,
        &alias,
        &[
            Update::bucket("reports", TRAFFIC_DIAL_PASSIVE),
            Update::bucket("finance", TRAFFIC_DIAL_ACTIVE),
        ],
    )
    .await;
    assert_accepted("the active Region traded for the passive one", &traded);
    let after_trade = routes_of(
        "the active Region traded for the passive one",
        &emulator,
        REGION,
        ACCOUNT,
        &alias,
    )
    .await;
    assert_eq!(
        after_trade,
        expected_routes(&["reports", "finance"], &[TRAFFIC_DIAL_PASSIVE, TRAFFIC_DIAL_ACTIVE]),
        "expected the submitted dials to be the configuration a later read reports, \
         got {after_trade:?}"
    );
}

#[tokio::test]
async fn a_submit_is_bounded_by_how_many_updates_this_emulator_accepts() {
    let emulator = Emulator::start();
    let buckets = ["reports", "finance"];
    let case = CreateCase::new(
        "the access point the bounded submits are made on",
        "bounded-mrap",
        "t-bounded",
        &["reports", "finance"],
    );
    let created = create(&emulator, &case).await;
    assert_eq!(
        created.status, 200,
        "expected the create to be accepted, got {} with body {}",
        created.status, created.body
    );
    let alias = alias_of(&emulator, &case).await;

    // ON the bound: carried out. Every update names the same associated bucket,
    // so the configuration it leaves is the dial the last of them submitted.
    let at_bound: Vec<Update> = (0..MAX_ROUTE_UPDATES)
        .map(|_| Update::bucket("finance", TRAFFIC_DIAL_PASSIVE))
        .collect();
    assert_eq!(
        at_bound.len(),
        MAX_ROUTE_UPDATES,
        "the on-the-bound row must sit on the bound, not near it"
    );
    let accepted = submit(&emulator, REGION, ACCOUNT, &alias, &at_bound).await;
    assert_accepted("as many updates as this emulator accepts", &accepted);
    let after = routes_of(
        "as many updates as this emulator accepts",
        &emulator,
        REGION,
        ACCOUNT,
        &alias,
    )
    .await;
    assert_eq!(
        after,
        expected_routes(&buckets, &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_PASSIVE]),
        "expected the submitted dial on the named bucket, got {after:?}"
    );

    // ONE PAST it: refused, naming the bound, and nothing is changed.
    let past_bound: Vec<Update> = (0..=MAX_ROUTE_UPDATES)
        .map(|_| Update::bucket("reports", TRAFFIC_DIAL_PASSIVE))
        .collect();
    let refused = submit(&emulator, REGION, ACCOUNT, &alias, &past_bound).await;
    assert_eq!(
        (refused.status, refused.code()),
        (400, INVALID_REQUEST_CODE.to_string()),
        "expected one update past the bound to be refused, got body {}",
        refused.body
    );
    let message = refused.text("Message").unwrap_or_default();
    assert!(
        message.contains(&MAX_ROUTE_UPDATES.to_string()),
        "expected the refusal to state the bound, got message {message:?}"
    );
    let unchanged = routes_of("one update past the bound", &emulator, REGION, ACCOUNT, &alias).await;
    assert_eq!(
        unchanged, after,
        "expected the refused submit to change nothing, got {unchanged:?}"
    );
}

#[tokio::test]
async fn a_submit_whose_mrap_resolves_to_nothing_is_refused_and_writes_nothing() {
    let emulator = Emulator::start();
    let case = CreateCase::new(
        "the access point the submits do not reach",
        "unresolved-mrap",
        "t-unresolved",
        &["reports", "finance"],
    );
    let created = create(&emulator, &case).await;
    assert_eq!(
        created.status, 200,
        "expected the create to be accepted, got {} with body {}",
        created.status, created.body
    );
    let alias = alias_of(&emulator, &case).await;
    let untouched = expected_routes(
        &["reports", "finance"],
        &[TRAFFIC_DIAL_ACTIVE, TRAFFIC_DIAL_ACTIVE],
    );

    // Each row names a value the submit cannot resolve to an access point this
    // account holds: the NAME rather than the alias, an alias nothing was
    // created under, a value of the shortest shape the label admits, and the
    // real alias under another account.
    let rows: Vec<(&str, String, &str)> = vec![
        ("the access point's name rather than its alias", case.name.to_string(), ACCOUNT),
        ("an alias nothing was created under", "nevercreated.mrap".to_string(), ACCOUNT),
        ("the shortest value the label admits", "abc".to_string(), ACCOUNT),
        ("another account's submit on this alias", alias.clone(), OTHER_ACCOUNT),
    ];

    for (row, mrap, account) in rows {
        let submitted = submit(
            &emulator,
            REGION,
            account,
            &mrap,
            &[Update::bucket("reports", TRAFFIC_DIAL_PASSIVE)],
        )
        .await;
        assert_refused(row, &submitted);

        let after = routes_of(row, &emulator, REGION, ACCOUNT, &alias).await;
        assert_eq!(
            after, untouched,
            "case {row:?}: a submit that resolves no access point must write nothing, \
             got {after:?}"
        );
    }
}
