// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcEndpointConnectionNotifications`, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=DescribeVpcEndpointConnectionNotifications`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the connection notifications the account holds, narrowed by `ConnectionNotificationId`
//! and by the documented `Filters`, and cut into `MaxResults`-sized pages. Every table row
//! is a distinct input along what that generalises over — which documented filter, which
//! state, which notification target, how many values, how many filters, which page size — so
//! no single recorded request is the specification.

use ec2::ops::types::{CONNECTION_NOTIFICATION_STATE_VALUES, CONNECTION_NOTIFICATION_TYPE_VALUES};
use ec2::state::{
    CONNECTION_NOTIFICATION_ID_PREFIX, ComAmazonawsEc2Amazonec2State as Ec2State,
    ConnectionNotificationSpec, MAX_PAGE_SIZE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, unescaped};

/// The Region the endpoint services are hosted in, which every call here is signed for.
const REGION: &str = "us-east-1";

/// One `DescribeVpcEndpointConnectionNotifications` call carrying `params`, as its HTTP
/// status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DescribeVpcEndpointConnectionNotifications",
        params,
    )
    .await
}

/// The notification ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "connectionNotificationId")
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// One `ConnectionNotificationId` parameter.
fn id_param(connection_notification_id: &str) -> Vec<(String, String)> {
    vec![(
        "ConnectionNotificationId".to_string(),
        connection_notification_id.to_string(),
    )]
}

/// The ARN of the SNS topic a notification publishes to.
fn topic_arn(name: &str) -> String {
    engine::emulator_arn("sns", REGION, name)
}

/// What one seeded notification is created with, beyond the id the store mints itself.
struct NotificationFixture {
    /// Which of the seeded endpoint services this notification is for, if it is for one:
    /// "You can create a connection notification for a specified VPC endpoint **or** VPC
    /// endpoint service" (`CreateVpcEndpointConnectionNotification`).
    service_index: Option<usize>,
    /// Which of the seeded endpoints this notification is for, if it is for one.
    endpoint_index: Option<usize>,
    /// The notification's state, a member of the `ConnectionNotificationState` enum.
    state: &'static str,
    /// The notification's type, a member of the `ConnectionNotificationType` enum.
    notification_type: &'static str,
    /// "The endpoint events for which to receive notifications. Valid values are Accept,
    /// Connect, Delete, and Reject" (`CreateVpcEndpointConnectionNotificationRequest$ConnectionEvents`).
    events: &'static [&'static str],
    /// The name of the SNS topic the notification publishes to.
    topic: &'static str,
}

/// The notifications the filter and paging tables are written against. They differ along
/// every attribute the documented filters select on — the ARN, the state, the type, the
/// service and the endpoint — and along the shapes of the event lists they carry.
fn fixtures() -> Vec<NotificationFixture> {
    vec![
        NotificationFixture {
            service_index: Some(0),
            endpoint_index: None,
            state: "Enabled",
            notification_type: "Topic",
            events: &["Accept", "Connect", "Delete", "Reject"],
            topic: "endpoint-service-events",
        },
        NotificationFixture {
            service_index: Some(0),
            endpoint_index: None,
            state: "Disabled",
            notification_type: "Topic",
            events: &["Delete"],
            topic: "endpoint-service-deletions",
        },
        NotificationFixture {
            service_index: Some(1),
            endpoint_index: None,
            state: "Enabled",
            notification_type: "Topic",
            events: &["Accept", "Reject"],
            topic: "endpoint-service-decisions",
        },
        NotificationFixture {
            service_index: None,
            endpoint_index: Some(0),
            state: "Enabled",
            notification_type: "Topic",
            events: &["Connect"],
            topic: "endpoint-connections",
        },
        NotificationFixture {
            service_index: None,
            endpoint_index: Some(0),
            state: "Disabled",
            notification_type: "Topic",
            events: &[],
            topic: "endpoint-silence",
        },
        NotificationFixture {
            service_index: None,
            endpoint_index: Some(1),
            state: "Enabled",
            notification_type: "Topic",
            events: &["Accept", "Connect"],
            topic: "endpoint-acceptances",
        },
        NotificationFixture {
            service_index: Some(2),
            endpoint_index: Some(1),
            state: "Disabled",
            notification_type: "Topic",
            events: &["Reject"],
            topic: "endpoint-rejections",
        },
        NotificationFixture {
            service_index: Some(2),
            endpoint_index: None,
            state: "Enabled",
            notification_type: "Topic",
            events: &["Accept"],
            topic: "endpoint-service-acceptances",
        },
    ]
}

/// The ids the seeded notifications and the resources they name were given.
struct Seeded {
    /// The `ConnectionNotificationId` of each fixture, in fixture order — which is also
    /// store order, the ids being minted in that order.
    notification_ids: Vec<String>,
    /// The `ServiceId` of each seeded endpoint service.
    service_ids: Vec<String>,
    /// The `VpcEndpointId` of each seeded endpoint.
    endpoint_ids: Vec<String>,
}

/// Write the seeded notifications into the engine's store, the way an operation creating one
/// would, and report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let service_ids: Vec<String> = (0..3).map(|_| guard.next_id("vpce-svc")).collect();
    let endpoint_ids: Vec<String> = (0..2).map(|_| guard.next_id("vpce")).collect();
    let mut notification_ids = Vec::new();
    for fixture in fixtures() {
        let notification_id = guard
            .insert_vpc_endpoint_connection_notification(ConnectionNotificationSpec {
                service_id: fixture
                    .service_index
                    .map(|index| service_ids[index].clone()),
                vpc_endpoint_id: fixture
                    .endpoint_index
                    .map(|index| endpoint_ids[index].clone()),
                connection_notification_type: fixture.notification_type.to_string(),
                connection_notification_arn: topic_arn(fixture.topic),
                connection_events: fixture
                    .events
                    .iter()
                    .map(|event| (*event).to_string())
                    .collect(),
                connection_notification_state: fixture.state.to_string(),
                service_region: REGION.to_string(),
            })
            .expect("the store must have room for the seeded notification");
        notification_ids.push(notification_id);
    }

    Seeded {
        notification_ids,
        service_ids,
        endpoint_ids,
    }
}

/// The notification ids of the fixtures at `indices`, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.notification_ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has created no connection notification, so the report is empty — and
/// describing is a READ: two calls answer the same and the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_no_notification_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "connectionNotificationId").is_empty(),
        "fresh account: expected no notification, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty report, so no token is due, got {body}"
    );

    let (second_status, second) = describe(&router, &[]).await;
    assert_eq!(
        second_status, 200,
        "second read: expected HTTP 200, got {second_status} with body {second}"
    );
    assert_eq!(
        elements(&second, "connectionNotificationId"),
        Vec::<String>::new(),
        "a read must not change what a later caller sees, got {second}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .vpc_endpoint_connection_notifications
            .is_empty(),
        "describing notifications must not write to the store"
    );
}

/// Every notification the store holds is reported, in store order, with the members it was
/// stored with — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_notification_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all notifications", &[]).await;
    assert_eq!(
        reported, seeded.notification_ids,
        "all notifications: expected every stored notification in store order, got {reported:?}"
    );

    // One notification, member by member: everything the write set round-trips through the
    // describe, including the event list.
    let (status, body) = describe(&router, &id_param(&seeded.notification_ids[6])).await;
    assert_eq!(status, 200, "one notification: got {status} with body {body}");
    let expected_members = [
        (
            "connectionNotificationId",
            seeded.notification_ids[6].clone(),
        ),
        ("serviceId", seeded.service_ids[2].clone()),
        ("vpcEndpointId", seeded.endpoint_ids[1].clone()),
        ("connectionNotificationType", "Topic".to_string()),
        (
            "connectionNotificationArn",
            topic_arn("endpoint-rejections"),
        ),
        ("connectionNotificationState", "Disabled".to_string()),
        ("serviceRegion", REGION.to_string()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "one notification: expected <{member}>{value}</{member}>, got {body}"
        );
    }
    assert!(
        body.contains("<item>Reject</item>"),
        "one notification: expected the stored event list, got {body}"
    );

    // A notification stored after the first call is reported by the next one.
    let late_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard
            .insert_vpc_endpoint_connection_notification(ConnectionNotificationSpec {
                service_id: Some(seeded.service_ids[0].clone()),
                vpc_endpoint_id: None,
                connection_notification_type: "Topic".to_string(),
                connection_notification_arn: topic_arn("late-events"),
                connection_events: vec!["Accept".to_string()],
                connection_notification_state: "Enabled".to_string(),
                service_region: REGION.to_string(),
            })
            .expect("the store must have room for the seeded notification")
    };

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.notification_ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// ConnectionNotificationId
// ---------------------------------------------------------------------------

/// `ConnectionNotificationId` narrows the report to the one notification it names, for each
/// of the stored notifications rather than only the first.
#[tokio::test]
async fn the_named_notification_is_the_only_one_reported() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (index, notification_id) in seeded.notification_ids.iter().enumerate() {
        let case = format!("ConnectionNotificationId={notification_id}");
        let reported = described_ids(&router, &case, &id_param(notification_id)).await;
        assert_eq!(
            reported,
            vec![notification_id.clone()],
            "case {case:?} (fixture {index}): expected only {notification_id:?}, got {reported:?}"
        );
    }

    // The id member and a filter narrow the SAME report, so a notification has to satisfy
    // both: naming one notification and filtering for another's state selects nothing.
    let mut both = id_param(&seeded.notification_ids[0]);
    both.extend(filters(&[("connection-notification-state", &["Disabled"])]));
    let reported = described_ids(&router, "id AND filter", &both).await;
    assert!(
        reported.is_empty(),
        "id AND filter: the named notification is Enabled, so a Disabled filter selects \
         nothing, got {reported:?}"
    );
}

/// A `ConnectionNotificationId` this account holds no notification for is refused, and one
/// that is not such an id at all is refused as malformed — the two cases a caller has to be
/// able to tell apart.
#[tokio::test]
async fn an_unknown_or_malformed_notification_id_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a well-formed id of each length EC2 issues that this store never minted.
    let unknown = [
        format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-0123456789abcdef0"),
        format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-deadbeef"),
    ];
    for notification_id in unknown {
        let (status, body) = describe(&router, &id_param(&notification_id)).await;
        assert_eq!(
            status, 400,
            "case {notification_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidConnectionNotificationId.NotFound",
            "case {notification_id:?}: expected the NotFound code, got {body}"
        );
        assert!(
            error_message(&body).contains(&notification_id),
            "case {notification_id:?}: expected the message to name the id, got {body}"
        );
        assert!(
            elements(&body, "connectionNotificationId").is_empty(),
            "case {notification_id:?}: a refused request reports no notification, got {body}"
        );
    }

    // Rows: another resource's id, a prefix with no digits, too few and too many digits,
    // upper-case digits, an id of a kind this operation does not name, and the empty string —
    // a member named with no value still names an id, and that is not one.
    let malformed = [
        seeded.service_ids[0].clone(),
        seeded.endpoint_ids[0].clone(),
        format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-"),
        format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-0123456"),
        format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-0123456789abcdef01"),
        format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-DEADBEEF"),
        "not-an-id".to_string(),
        String::new(),
    ];
    for notification_id in malformed {
        let (status, body) = describe(&router, &id_param(&notification_id)).await;
        assert_eq!(
            status, 400,
            "case {notification_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidConnectionNotificationId.Malformed",
            "case {notification_id:?}: expected the Malformed code, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects the notifications whose own attribute matches a value;
/// values within one filter are OR'd and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_notifications_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let all: Vec<usize> = (0..seeded.notification_ids.len()).collect();

    // One row per filter name `DescribeVpcEndpointConnectionNotificationsRequest$Filters`
    // documents, with rows for the value counts, the wildcards and the case rule.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "connection-notification-arn",
            vec![topic_arn("endpoint-connections")],
            vec![3],
        ),
        (
            "connection-notification-arn",
            vec![topic_arn("endpoint-connections"), topic_arn("endpoint-silence")],
            vec![3, 4],
        ),
        // EC2's own `*` wildcard, which every topic of this account matches.
        (
            "connection-notification-arn",
            vec![format!("{}*", topic_arn("endpoint-service-"))],
            vec![0, 1, 2, 7],
        ),
        (
            "connection-notification-id",
            vec![seeded.notification_ids[5].clone()],
            vec![5],
        ),
        (
            "connection-notification-id",
            vec![format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-*")],
            all.clone(),
        ),
        (
            "connection-notification-state",
            vec!["Enabled".into()],
            vec![0, 2, 3, 5, 7],
        ),
        (
            "connection-notification-state",
            vec!["Disabled".into()],
            vec![1, 4, 6],
        ),
        (
            "connection-notification-state",
            vec!["Enabled".into(), "Disabled".into()],
            all.clone(),
        ),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("connection-notification-state", vec!["enabled".into()], vec![]),
        ("connection-notification-type", vec!["Topic".into()], all),
        ("connection-notification-type", vec!["topic".into()], vec![]),
        (
            "service-id",
            vec![seeded.service_ids[0].clone()],
            vec![0, 1],
        ),
        (
            "service-id",
            vec![seeded.service_ids[1].clone(), seeded.service_ids[2].clone()],
            vec![2, 6, 7],
        ),
        (
            "vpc-endpoint-id",
            vec![seeded.endpoint_ids[0].clone()],
            vec![3, 4],
        ),
        (
            "vpc-endpoint-id",
            vec![seeded.endpoint_ids[1].clone()],
            vec![5, 6],
        ),
        // A documented filter carrying no value at all: there is no value for the attribute
        // to match, so it selects nothing rather than everything.
        ("connection-notification-state", vec![], vec![]),
        ("service-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter naming an attribute a notification does not CARRY selects nothing for it: a
/// notification of an endpoint service carries no `VpcEndpointId`, and one of an endpoint
/// carries no `ServiceId`.
#[tokio::test]
async fn a_filter_on_an_attribute_a_notification_lacks_selects_nothing_for_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Every notification the store holds whose own service or endpoint member is unset must
    // be absent from the report of a filter naming that member with the wildcard every value
    // matches.
    let cases: [(&str, Vec<usize>); 2] = [
        // The notifications that name an endpoint at all.
        ("vpc-endpoint-id", vec![3, 4, 5, 6]),
        // The notifications that name an endpoint service at all.
        ("service-id", vec![0, 1, 2, 6, 7]),
    ];
    for (name, expected_indices) in cases {
        let case = format!("{name}=*");
        let reported = described_ids(&router, &case, &filters(&[(name, &["*"])])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected only the notifications carrying that member, \
             {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the notifications that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the notifications matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let service_two = seeded.service_ids[2].clone();
    let service_two_values = [service_two.as_str()];
    let endpoint_one = seeded.endpoint_ids[1].clone();
    let endpoint_one_values = [endpoint_one.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("service-id", &service_two_values[..]),
                ("connection-notification-state", &["Disabled"][..]),
            ],
            vec![6],
        ),
        (
            vec![
                ("vpc-endpoint-id", &endpoint_one_values[..]),
                ("connection-notification-type", &["Topic"][..]),
            ],
            vec![5, 6],
        ),
        (
            vec![
                ("connection-notification-type", &["Topic"][..]),
                ("connection-notification-state", &["Enabled"][..]),
                ("vpc-endpoint-id", &["*"][..]),
            ],
            vec![3, 5],
        ),
        // Each filter matches something, but no notification matches both.
        (
            vec![
                ("service-id", &service_two_values[..]),
                ("vpc-endpoint-id", &endpoint_one_values[..]),
                ("connection-notification-state", &["Enabled"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// Every member of the modelled `ConnectionNotificationState` and
/// `ConnectionNotificationType` enums selects the notification stored in it — the whole
/// catalog the model publishes, not the values this suite happened to seed above.
#[tokio::test]
async fn every_modelled_state_and_type_selects_the_notification_carrying_it() {
    let (router, engine) = super::ec2_query::emulator();

    // One notification per (state, type) pair the model publishes.
    let mut seeded_pairs: Vec<(&str, &str, String)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let service_id = guard.next_id("vpce-svc");
        for state in CONNECTION_NOTIFICATION_STATE_VALUES {
            for notification_type in CONNECTION_NOTIFICATION_TYPE_VALUES {
                let notification_id = guard
                    .insert_vpc_endpoint_connection_notification(ConnectionNotificationSpec {
                        service_id: Some(service_id.clone()),
                        vpc_endpoint_id: None,
                        connection_notification_type: (*notification_type).to_string(),
                        connection_notification_arn: topic_arn(&format!(
                            "{state}-{notification_type}"
                        )),
                        connection_events: vec!["Accept".to_string()],
                        connection_notification_state: (*state).to_string(),
                        service_region: REGION.to_string(),
                    })
                    .expect("the store must have room for the seeded notification");
                seeded_pairs.push((state, notification_type, notification_id));
            }
        }
    }

    for (state, notification_type, notification_id) in &seeded_pairs {
        let case = format!("state={state:?} type={notification_type:?}");
        let reported = described_ids(
            &router,
            &case,
            &filters(&[
                ("connection-notification-state", &[state]),
                ("connection-notification-type", &[notification_type]),
            ]),
        )
        .await;
        assert_eq!(
            reported,
            vec![notification_id.clone()],
            "case {case:?}: expected only {notification_id:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the tag filters this operation does NOT document (other describes do), the
    // underscore spelling, a documented name with the wrong case, near-misses of two
    // documented names, a filter of another describe, and an empty name.
    let cases = [
        "tag:Name",
        "tag-key",
        "connection_notification_state",
        "Connection-Notification-State",
        "connection-notification-states",
        "service-ids",
        "service-name",
        "vpc-endpoint-state",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: expected the message to name the filter, got {body}"
        );
        assert!(
            elements(&body, "connectionNotificationId").is_empty(),
            "case {name:?}: a refused request reports no notification, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every notification exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_notification_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: the smallest page there is, one that does not divide the store, one that holds
    // it exactly, and the largest this service serves.
    for max_results in [1, 3, 8, i64::try_from(MAX_PAGE_SIZE).unwrap_or(i64::MAX)] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "connectionNotificationId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.notification_ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                None => break,
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
            }
        }
        assert_eq!(
            walked, seeded.notification_ids,
            "case {case:?}: expected every notification exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page is cut from the SELECTED set, not from the whole store: `MaxResults` composes with
/// a filter, and the token resumes within the filtered set.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[("connection-notification-state", &["Enabled"])]);
    params.extend(max_results_param(3));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    assert_eq!(
        elements(&body, "connectionNotificationId"),
        ids_at(&seeded, &[0, 2, 3]),
        "filtered first page: expected the first three Enabled notifications, got {body}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut resumed = filters(&[("connection-notification-state", &["Enabled"])]);
    resumed.extend(max_results_param(3));
    resumed.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &resumed).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5, 7]),
        "filtered second page: expected the Enabled notifications left, got {second:?}"
    );
}

/// `MaxResults` is accepted at each bound this service serves and refused outside them: the
/// model declares no range on the member and this operation documents none, so the bound is
/// the service's own largest page.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_outside_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let largest = i64::try_from(MAX_PAGE_SIZE).unwrap_or(i64::MAX);
    // Rows: past the lower bound, each bound itself, and past the upper bound.
    let cases = [
        (-1, 400),
        (0, 400),
        (1, 200),
        (largest, 200),
        (largest + 1, 400),
    ];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case MaxResults={max_results}: expected InvalidParameterValue, got {body}"
            );
        } else {
            assert!(
                element(&body, "connectionNotificationId").is_some(),
                "case MaxResults={max_results}: expected a reported notification, got {body}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // notification id a caller might guess the token is, and that id hex-encoded (the shape
    // the token takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.notification_ids[0].clone(),
        ::hex::encode(&seeded.notification_ids[0]),
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` on a request that would have succeeded answers `DryRunOperation` and reports
/// nothing; a request that would have been REFUSED is refused with its own reason instead.
#[tokio::test]
async fn dry_run_reports_whether_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let dry_run = vec![("DryRun".to_string(), "true".to_string())];
    let (status, body) = describe(&router, &dry_run).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "connectionNotificationId").is_empty(),
        "a dry run reports no notification, got {body}"
    );

    // Rows: a dry run whose filter name is undefined, one whose page size is below the
    // smallest page there is, and one naming a notification this account does not hold —
    // each refused for that reason rather than with DryRunOperation.
    let mut undefined_filter = filters(&[("tag:Name", &["web"])]);
    undefined_filter.extend(dry_run.clone());
    let mut small_page = max_results_param(0);
    small_page.extend(dry_run.clone());
    let mut unknown_id = id_param(&format!("{CONNECTION_NOTIFICATION_ID_PREFIX}-0123456789abcdef0"));
    unknown_id.extend(dry_run);
    for (case, params, expected_code) in [
        ("an undefined filter", undefined_filter, "InvalidParameterValue"),
        ("MaxResults=0", small_page, "InvalidParameterValue"),
        (
            "an unknown notification",
            unknown_id,
            "InvalidConnectionNotificationId.NotFound",
        ),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A dry run must not have changed the store either.
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .vpc_endpoint_connection_notifications
            .len(),
        seeded.notification_ids.len(),
        "a dry run must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// The store's own bounds
// ---------------------------------------------------------------------------

/// The notification store's count ceiling is STRUCTURAL:
/// `insert_vpc_endpoint_connection_notification` asks it itself, so a writer that forgets
/// [`Ec2State::has_vpc_endpoint_connection_notification_capacity`] still cannot grow the
/// store past [`ec2::state::MAX_STORED_RECORDS_PER_STORE`] — nothing reclaims a stored
/// notification.
#[test]
fn the_notification_store_refuses_to_grow_past_its_count_ceiling() {
    use ec2::state::MAX_STORED_RECORDS_PER_STORE;

    let engine = engine::InMemoryEngine::new();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let spec = || ConnectionNotificationSpec {
        service_id: None,
        vpc_endpoint_id: None,
        connection_notification_type: "Topic".to_string(),
        connection_notification_arn: topic_arn("ceiling"),
        connection_events: vec!["Accept".to_string()],
        connection_notification_state: "Enabled".to_string(),
        service_region: REGION.to_string(),
    };

    for index in 0..MAX_STORED_RECORDS_PER_STORE {
        assert!(
            guard
                .insert_vpc_endpoint_connection_notification(spec())
                .is_some(),
            "case {index}: a notification below the ceiling must be stored"
        );
    }
    assert!(
        !guard.has_vpc_endpoint_connection_notification_capacity(),
        "a store at {} notifications must report itself full",
        guard.vpc_endpoint_connection_notifications.len()
    );
    assert_eq!(
        guard.insert_vpc_endpoint_connection_notification(spec()),
        None,
        "a notification past the ceiling must be refused, not stored"
    );
    assert_eq!(
        guard.vpc_endpoint_connection_notifications.len(),
        MAX_STORED_RECORDS_PER_STORE,
        "the refused notification must leave the store at its ceiling"
    );
}
