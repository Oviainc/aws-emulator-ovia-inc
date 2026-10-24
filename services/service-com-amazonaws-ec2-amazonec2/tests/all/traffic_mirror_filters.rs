// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Traffic Mirror filter slice:
//! `CreateTrafficMirrorFilter` and `DescribeTrafficMirrorFilters`, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. `CreateTrafficMirrorFilter` MINTS A FILTER FROM THE REQUEST and
//!    `DescribeTrafficMirrorFilters` REPORTS THE STORE: every member the request sets
//!    round-trips through the describe, the id is the service's own, and a fresh filter
//!    carries no rule in either direction ("By default, no traffic is mirrored");
//! 2. `DescribeTrafficMirrorFilters` NARROWS: by `TrafficMirrorFilterIds`, and by every
//!    filter name it answers — values ORed within one entry, entries ANDed with each
//!    other — and a filter name it does not answer is refused;
//! 3. an id that names no Traffic Mirror filter of the account is refused with
//!    `InvalidTrafficMirrorFilterId.NotFound`, whatever the id looks like, while an account
//!    that simply holds no filter is reported as holding none;
//! 4. the report PAGES: `MaxResults` bounds a page, `NextToken` resumes it, and walking the
//!    pages visits every filter exactly once;
//! 5. one `ClientToken` names one filter, and `DryRun` answers `DryRunOperation` without
//!    storing or reporting anything.
//!
//! Each table row is a distinct input along what the rule generalises over — how long a
//! description is and which characters it holds, how many tags a filter carries, which
//! filter name selects, how many values one entry carries, which page size, which shape of
//! unknown id — so no row is the specification. The recorded unit this slice was filed from
//! is ONE row of the table in [`an_unknown_filter_id_is_refused_whatever_it_looks_like`].

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::state::{MAX_TRAFFIC_MIRROR_FILTERS_PAGE_SIZE, TRAFFIC_MIRROR_FILTER_TAGGABLE_RESOURCE};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. A Traffic Mirror filter is a Region-scoped
/// resource whose modelled shape names no Region, so the Region only selects the envelope.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the create table: `(case, the create parameters, the description it must
/// report, the tags it must report)`.
type CreateRow = (&'static str, Params, Option<&'static str>, Vec<(&'static str, &'static str)>);

/// One row of the filter table: `(case, the describe parameters, the ids it must select)`.
type FilterRow = (String, Params, Vec<String>);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateTrafficMirrorFilter", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeTrafficMirrorFilters", params).await
}

/// A `TrafficMirrorFilterId` is `tmf-` followed by 8 or 17 lowercase hex digits, the short
/// and the long EC2 id form.
fn is_traffic_mirror_filter_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("tmf-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Create one filter and answer the id it was minted with, failing the case when the create
/// did not succeed.
async fn created_id(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateTrafficMirrorFilter, got {status} \
         with body {body}"
    );
    let id = element(&body, "trafficMirrorFilterId").unwrap_or_default();
    assert!(
        is_traffic_mirror_filter_id(&id),
        "case {case:?}: expected a minted Traffic Mirror filter id, got {id:?}"
    );
    id
}

/// The ids `DescribeTrafficMirrorFilters` reports for `params`, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeTrafficMirrorFilters, got {status} \
         with body {body}"
    );
    elements(&body, "trafficMirrorFilterId")
}

/// The form parameters for one tag on the one resource type this create tags.
fn tag_params(index: usize, key: &str, value: &str) -> Params {
    vec![
        p(
            &format!("TagSpecification.{index}.ResourceType"),
            TRAFFIC_MIRROR_FILTER_TAGGABLE_RESOURCE,
        ),
        p(&format!("TagSpecification.{index}.Tag.1.Key"), key),
        p(&format!("TagSpecification.{index}.Tag.1.Value"), value),
    ]
}

/// RULE 1: every create in the table is stored as the request describes it and reads back
/// out of the store through the describe, with the id, and only the id, derived by the
/// service — and a fresh filter carries no rule in either direction and no network service.
#[tokio::test]
async fn every_created_filter_round_trips_through_the_describe() {
    // The rows differ along what the rule generalises over: whether a description is given
    // at all, how long it is, which characters it holds, and how many tags the filter
    // carries.
    let rows: Vec<CreateRow> = vec![
        ("no description and no tags", Vec::new(), None, Vec::new()),
        (
            "a short description",
            vec![p("Description", "web tier")],
            Some("web tier"),
            Vec::new(),
        ),
        (
            "a description holding punctuation and non-ASCII text",
            vec![p("Description", "mirror: ingress/egress — grün")],
            Some("mirror: ingress/egress — grün"),
            Vec::new(),
        ),
        (
            "a 255-character description",
            vec![p("Description", "d".repeat(255))],
            None,
            Vec::new(),
        ),
        (
            "one tag and a description",
            [vec![p("Description", "tagged")], tag_params(1, "Team", "platform")].concat(),
            Some("tagged"),
            vec![("Team", "platform")],
        ),
        (
            "two tag specifications of the one taggable resource type",
            [tag_params(1, "Team", "network"), tag_params(2, "Env", "staging")].concat(),
            None,
            vec![("Team", "network"), ("Env", "staging")],
        ),
    ];

    for (case, params, description, tags) in rows {
        let router = emulator();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let id = element(&body, "trafficMirrorFilterId").unwrap_or_default();
        assert!(
            is_traffic_mirror_filter_id(&id),
            "case {case:?}: expected a minted Traffic Mirror filter id, got {id:?}"
        );
        // A fresh filter mirrors nothing: no rule in either direction, and no network
        // service, so no rule id can be reported for it.
        assert!(
            elements(&body, "trafficMirrorFilterRuleId").is_empty(),
            "case {case:?}: a fresh filter carries no rules, got {body}"
        );

        // ROUND TRIP: the filter is in the store, not just echoed.
        let (status, described) =
            describe(&router, &indexed("TrafficMirrorFilterId", &[&id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the describe, got {status} / {described}"
        );
        assert_eq!(
            elements(&described, "trafficMirrorFilterId"),
            vec![id.clone()],
            "case {case:?}: expected the describe to report the created filter, got {described}"
        );
        for (source, reported) in [("the create", &body), ("the describe", &described)] {
            let described_description = element(reported, "description").map(|text| unescaped(&text));
            match description {
                Some(expected) => assert_eq!(
                    described_description.as_deref(),
                    Some(expected),
                    "case {case:?}: expected {source} to report description {expected:?}, \
                     got {described_description:?}"
                ),
                // The 255-character row's text is built rather than written out, and a row
                // that named no description reports none at all.
                None => {
                    let named = params
                        .iter()
                        .find(|(key, _)| key == "Description")
                        .map(|(_, value)| value.clone());
                    assert_eq!(
                        described_description, named,
                        "case {case:?}: expected {source} to report the description the \
                         request gave ({named:?}), got {described_description:?}"
                    );
                }
            }
            let keys = elements(reported, "key");
            let values = elements(reported, "value");
            assert_eq!(
                keys.len(),
                tags.len(),
                "case {case:?}: expected {source} to report {} tags, got {keys:?}",
                tags.len()
            );
            for (key, value) in &tags {
                let at = keys.iter().position(|reported_key| reported_key == key);
                assert!(
                    at.is_some(),
                    "case {case:?}: expected {source} to report the tag key {key:?}, got {keys:?}"
                );
                assert_eq!(
                    values.get(at.unwrap_or_default()).map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected {source} to report {key:?}={value:?}, got {values:?}"
                );
            }
        }
    }
}

/// RULE 2: the describe narrows by ids and by every filter name it answers, and refuses a
/// name it does not answer.
#[tokio::test]
async fn the_describe_narrows_by_ids_and_by_every_filter_it_answers() {
    let router = emulator();
    let web = created_id(
        &router,
        "web",
        &[vec![p("Description", "web tier")], tag_params(1, "Team", "platform")].concat(),
    )
    .await;
    let data = created_id(
        &router,
        "data",
        &[vec![p("Description", "data tier")], tag_params(1, "Team", "data")].concat(),
    )
    .await;
    let bare = created_id(&router, "bare", &[]).await;
    let all = vec![web.clone(), data.clone(), bare.clone()];

    // The rows differ along which filter name selects, how many values one entry carries,
    // whether a wildcard is used, and how many entries are ANDed together.
    let rows: Vec<FilterRow> = vec![
        (
            "no narrowing at all".to_string(),
            Vec::new(),
            all.clone(),
        ),
        (
            "one id".to_string(),
            indexed("TrafficMirrorFilterId", &[&data]),
            vec![data.clone()],
        ),
        (
            "two ids".to_string(),
            indexed("TrafficMirrorFilterId", &[&bare, &web]),
            vec![web.clone(), bare.clone()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
        (
            "the traffic-mirror-filter-id filter".to_string(),
            filters(&[("traffic-mirror-filter-id", &[web.as_str()])]),
            vec![web.clone()],
        ),
        (
            "the description filter, matched exactly".to_string(),
            filters(&[("description", &["data tier"])]),
            vec![data.clone()],
        ),
        (
            "the description filter, two values ORed".to_string(),
            filters(&[("description", &["data tier", "web tier"])]),
            vec![web.clone(), data.clone()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
        (
            "the description filter with EC2's * wildcard".to_string(),
            filters(&[("description", &["*tier"])]),
            vec![web.clone(), data.clone()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
        (
            "the description filter on a value nothing holds".to_string(),
            filters(&[("description", &["mirror everything"])]),
            Vec::new(),
        ),
        (
            "the tag-key filter".to_string(),
            filters(&[("tag-key", &["Team"])]),
            vec![web.clone(), data.clone()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
        (
            "the tag:<key> filter".to_string(),
            filters(&[("tag:Team", &["data"])]),
            vec![data.clone()],
        ),
        (
            "two filters, ANDed".to_string(),
            filters(&[("description", &["*tier"]), ("tag:Team", &["platform"])]),
            vec![web.clone()],
        ),
        (
            "two filters, ANDed, that no one filter satisfies together".to_string(),
            filters(&[("description", &["data tier"]), ("tag:Team", &["platform"])]),
            Vec::new(),
        ),
        (
            "a filter carrying no value at all".to_string(),
            filters(&[("description", &[])]),
            Vec::new(),
        ),
        (
            "an id and a filter that disagree".to_string(),
            [
                indexed("TrafficMirrorFilterId", &[&bare]),
                filters(&[("description", &["web tier"])]),
            ]
            .concat(),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        let mut reported = described_ids(&router, &case, &params).await;
        reported.sort();
        let mut want = expected;
        want.sort();
        assert_eq!(
            reported, want,
            "case {case:?}: expected the describe to select {want:?}, got {reported:?}"
        );
    }

    // A filter name this operation does not answer is refused, whether or not any filter
    // would have matched it — including a `tag:` prefix with no key after it.
    for name in ["owner-id", "Description", "traffic-mirror-filter-ids", "tag:"] {
        let (status, body) = describe(&router, &filters(&[(name, &["web tier"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an unanswered filter name, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// RULE 3: an id that names no Traffic Mirror filter of the account is refused, whatever
/// shape it has — while an account that holds no filter at all is reported as holding none.
#[tokio::test]
async fn an_unknown_filter_id_is_refused_whatever_it_looks_like() {
    let router = emulator();

    // An empty account: the report is empty, not an error.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"an account with no filters\": expected HTTP 200, got {status} / {body}"
    );
    assert!(
        elements(&body, "trafficMirrorFilterId").is_empty(),
        "case \"an account with no filters\": expected an empty report, got {body}"
    );

    let known = created_id(&router, "the one stored filter", &[]).await;

    // The rows differ along the SHAPE of the unknown id: the long 17-hex form, the short
    // 8-hex form, the 16-hex form the recorded unit this issue was filed from used, a
    // non-hex suffix and a foreign prefix.
    let unknown = [
        "tmf-0123456789abcdef0",
        "tmf-00000000",
        "tmf-ffffffffffffffff",
        "tmf-not-hex",
        "vpc-0123456789abcdef0",
    ];
    for id in unknown {
        for (case, params) in [
            (format!("{id} alone"), indexed("TrafficMirrorFilterId", &[id])),
            (
                format!("{id} beside a known id"),
                indexed("TrafficMirrorFilterId", &[known.as_str(), id]),
            ),
        ] {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an id naming no filter, got {status} / {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidTrafficMirrorFilterId.NotFound",
                "case {case:?}: expected InvalidTrafficMirrorFilterId.NotFound, got {body}"
            );
            assert!(
                element(&body, "Message")
                    .map(|message| unescaped(&message))
                    .is_some_and(|message| message.contains(id)),
                "case {case:?}: expected the message to name the id the request gave, got {body}"
            );
        }
    }
}

/// RULE 4: the report pages — a page holds at most `MaxResults` filters, `NextToken`
/// resumes where the page ended, and walking the pages visits every filter exactly once.
#[tokio::test]
async fn the_report_pages_over_every_filter_exactly_once() {
    let router = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        created.push(
            created_id(
                &router,
                &format!("filter {index}"),
                &[p("Description", format!("tier {index}"))],
            )
            .await,
        );
    }
    created.sort();

    // The modelled `MaxResults` range is 5..1000, so these are the smallest page the
    // operation can be asked for, one above it, and a page larger than the whole store.
    for page_size in [5usize, 6, 7, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "trafficMirrorFilterId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults filters, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: paging must walk every filter exactly once, got {walked:?}"
        );
    }

    // No page size named: one page, up to the largest the operation can be asked for.
    assert!(
        created.len() <= MAX_TRAFFIC_MIRROR_FILTERS_PAGE_SIZE,
        "this suite seeds fewer filters than one page holds"
    );
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} with body {body}");
    assert!(
        element(&body, "nextToken").is_none(),
        "case \"no MaxResults\": a page that holds everything carries no token, got {body}"
    );

    // A token this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "00", ""] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 5: one `ClientToken` names one filter, `DryRun` stores nothing, and a tag
/// specification naming another resource type is refused.
#[tokio::test]
async fn idempotency_dry_run_and_tagging_are_refused_or_replayed() {
    let router = emulator();

    // The rows differ along the token's own text, and each row is replayed, so idempotency
    // is asserted for a token of each shape rather than for one recorded value.
    for token in ["a", "idem-7", "0123456789abcdef0123456789abcdef"] {
        let params = vec![p("ClientToken", token), p("Description", format!("for {token}"))];
        let first = created_id(&router, token, &params).await;
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected a replay to succeed, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "trafficMirrorFilterId").as_deref(),
            Some(first.as_str()),
            "case {token:?}: one ClientToken names one filter, got {body}"
        );
        assert_eq!(
            element(&body, "clientToken").as_deref(),
            Some(token),
            "case {token:?}: the response echoes the token the request carried, got {body}"
        );
    }
    let after_replays = described_ids(&router, "after the replays", &[]).await;
    assert_eq!(
        after_replays.len(),
        3,
        "a replayed create stores no second filter, got {after_replays:?}"
    );

    // DryRun: refused with DryRunOperation ("If you have the required permissions, the
    // error response is DryRunOperation" — `DryRun`), which EC2 answers with HTTP 412, and
    // nothing stored or reported.
    for (case, params) in [
        ("a dry-run create", vec![p("DryRun", "true")]),
        (
            "a dry-run create with tags",
            [vec![p("DryRun", "true")], tag_params(1, "Team", "platform")].concat(),
        ),
    ] {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
    }
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"a dry-run describe\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"a dry-run describe\": expected DryRunOperation, got {body}"
    );
    assert_eq!(
        described_ids(&router, "after the dry runs", &[]).await,
        after_replays,
        "a dry run stores nothing"
    );

    // A tag specification may name only the resource this call creates.
    for resource_type in ["vpc", "traffic-mirror-session", "instance"] {
        let (status, body) = create(
            &router,
            &[
                p("TagSpecification.1.ResourceType", resource_type),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400 for a foreign resource type, \
             got {status} / {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {resource_type:?}: expected InvalidParameterValue, got {body}"
        );
    }
    assert_eq!(
        described_ids(&router, "after the refused tag specifications", &[]).await,
        after_replays,
        "a refused create stores nothing"
    );
}
