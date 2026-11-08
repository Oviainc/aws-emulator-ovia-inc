// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionsByRealtimeLogConfig`, end to end over HTTP.
//!
//! The rule under test: a distribution is listed under a real-time log configuration
//! exactly when one of its STORED cache behaviours — the default one or any ordered one —
//! sends its logs through that configuration's ARN (`RealtimeLogConfigArn`), each matching
//! distribution listed ONCE and in id order; the configuration is addressed by its `Name`
//! or its `ARN`, the name identifying it whenever the request carries one, and a request
//! that names neither identifier, or names one no stored configuration answers to, is
//! refused with `InvalidArgument` (HTTP 400) — the one error this operation models. The
//! page contract is the one every CloudFront distribution listing states (`MaxItems` is the
//! page size and is echoed, `Marker` resumes after the id the previous page ended at,
//! `NextMarker` and `IsTruncated` arrive exactly when matching distributions remain,
//! `Quantity` states the length of `Items`), and a configuration nothing attaches lists
//! nothing rather than falling back to the whole store.
//!
//! The rows below span what that rule generalises over: WHICH behaviour attaches the
//! configuration (the default one, one ordered one, a later ordered one among several, both
//! at once), how many behaviours a distribution carries, how many distributions share one
//! configuration (so a page can cut inside a matching set), a distribution attaching NO
//! configuration at all, and a configuration no distribution attaches.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] builds, and the configuration create is
//! [`crate::realtime_log_config_lifecycle`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, REALTIME_LOG_CONFIG_LISTING_IDENTIFIER_REQUIRED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, get_distribution, is_distribution_id, list_distributions,
    Answer, Node, API_VERSION,
};
use crate::realtime_log_config_lifecycle::{
    config_node, create_accepted, strings, Endpoint, Submitted,
};
use crate::response_headers_policy_lifecycle::escape;

/// One distribution under test: which of the created configurations its default cache
/// behaviour sends its logs through, and which one each of its ordered cache behaviours
/// does. An index is into the configurations [`create_configs`] creates, in order; `None`
/// is a behaviour that attaches no real-time log configuration at all.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    default_config: Option<usize>,
    ordered: &'static [Option<usize>],
}

impl Row {
    /// Whether this distribution attaches the configuration at `index` to any of its cache
    /// behaviours — the rule the listing implements, computed here from the row rather than
    /// from the handler.
    fn attaches(&self, index: usize) -> bool {
        self.default_config == Some(index) || self.ordered.contains(&Some(index))
    }
}

/// How many real-time log configurations the suite creates. The last one is attached by
/// nothing, so an empty listing is covered, and index 2 is only ever attached by an ordered
/// behaviour alongside the default one.
const CONFIG_COUNT: usize = 4;

/// The distributions created for every case below.
const ROWS: &[Row] = &[
    Row {
        case: "the default behaviour attaches configuration 0, no ordered behaviour at all",
        default_config: Some(0),
        ordered: &[],
    },
    Row {
        case: "a second distribution whose default behaviour attaches configuration 0",
        default_config: Some(0),
        ordered: &[],
    },
    Row {
        case: "a third distribution whose default behaviour attaches configuration 0",
        default_config: Some(0),
        ordered: &[],
    },
    Row {
        case: "the default behaviour attaches none and the one ordered behaviour attaches 1",
        default_config: None,
        ordered: &[Some(1)],
    },
    Row {
        case: "three ordered behaviours of which the last attaches configuration 1",
        default_config: None,
        ordered: &[None, None, Some(1)],
    },
    Row {
        case: "the default behaviour and an ordered behaviour both attach configuration 2",
        default_config: Some(2),
        ordered: &[Some(2)],
    },
    Row {
        case: "the default behaviour attaches 1 and an ordered behaviour attaches 0",
        default_config: Some(1),
        ordered: &[Some(0), None],
    },
    Row {
        case: "no behaviour attaches a real-time log configuration at all",
        default_config: None,
        ordered: &[None],
    },
];

// ---------------------------------------------------------------------------
// Calling the listing
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distributionsByRealtimeLogConfig` — the route the model binds this
/// operation to. None of the four members is bound to the URI or to a header, so each one
/// travels in the body, in the model's member order; a `None` leaves that element out of
/// the document entirely.
async fn list_by_config(
    router: &MultiServiceRouter,
    name: Option<&str>,
    arn: Option<&str>,
    marker: Option<&str>,
    max_items: Option<i32>,
) -> Answer {
    let mut body = format!(
        "<ListDistributionsByRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(marker) = marker {
        body.push_str(&format!("<Marker>{}</Marker>", escape(marker)));
    }
    if let Some(max_items) = max_items {
        body.push_str(&format!("<MaxItems>{max_items}</MaxItems>"));
    }
    if let Some(name) = name {
        body.push_str(&format!(
            "<RealtimeLogConfigName>{}</RealtimeLogConfigName>",
            escape(name)
        ));
    }
    if let Some(arn) = arn {
        body.push_str(&format!(
            "<RealtimeLogConfigArn>{}</RealtimeLogConfigArn>",
            escape(arn)
        ));
    }
    body.push_str("</ListDistributionsByRealtimeLogConfigRequest>");
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distributionsByRealtimeLogConfig"),
        Some(&body),
        None,
    )
    .await
}

/// The distribution ids of a `DistributionList` answer, in document order. Each item
/// travels in a `DistributionSummary` element.
fn listed_ids(list: &Node) -> Vec<String> {
    list.child("Items")
        .map(|items| {
            items
                .children_named("DistributionSummary")
                .into_iter()
                .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Seeding the store
// ---------------------------------------------------------------------------

/// Create [`CONFIG_COUNT`] real-time log configurations, returning the `(name, ARN)` of each
/// in creation order. The sampling rate varies by row so no two records are the same
/// document.
async fn create_configs(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut created = Vec::new();
    for index in 0..CONFIG_COUNT {
        let row = Submitted {
            case: "a real-time log configuration a cache behaviour can attach",
            name: format!("by-realtime-log-{index}"),
            sampling_rate: (index as i64) + 1,
            end_points: vec![Endpoint::kinesis(&format!("by-realtime-log-{index}"))],
            fields: strings(&["timestamp", "c-ip"]),
        };
        let answer = create_accepted(router, &row).await;
        let arn = config_node(&answer, row.case, "the create's answer")
            .text_at("ARN")
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case configuration {index}: expected a minted ARN, got {}",
            answer.raw
        );
        created.push((row.name.clone(), arn));
    }
    created
}

/// The `DistributionConfig` a row submits: one origin, a default cache behaviour and the
/// row's ordered cache behaviours, each sending its logs through the configuration the row
/// names.
fn config_xml(caller_reference: &str, row: &Row, configs: &[(String, String)]) -> String {
    let attachment = |index: Option<usize>| match index {
        Some(index) => format!(
            "<RealtimeLogConfigArn>{}</RealtimeLogConfigArn>",
            escape(&configs[index].1)
        ),
        None => String::new(),
    };
    let mut behaviors = format!("<Quantity>{}</Quantity>", row.ordered.len());
    if !row.ordered.is_empty() {
        behaviors.push_str("<Items>");
        for (position, index) in row.ordered.iter().enumerate() {
            behaviors.push_str(&format!(
                "<CacheBehavior><PathPattern>/path-{position}/*</PathPattern>\
                 <TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
                 </CacheBehavior>",
                attachment(*index)
            ));
        }
        behaviors.push_str("</Items>");
    }
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>a.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}\
         </DefaultCacheBehavior>\
         <CacheBehaviors>{behaviors}</CacheBehaviors>\
         <Comment>{caller_reference}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        attachment(row.default_config)
    )
}

/// Create every configuration and every row, returning the `(name, ARN)` pairs and the
/// `(row, distribution id)` pairs.
async fn seed(
    router: &MultiServiceRouter,
) -> (Vec<(String, String)>, Vec<(&'static Row, String)>) {
    let configs = create_configs(router).await;
    let mut created = Vec::new();
    for (position, row) in ROWS.iter().enumerate() {
        let body = config_xml(&format!("ref-rtlog-{position}"), row, &configs);
        let answer = create_distribution(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, answer.status, answer.raw
        );
        let id = answer.root().text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&id),
            "case {}: expected a minted distribution id, got {id:?}",
            row.case
        );
        created.push((row, id));
    }
    (configs, created)
}

/// The ids this suite expects under the configuration at `index`, in id order.
fn expected_ids(created: &[(&'static Row, String)], index: usize) -> Vec<String> {
    let mut ids: Vec<String> = created
        .iter()
        .filter(|(row, _)| row.attaches(index))
        .map(|(_, id)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Walk every page of one configuration, returning the ids in the order they were served.
async fn walk(router: &MultiServiceRouter, name: &str, page_size: Option<i32>) -> Vec<String> {
    let label = match page_size {
        None => format!("{name}/default"),
        Some(size) => format!("{name}/MaxItems={size}"),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    // At most one page per distribution plus one, so a listing that never stops truncating
    // fails here rather than looping forever.
    for page in 0..=ROWS.len() + 1 {
        let answer = list_by_config(router, Some(name), None, marker.as_deref(), page_size).await;
        assert_eq!(
            answer.status, 200,
            "case {label} page {page}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let list = answer.root();
        // "Marker: the value you provided for the Marker request parameter."
        assert_eq!(
            list.text_at("Marker"),
            Some(marker.as_deref().unwrap_or_default()),
            "case {label} page {page}: expected the Marker the request carried, got {:?}",
            list.text_at("Marker")
        );
        // "MaxItems: the value you provided for the MaxItems request parameter."
        assert_eq!(
            list.text_at("MaxItems"),
            Some(page_size.unwrap_or(MAX_ITEMS).to_string().as_str()),
            "case {label} page {page}: expected MaxItems to echo the page size, got {:?}",
            list.text_at("MaxItems")
        );
        let ids = listed_ids(list);
        assert_eq!(
            list.text_at("Quantity"),
            Some(ids.len().to_string().as_str()),
            "case {label} page {page}: expected Quantity to state the summaries served ({}), \
             got {:?}",
            ids.len(),
            list.text_at("Quantity")
        );
        if let Some(size) = page_size {
            assert!(
                i32::try_from(ids.len()).unwrap_or(i32::MAX) <= size.min(MAX_ITEMS),
                "case {label} page {page}: expected at most {} summaries, got {}",
                size.min(MAX_ITEMS),
                ids.len()
            );
        }
        collected.extend(ids.iter().cloned());

        let next_marker = list.text_at("NextMarker").map(str::to_string);
        let is_truncated = list.text_at("IsTruncated").unwrap_or_default().to_string();
        match next_marker {
            Some(next) => {
                assert_eq!(
                    is_truncated, "true",
                    "case {label} page {page}: a NextMarker means IsTruncated true, got \
                     {is_truncated:?}"
                );
                assert_eq!(
                    ids.last().map(String::as_str),
                    Some(next.as_str()),
                    "case {label} page {page}: expected NextMarker to be the id this page ended \
                     at, got {next:?}"
                );
                marker = Some(next);
            }
            None => {
                assert_eq!(
                    is_truncated, "false",
                    "case {label} page {page}: no NextMarker means IsTruncated false, got \
                     {is_truncated:?}"
                );
                return collected;
            }
        }
    }
    panic!("case {label}: the listing never stopped truncating");
}

/// Every distribution id in `ListDistributions`, in document order.
async fn listed_ids_of_distributions(router: &MultiServiceRouter) -> Vec<String> {
    let answer = list_distributions(router, "").await;
    assert_eq!(
        answer.status, 200,
        "case the unfiltered listing: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    listed_ids(answer.root())
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Each configuration lists exactly the distributions whose stored behaviours attach it,
/// once each and in id order — whether the request addresses it by name or by ARN — and
/// every id it lists is a distribution the store holds whose configuration really sends its
/// logs through that ARN.
#[tokio::test]
async fn each_configuration_lists_the_distributions_that_attach_it() {
    let router = emulator();
    let (configs, created) = seed(&router).await;
    let all_ids = listed_ids_of_distributions(&router).await;

    for (index, (name, arn)) in configs.iter().enumerate() {
        let expected = expected_ids(&created, index);
        for (context, by_name, by_arn) in [
            ("addressed by Name", Some(name.as_str()), None),
            ("addressed by ARN", None, Some(arn.as_str())),
            (
                "addressed by Name and ARN together",
                Some(name.as_str()),
                Some(arn.as_str()),
            ),
        ] {
            let answer = list_by_config(&router, by_name, by_arn, None, None).await;
            assert_eq!(
                answer.status, 200,
                "case configuration {index} ({context}): expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let ids = listed_ids(answer.root());
            assert_eq!(
                ids, expected,
                "case configuration {index} ({context}): expected the distributions that attach \
                 it, in id order, got {ids:?}"
            );

            for id in &ids {
                assert!(
                    all_ids.contains(id),
                    "case configuration {index} ({context}): expected {id} to be a distribution \
                     ListDistributions reports, got {all_ids:?}"
                );
                // The listing answers from the stored configuration: read the distribution
                // back and find the ARN among its behaviours' RealtimeLogConfigArn elements.
                let fetched = get_distribution(&router, id).await;
                assert_eq!(
                    fetched.status, 200,
                    "case configuration {index} ({context}): expected HTTP 200 from \
                     GetDistribution for {id}, got {} with {}",
                    fetched.status, fetched.raw
                );
                assert!(
                    fetched
                        .raw
                        .contains(&format!("<RealtimeLogConfigArn>{arn}</RealtimeLogConfigArn>")),
                    "case configuration {index} ({context}): expected the stored configuration \
                     of {id} to attach the real-time log configuration, got {}",
                    fetched.raw
                );
            }
        }
    }
}

/// Walking the pages at ANY admissible page size enumerates that configuration's
/// distributions exactly once, in id order.
#[tokio::test]
async fn walking_the_pages_enumerates_each_configuration_exactly_once() {
    let router = emulator();
    let (configs, created) = seed(&router).await;

    for (index, (name, _)) in configs.iter().enumerate() {
        let expected = expected_ids(&created, index);
        for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_ITEMS), Some(i32::MAX)] {
            let collected = walk(&router, name, page_size).await;
            assert_eq!(
                collected, expected,
                "case configuration {index} page size {page_size:?}: expected every distribution \
                 that attaches it exactly once in id order, got {collected:?}"
            );
        }
    }
}

/// A configuration no cache behaviour attaches lists nothing — `Quantity` zero, no summary
/// and no `NextMarker` — rather than falling back to the whole store.
#[tokio::test]
async fn a_configuration_nothing_attaches_lists_nothing() {
    let router = emulator();
    let (configs, created) = seed(&router).await;

    let unattached = configs
        .iter()
        .enumerate()
        .find(|(index, _)| expected_ids(&created, *index).is_empty())
        .map(|(_, (name, _))| name.clone())
        .expect("the table must cover a configuration nothing attaches");

    let answer = list_by_config(&router, Some(&unattached), None, None, None).await;
    assert_eq!(
        answer.status, 200,
        "case the unattached configuration: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let list = answer.root();
    assert_eq!(
        list.text_at("Quantity"),
        Some("0"),
        "case the unattached configuration: expected Quantity 0, got {:?}",
        list.text_at("Quantity")
    );
    assert!(
        listed_ids(list).is_empty(),
        "case the unattached configuration: expected no distribution summary, got {:?}",
        listed_ids(list)
    );
    assert_eq!(
        list.text_at("IsTruncated"),
        Some("false"),
        "case the unattached configuration: expected IsTruncated false, got {:?}",
        list.text_at("IsTruncated")
    );
    assert!(
        list.text_at("NextMarker").is_none(),
        "case the unattached configuration: expected no NextMarker, got {:?}",
        list.text_at("NextMarker")
    );
}

/// "If you provide both, CloudFront uses the name to identify the real-time log
/// configuration to list distributions for": a request carrying the NAME of one
/// configuration and the ARN of another answers for the name's one.
#[tokio::test]
async fn the_name_identifies_the_configuration_when_both_are_given() {
    let router = emulator();
    let (configs, created) = seed(&router).await;

    for (by_name, by_arn) in [(0usize, 1usize), (1, 0), (2, 3), (3, 0)] {
        let expected = expected_ids(&created, by_name);
        let other = expected_ids(&created, by_arn);
        let answer = list_by_config(
            &router,
            Some(configs[by_name].0.as_str()),
            Some(configs[by_arn].1.as_str()),
            None,
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case Name {by_name} with ARN {by_arn}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let ids = listed_ids(answer.root());
        assert_eq!(
            ids, expected,
            "case Name {by_name} with ARN {by_arn}: expected the distributions the NAME's \
             configuration carries, got {ids:?}"
        );
        assert_ne!(
            expected, other,
            "case Name {by_name} with ARN {by_arn}: the two configurations must differ for this \
             case to say anything"
        );
    }
}

/// "You must provide at least one": a request naming neither identifier addresses no
/// configuration at all and is refused with `InvalidArgument`. An identifier submitted as
/// an empty element names nothing either, so it is the same refusal.
#[tokio::test]
async fn a_request_naming_no_identifier_is_refused() {
    let router = emulator();
    seed(&router).await;

    let cases: [(&str, Option<&str>, Option<&str>); 4] = [
        ("neither element present", None, None),
        ("an empty Name", Some(""), None),
        ("an empty ARN", None, Some("")),
        ("both elements empty", Some(""), Some("")),
    ];
    for (case, name, arn) in cases {
        let answer = list_by_config(&router, name, arn, None, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, REALTIME_LOG_CONFIG_LISTING_IDENTIFIER_REQUIRED_MESSAGE,
            "case {case}: expected the refusal that states the constraint, got {message:?}"
        );
    }
}

/// An identifier no stored configuration answers to is refused with `InvalidArgument` —
/// never an empty page, which would claim the configuration exists — whatever shape the
/// identifier takes.
#[tokio::test]
async fn an_identifier_no_configuration_holds_is_refused() {
    let router = emulator();
    let (configs, _) = seed(&router).await;
    let (known_name, known_arn) = configs.first().cloned().expect("a created configuration");

    let absent: [(&str, Option<String>, Option<String>); 6] = [
        (
            "a name never created",
            Some("no-such-realtime-log-config".to_string()),
            None,
        ),
        (
            "a known name in upper case",
            Some(known_name.to_uppercase()),
            None,
        ),
        (
            "a known name with one character changed",
            Some(known_name.replace("by-", "bz-")),
            None,
        ),
        (
            "an ARN for a name never created",
            None,
            Some(known_arn.replace(&known_name, "never-created")),
        ),
        (
            "a token that is no ARN at all",
            None,
            Some("not-an-arn".to_string()),
        ),
        (
            "a known name with an ARN that holds nothing",
            Some(format!("{known_name}-suffix")),
            Some(known_arn.clone()),
        ),
    ];

    for (case, name, arn) in absent {
        let answer = list_by_config(&router, name.as_deref(), arn.as_deref(), None, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {name:?}/{arn:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {name:?}/{arn:?}, got {}",
            answer.raw
        );
        assert!(
            answer.root().child("Items").is_none(),
            "case {case}: expected no distribution list in a refusal, got {}",
            answer.raw
        );
    }
}

/// A page size that names no page, and a marker this service never issued, are refused as
/// invalid arguments — the same constraint every CloudFront distribution listing states.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let (configs, _) = seed(&router).await;

    let refusals: [(&str, Option<&str>, Option<i32>); 3] = [
        ("a negative page size", None, Some(-7)),
        ("a marker that is no distribution id", Some("not-an-id"), None),
        (
            "a marker that is a configuration name",
            Some("by-realtime-log-0"),
            None,
        ),
    ];
    for (name, _) in &configs {
        for (case, marker, max_items) in refusals {
            let answer = list_by_config(&router, Some(name), None, marker, max_items).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case}: expected InvalidArgument, got {}",
                answer.raw
            );
        }
    }
}
