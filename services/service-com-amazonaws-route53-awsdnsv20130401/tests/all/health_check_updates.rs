// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateHealthCheck`.
//!
//! The rule under test: an update edits the health check the account holds in
//! place — every editable setting the request carries replaces the stored one,
//! every element `ResetElements` names goes back to the value a create that left
//! it out would have persisted, every setting the request carries none of is
//! left exactly as it was, and the health check's `HealthCheckVersion` moves on
//! by one, so the next caller's copy of that counter is the one the service
//! holds. The counter is also the optimistic-concurrency token: a request whose
//! `HealthCheckVersion` is not the stored one is refused and changes nothing,
//! and a configuration an update would leave unusable is refused the same way a
//! create of it would be.
//!
//! So the tables below drive the update over a SET of inputs rather than one
//! request: every editable member, across the health-check types whose
//! configuration carries it, at the published bounds of the members that have a
//! range and one past them, over list members whose length changes, and over the
//! whole `ResettableElementName` catalog (taken from the model's own value list).
//! Every row is read back through `GetHealthCheck`, so a write is proved by a
//! read rather than by the answer of the call that made it.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::{
    INSUFFICIENT_DATA_HEALTH_STATUS_VALUES, RESETTABLE_ELEMENT_NAME_VALUES,
};
use service_com_amazonaws_route53_awsdnsv20130401::state::FIRST_HEALTH_CHECK_VERSION;

use crate::health_checks::{create_health_check, get_health_check, health_check_id};
use crate::hosted_zone_records::{call, router, Reply};

/// The error code every refusal in this suite that is about the request's
/// contents carries: `InvalidInput`, HTTP 400, the one input error this
/// service's model declares.
const INVALID_INPUT: &str = "InvalidInput";

/// Send one `UpdateHealthCheck` request, with `members` as the body of the
/// request.
///
/// `pub(crate)` so a sibling suite drives the update through the SAME request
/// rather than growing a second copy of it.
pub(crate) async fn update_health_check(
    router: &MultiServiceRouter,
    id: &str,
    members: &str,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/healthcheck/{id}"),
        &format!(
            "<UpdateHealthCheckRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             {members}</UpdateHealthCheckRequest>"
        ),
    )
    .await
}

/// A TCP health check that exists only to be named as the child of a
/// `CALCULATED` one, and its id.
async fn child_health_check(router: &MultiServiceRouter, caller_reference: &str) -> String {
    let created = create_health_check(
        router,
        caller_reference,
        "<Type>TCP</Type><IPAddress>192.0.2.60</IPAddress><Port>443</Port>",
    )
    .await;
    health_check_id(&created, caller_reference)
}

/// `text` with the child-health-check placeholders replaced by the ids of the
/// two health checks a row may name as children.
fn fill(text: &str, child: &str, other_child: &str) -> String {
    text.replace("{child}", child)
        .replace("{other_child}", other_child)
}

/// Assert that `reply` reports each `(path, expected)` pair, where the expected
/// text is every value at that path joined by commas and `""` means the reply
/// carries no such element at all.
fn assert_reports(reply: &Reply, case: &str, pairs: &[(String, String)]) {
    for (path, expected) in pairs {
        let actual = reply.all(path).join(",");
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected {path} {expected:?}, got {actual:?}; body {}",
            reply.body
        );
    }
}

/// The `HealthCheckVersion` a reply carrying a `HealthCheck` reports.
///
/// `pub(crate)` so a sibling suite reads the counter out of a reply the same
/// way.
pub(crate) fn reported_version(reply: &Reply, case: &str) -> i64 {
    let reported = reply.first("HealthCheck/HealthCheckVersion").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the reply must carry HealthCheck/HealthCheckVersion; body {}",
            reply.body
        )
    });
    reported.parse::<i64>().unwrap_or_else(|_| {
        panic!(
            "case {case:?}: HealthCheckVersion must be a number, got {reported:?}; body {}",
            reply.body
        )
    })
}

/// One row of the editable-member table: a health check to create, the members
/// one `UpdateHealthCheck` request carries, what the health check must report
/// afterwards, and what the update must have left alone.
struct UpdateRow {
    /// What this row is about, named in every assertion it makes.
    case: &'static str,
    /// The `HealthCheckConfig` the health check is created with.
    created: &'static str,
    /// The body of the `UpdateHealthCheck` request.
    update: &'static str,
    /// What the update must change, as `(path suffix, comma-joined text)`.
    expected: &'static [(&'static str, &'static str)],
    /// What the update must leave as the create persisted it.
    unchanged: &'static [(&'static str, &'static str)],
}

/// Every editable member of a health check, across the types whose
/// configuration carries it. `FailureThreshold` (modelled `range 1..10`) and
/// `HealthThreshold` (modelled `range 0..256`) are each driven to both published
/// bounds; the list members are driven to a different length than they were
/// created with; the rest are driven to a different value of the same form.
const UPDATES: &[UpdateRow] = &[
    UpdateRow {
        case: "an IPv4 address replaced on an HTTP check",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <ResourcePath>/healthz</ResourcePath>",
        update: "<IPAddress>198.51.100.7</IPAddress>",
        expected: &[("HealthCheckConfig/IPAddress", "198.51.100.7")],
        unchanged: &[
            ("HealthCheckConfig/Type", "HTTP"),
            ("HealthCheckConfig/Port", "80"),
            ("HealthCheckConfig/ResourcePath", "/healthz"),
        ],
    },
    UpdateRow {
        case: "an IPv6 address and a port replaced on a TCP check",
        created: "<Type>TCP</Type><IPAddress>2001:db8:85a3::abcd:1:2345</IPAddress>\
                  <Port>22</Port>",
        update: "<IPAddress>2001:db8::1</IPAddress><Port>2222</Port>",
        expected: &[
            ("HealthCheckConfig/IPAddress", "2001:db8::1"),
            ("HealthCheckConfig/Port", "2222"),
        ],
        unchanged: &[("HealthCheckConfig/Type", "TCP")],
    },
    UpdateRow {
        case: "a port, a resource path and SNI replaced on an HTTPS check",
        created: "<Type>HTTPS</Type>\
                  <FullyQualifiedDomainName>secure.example.com</FullyQualifiedDomainName>\
                  <Port>443</Port><EnableSNI>true</EnableSNI>",
        update: "<Port>8443</Port><ResourcePath>/status</ResourcePath>\
                 <EnableSNI>false</EnableSNI>",
        expected: &[
            ("HealthCheckConfig/Port", "8443"),
            ("HealthCheckConfig/ResourcePath", "/status"),
            ("HealthCheckConfig/EnableSNI", "false"),
        ],
        unchanged: &[(
            "HealthCheckConfig/FullyQualifiedDomainName",
            "secure.example.com",
        )],
    },
    UpdateRow {
        case: "the domain name replaced on a check that resolves one",
        created: "<Type>HTTPS</Type>\
                  <FullyQualifiedDomainName>old.example.com</FullyQualifiedDomainName>",
        update: "<FullyQualifiedDomainName>new.example.net</FullyQualifiedDomainName>",
        expected: &[(
            "HealthCheckConfig/FullyQualifiedDomainName",
            "new.example.net",
        )],
        unchanged: &[("HealthCheckConfig/Type", "HTTPS")],
    },
    UpdateRow {
        case: "the search string replaced on a string-match check",
        created: "<Type>HTTP_STR_MATCH</Type><IPAddress>192.0.2.45</IPAddress>\
                  <Port>8080</Port><SearchString>healthy</SearchString>",
        update: "<SearchString>SERVING OK</SearchString>",
        expected: &[("HealthCheckConfig/SearchString", "SERVING OK")],
        unchanged: &[("HealthCheckConfig/Port", "8080")],
    },
    UpdateRow {
        case: "the failure threshold at the bottom of its modelled range",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <FailureThreshold>3</FailureThreshold>",
        update: "<FailureThreshold>1</FailureThreshold>",
        expected: &[("HealthCheckConfig/FailureThreshold", "1")],
        unchanged: &[("HealthCheckConfig/RequestInterval", "30")],
    },
    UpdateRow {
        case: "the failure threshold at the top of its modelled range",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <FailureThreshold>3</FailureThreshold>",
        update: "<FailureThreshold>10</FailureThreshold>",
        expected: &[("HealthCheckConfig/FailureThreshold", "10")],
        unchanged: &[("HealthCheckConfig/RequestInterval", "30")],
    },
    UpdateRow {
        case: "the status inversion and the disabling turned on",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <Inverted>false</Inverted><Disabled>false</Disabled>",
        update: "<Inverted>true</Inverted><Disabled>true</Disabled>",
        expected: &[
            ("HealthCheckConfig/Inverted", "true"),
            ("HealthCheckConfig/Disabled", "true"),
        ],
        unchanged: &[("HealthCheckConfig/IPAddress", "192.0.2.44")],
    },
    UpdateRow {
        case: "the health threshold of a CALCULATED check at the bottom of its range",
        created: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        update: "<HealthThreshold>0</HealthThreshold>",
        expected: &[("HealthCheckConfig/HealthThreshold", "0")],
        unchanged: &[(
            "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck",
            "{child}",
        )],
    },
    UpdateRow {
        case: "the health threshold of a CALCULATED check at the top of its range",
        created: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        update: "<HealthThreshold>256</HealthThreshold>",
        expected: &[("HealthCheckConfig/HealthThreshold", "256")],
        unchanged: &[(
            "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck",
            "{child}",
        )],
    },
    UpdateRow {
        case: "the child health checks replaced, one for two",
        created: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        update: "<HealthThreshold>2</HealthThreshold><ChildHealthChecks>\
                 <ChildHealthCheck>{child}</ChildHealthCheck>\
                 <ChildHealthCheck>{other_child}</ChildHealthCheck></ChildHealthChecks>",
        expected: &[
            ("HealthCheckConfig/HealthThreshold", "2"),
            (
                "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck",
                "{child},{other_child}",
            ),
        ],
        unchanged: &[("HealthCheckConfig/Type", "CALCULATED")],
    },
    UpdateRow {
        case: "the health-checker regions replaced, three for four",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <Regions><Region>us-east-1</Region><Region>us-west-1</Region>\
                  <Region>eu-west-1</Region></Regions>",
        update: "<Regions><Region>us-east-1</Region><Region>us-west-2</Region>\
                 <Region>eu-west-1</Region><Region>sa-east-1</Region></Regions>",
        expected: &[(
            "HealthCheckConfig/Regions/Region",
            "us-east-1,us-west-2,eu-west-1,sa-east-1",
        )],
        unchanged: &[("HealthCheckConfig/IPAddress", "192.0.2.44")],
    },
    UpdateRow {
        case: "the CloudWatch alarm replaced on an alarm-backed check",
        created: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
                  <Name>endpoint-unhealthy</Name></AlarmIdentifier>\
                  <InsufficientDataHealthStatus>LastKnownStatus\
                  </InsufficientDataHealthStatus>",
        update: "<AlarmIdentifier><Region>eu-west-1</Region><Name>latency-high</Name>\
                 </AlarmIdentifier>\
                 <InsufficientDataHealthStatus>Unhealthy</InsufficientDataHealthStatus>",
        expected: &[
            ("HealthCheckConfig/AlarmIdentifier/Region", "eu-west-1"),
            ("HealthCheckConfig/AlarmIdentifier/Name", "latency-high"),
            (
                "HealthCheckConfig/InsufficientDataHealthStatus",
                "Unhealthy",
            ),
        ],
        unchanged: &[("HealthCheckConfig/Type", "CLOUDWATCH_METRIC")],
    },
];

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_the_rest() {
    for (index, row) in UPDATES.iter().enumerate() {
        let router = router();
        let child = child_health_check(&router, &format!("child-a-{index}")).await;
        let other_child = child_health_check(&router, &format!("child-b-{index}")).await;

        let caller_reference = format!("update-{index}");
        let created = create_health_check(
            &router,
            &caller_reference,
            &fill(row.created, &child, &other_child),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);

        let updated =
            update_health_check(&router, &id, &fill(row.update, &child, &other_child)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
            row.case, updated.status, updated.body
        );

        // The update moved the counter on by one, and it says so in its own
        // answer as well as in the store.
        let after = reported_version(&updated, row.case);
        assert_eq!(
            after,
            FIRST_HEALTH_CHECK_VERSION + 1,
            "case {:?}: expected HealthCheckVersion {}, got {after}; body {}",
            row.case,
            FIRST_HEALTH_CHECK_VERSION + 1,
            updated.body
        );

        let pairs: Vec<(String, String)> = row
            .expected
            .iter()
            .chain(row.unchanged.iter())
            .map(|(path, expected)| {
                (
                    (*path).to_string(),
                    fill(expected, &child, &other_child),
                )
            })
            .collect();
        assert_reports(&updated, row.case, &pairs);

        // The write is proved by a READ: what the caller was told has to be what
        // a later GetHealthCheck tells it.
        let read = get_health_check(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetHealthCheck, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_reports(&read, row.case, &pairs);
        assert_reports(
            &read,
            row.case,
            &[
                ("HealthCheck/Id".to_string(), id.clone()),
                (
                    "HealthCheck/CallerReference".to_string(),
                    caller_reference.clone(),
                ),
                (
                    "HealthCheck/HealthCheckVersion".to_string(),
                    (FIRST_HEALTH_CHECK_VERSION + 1).to_string(),
                ),
            ],
        );
    }
}

#[tokio::test]
async fn the_insufficient_data_status_can_be_moved_to_any_modelled_value() {
    // `INSUFFICIENT_DATA_HEALTH_STATUS_VALUES` is the model's own
    // `InsufficientDataHealthStatus` value list, so this covers the member's
    // whole catalog rather than the one value a single request happened to use.
    for value in INSUFFICIENT_DATA_HEALTH_STATUS_VALUES {
        let router = router();
        let created = create_health_check(
            &router,
            &format!("insufficient-{value}"),
            "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
             <Name>endpoint-unhealthy</Name></AlarmIdentifier>\
             <InsufficientDataHealthStatus>Healthy</InsufficientDataHealthStatus>",
        )
        .await;
        let id = health_check_id(&created, value);

        let updated = update_health_check(
            &router,
            &id,
            &format!(
                "<InsufficientDataHealthStatus>{value}</InsufficientDataHealthStatus>"
            ),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {value:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
            updated.status, updated.body
        );

        let read = get_health_check(&router, &id).await;
        assert_reports(
            &read,
            value,
            &[(
                "HealthCheckConfig/InsufficientDataHealthStatus".to_string(),
                (*value).to_string(),
            )],
        );
    }
}

#[tokio::test]
async fn every_update_moves_the_version_on_by_one() {
    // "A sequential counter that Amazon Route 53 sets to 1 when you create a
    // health check and increments by 1 each time you update settings for the
    // health check" — so a sequence of updates walks the counter one step at a
    // time, and each step's value is the token the next step has to present.
    let router = router();
    let created = create_health_check(
        &router,
        "version-walk",
        "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
    )
    .await;
    let id = health_check_id(&created, "version-walk");
    assert_eq!(
        reported_version(&created, "the create"),
        FIRST_HEALTH_CHECK_VERSION,
        "a created health check must start at version {FIRST_HEALTH_CHECK_VERSION}; body {}",
        created.body
    );

    for (step, port) in ["8080", "8081", "8082"].iter().enumerate() {
        let case = format!("update {} to port {port}", step + 1);
        // The version the PREVIOUS step left behind, read out of the store, is
        // what this step presents as its token.
        let read = get_health_check(&router, &id).await;
        let held = reported_version(&read, &case);
        let updated = update_health_check(
            &router,
            &id,
            &format!("<HealthCheckVersion>{held}</HealthCheckVersion><Port>{port}</Port>"),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
            updated.status, updated.body
        );
        let expected = FIRST_HEALTH_CHECK_VERSION + step as i64 + 1;
        let after = reported_version(&updated, &case);
        assert_eq!(
            after, expected,
            "case {case:?}: expected HealthCheckVersion {expected}, got {after}; body {}",
            updated.body
        );
        let read_back = get_health_check(&router, &id).await;
        assert_reports(
            &read_back,
            &case,
            &[
                (
                    "HealthCheck/HealthCheckVersion".to_string(),
                    expected.to_string(),
                ),
                ("HealthCheckConfig/Port".to_string(), (*port).to_string()),
            ],
        );
    }
}

#[tokio::test]
async fn a_version_that_is_not_the_health_checks_own_is_refused() {
    // "If the value of HealthCheckVersion in the health check is greater, the
    // health check was changed after you got the version number. Route 53 does
    // not update the health check, and it returns a HealthCheckVersionMismatch
    // error." The guard holds for any value that is not the one the health check
    // carries, and a refused update leaves the health check exactly as it was.
    for (case, updates_first, presented) in [
        ("one version past a freshly created health check", 0, 2),
        ("far past a freshly created health check", 0, 7),
        ("a version from before an intervening update", 1, 1),
        ("a version from before two intervening updates", 2, 2),
    ] {
        let router = router();
        let created = create_health_check(
            &router,
            "version-guard",
            "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
        )
        .await;
        let id = health_check_id(&created, case);
        for step in 0..updates_first {
            let intervening =
                update_health_check(&router, &id, &format!("<Port>{}</Port>", 9000 + step)).await;
            assert_eq!(
                intervening.status, 200,
                "case {case:?}: the intervening update must succeed, got {} with body {}",
                intervening.status, intervening.body
            );
        }
        let before = get_health_check(&router, &id).await;
        let held = reported_version(&before, case);
        let port_before = before.first("HealthCheckConfig/Port").unwrap_or("").to_string();

        let refused = update_health_check(
            &router,
            &id,
            &format!(
                "<HealthCheckVersion>{presented}</HealthCheckVersion><Port>4433</Port>"
            ),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "HealthCheckVersionMismatch",
            "case {case:?}: expected HealthCheckVersionMismatch, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );

        // Nothing moved: not the settings the refused request named, and not
        // the counter.
        let after = get_health_check(&router, &id).await;
        assert_reports(
            &after,
            case,
            &[
                (
                    "HealthCheck/HealthCheckVersion".to_string(),
                    held.to_string(),
                ),
                ("HealthCheckConfig/Port".to_string(), port_before),
            ],
        );
    }

    // The counterpart: the version the health check actually carries is
    // accepted, which is what makes the refusals above about the value and not
    // about presenting the token at all.
    let router = router();
    let created = create_health_check(
        &router,
        "version-guard-accepted",
        "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
    )
    .await;
    let id = health_check_id(&created, "the accepted token");
    let accepted = update_health_check(
        &router,
        &id,
        &format!(
            "<HealthCheckVersion>{FIRST_HEALTH_CHECK_VERSION}</HealthCheckVersion>\
             <Port>4433</Port>"
        ),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
        "the accepted token", accepted.status, accepted.body
    );
}

#[tokio::test]
async fn the_reset_table_below_covers_every_resettable_element() {
    // `RESETTABLE_ELEMENT_NAME_VALUES` is the model's own
    // `ResettableElementName` value list, so this is what keeps the table below
    // from drifting from it.
    let mut covered: Vec<&str> = RESETS.iter().map(|row| row.element).collect();
    covered.sort_unstable();
    let mut modelled: Vec<&str> = RESETTABLE_ELEMENT_NAME_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        covered, modelled,
        "every ResettableElementName must be covered: expected {modelled:?}, got {covered:?}"
    );
}

/// One row of the reset table: a `ResettableElementName`, a health check whose
/// configuration carries that element, and the path the element is reported at.
struct ResetRow {
    /// The `ResettableElementName` this row resets.
    element: &'static str,
    /// The `HealthCheckConfig` the health check is created with, carrying the
    /// element this row resets.
    created: &'static str,
    /// Where `GetHealthCheck` reports the element.
    path: &'static str,
}

/// The whole `ResettableElementName` catalog, checked against the model's own
/// value list by the test above.
const RESETS: &[ResetRow] = &[
    ResetRow {
        element: "FullyQualifiedDomainName",
        // A check that also carries an IPAddress, so clearing the name still
        // leaves an endpoint to check.
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <FullyQualifiedDomainName>old.example.com</FullyQualifiedDomainName>",
        path: "HealthCheckConfig/FullyQualifiedDomainName",
    },
    ResetRow {
        element: "Regions",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <Regions><Region>us-east-1</Region><Region>us-west-1</Region>\
                  <Region>eu-west-1</Region></Regions>",
        path: "HealthCheckConfig/Regions/Region",
    },
    ResetRow {
        element: "ResourcePath",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <ResourcePath>/healthz</ResourcePath>",
        path: "HealthCheckConfig/ResourcePath",
    },
    ResetRow {
        element: "ChildHealthChecks",
        created: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        path: "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck",
    },
];

#[tokio::test]
async fn a_reset_puts_an_element_back_to_the_value_a_create_would_have_left() {
    for row in RESETS {
        let router = router();
        let child = child_health_check(&router, &format!("child-of-{}", row.element)).await;
        let created = create_health_check(
            &router,
            &format!("reset-{}", row.element),
            &fill(row.created, &child, &child),
        )
        .await;
        let id = health_check_id(&created, row.element);
        assert!(
            !created.all(row.path).is_empty(),
            "case {:?}: the created health check must carry the element this row resets; \
             body {}",
            row.element,
            created.body
        );

        let reset = update_health_check(
            &router,
            &id,
            &format!(
                "<ResetElements><ResettableElementName>{}</ResettableElementName>\
                 </ResetElements>",
                row.element
            ),
        )
        .await;
        assert_eq!(
            reset.status, 200,
            "case {:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
            row.element, reset.status, reset.body
        );

        // A health check created without the element reports nothing at its
        // path, so that is what the element reset to its default reports.
        let read = get_health_check(&router, &id).await;
        assert_reports(
            &read,
            row.element,
            &[
                (row.path.to_string(), String::new()),
                (
                    "HealthCheck/HealthCheckVersion".to_string(),
                    (FIRST_HEALTH_CHECK_VERSION + 1).to_string(),
                ),
            ],
        );
    }
}

#[tokio::test]
async fn a_reset_and_a_value_for_the_same_element_keeps_the_value() {
    // The two instructions in one request: the explicit setting is the more
    // specific of them, so what the health check ends up with is the value the
    // request named rather than the default the reset asks for.
    let router = router();
    let created = create_health_check(
        &router,
        "reset-and-set",
        "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
         <ResourcePath>/healthz</ResourcePath>",
    )
    .await;
    let id = health_check_id(&created, "reset and set");
    let updated = update_health_check(
        &router,
        &id,
        "<ResourcePath>/ready</ResourcePath>\
         <ResetElements><ResettableElementName>ResourcePath</ResettableElementName>\
         </ResetElements>",
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateHealthCheck, got {} with body {}",
        updated.status, updated.body
    );
    let read = get_health_check(&router, &id).await;
    assert_reports(
        &read,
        "reset and set",
        &[(
            "HealthCheckConfig/ResourcePath".to_string(),
            "/ready".to_string(),
        )],
    );
}

#[tokio::test]
async fn an_id_that_names_no_health_check_cannot_be_updated() {
    // A health check is addressed by the id the service assigned it, so no other
    // id resolves — including the caller reference it was created with, the id
    // of another kind of resource and its own id in another case.
    let router = router();
    let created = create_health_check(
        &router,
        "update-lookup",
        "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
    )
    .await;
    let id = health_check_id(&created, "the created health check");

    for case in [
        "8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c",
        "update-lookup",
        "Z1D633PJN98FT9",
        &id.to_uppercase(),
    ] {
        if case == id {
            continue;
        }
        let reply = update_health_check(&router, case, "<Port>8080</Port>").await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchHealthCheck",
            "case {case:?}: expected NoSuchHealthCheck, got {:?}; body {}",
            reply.error_code(),
            reply.body
        );
    }

    // The health check that DOES exist was left alone by every refusal above.
    let read = get_health_check(&router, &id).await;
    assert_reports(
        &read,
        "the created health check",
        &[
            ("HealthCheckConfig/Port".to_string(), "80".to_string()),
            (
                "HealthCheck/HealthCheckVersion".to_string(),
                FIRST_HEALTH_CHECK_VERSION.to_string(),
            ),
        ],
    );
}

/// One row of the refusal table: a health check to create, the update that
/// cannot be applied to it, and the member whose value must be unchanged
/// afterwards.
struct RefusalRow {
    /// What this row is about.
    case: &'static str,
    /// The `HealthCheckConfig` the health check is created with.
    created: &'static str,
    /// The body of the `UpdateHealthCheck` request that must be refused.
    update: &'static str,
    /// A `(path, text)` pair the health check must still report afterwards.
    unchanged: (&'static str, &'static str),
}

/// Updates that cannot be applied: one that would leave a configuration no
/// create could have persisted, one that would move a member outside its
/// modelled range, and one that would change whether the health check probes an
/// address at all.
const REFUSALS: &[RefusalRow] = &[
    RefusalRow {
        case: "clearing the only endpoint an HTTPS check has",
        created: "<Type>HTTPS</Type>\
                  <FullyQualifiedDomainName>only.example.com</FullyQualifiedDomainName>",
        update: "<ResetElements>\
                 <ResettableElementName>FullyQualifiedDomainName</ResettableElementName>\
                 </ResetElements>",
        unchanged: (
            "HealthCheckConfig/FullyQualifiedDomainName",
            "only.example.com",
        ),
    },
    RefusalRow {
        case: "naming a child health check the account does not hold",
        created: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        update: "<ChildHealthChecks>\
                 <ChildHealthCheck>8f3b7a1e-0c2d-4f5a-9b6c-1d2e3f4a5b6c</ChildHealthCheck>\
                 </ChildHealthChecks>",
        unchanged: (
            "HealthCheckConfig/ChildHealthChecks/ChildHealthCheck",
            "{child}",
        ),
    },
    RefusalRow {
        case: "adding an address to a check created without one",
        created: "<Type>HTTPS</Type>\
                  <FullyQualifiedDomainName>resolved.example.com</FullyQualifiedDomainName>",
        update: "<IPAddress>192.0.2.44</IPAddress>",
        unchanged: ("HealthCheckConfig/IPAddress", ""),
    },
    RefusalRow {
        case: "a port on an alarm-backed check, which takes none",
        created: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
                  <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
        update: "<Port>80</Port>",
        unchanged: ("HealthCheckConfig/Port", ""),
    },
    RefusalRow {
        case: "a failure threshold one past the top of its modelled range",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <FailureThreshold>3</FailureThreshold>",
        update: "<FailureThreshold>11</FailureThreshold>",
        unchanged: ("HealthCheckConfig/FailureThreshold", "3"),
    },
    RefusalRow {
        case: "a failure threshold one below the bottom of its modelled range",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>\
                  <FailureThreshold>3</FailureThreshold>",
        update: "<FailureThreshold>0</FailureThreshold>",
        unchanged: ("HealthCheckConfig/FailureThreshold", "3"),
    },
    RefusalRow {
        case: "a port one past the top of its modelled range",
        created: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
        update: "<Port>65536</Port>",
        unchanged: ("HealthCheckConfig/Port", "80"),
    },
    RefusalRow {
        case: "a health threshold one past the top of its modelled range",
        created: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                  <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                  </ChildHealthChecks>",
        update: "<HealthThreshold>257</HealthThreshold>",
        unchanged: ("HealthCheckConfig/HealthThreshold", "1"),
    },
];

#[tokio::test]
async fn an_update_that_cannot_be_applied_is_refused_and_changes_nothing() {
    for (index, row) in REFUSALS.iter().enumerate() {
        let router = router();
        let child = child_health_check(&router, &format!("refusal-child-{index}")).await;
        let created = create_health_check(
            &router,
            &format!("refused-update-{index}"),
            &fill(row.created, &child, &child),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
            row.case, created.status, created.body
        );
        let id = health_check_id(&created, row.case);

        let refused = update_health_check(&router, &id, &fill(row.update, &child, &child)).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400, got {} with body {}",
            row.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            INVALID_INPUT,
            "case {:?}: expected {INVALID_INPUT}, got {:?}; body {}",
            row.case,
            refused.error_code(),
            refused.body
        );

        // A refused update persists NOTHING: neither the member it named nor
        // the version counter moves.
        let (path, expected) = row.unchanged;
        let read = get_health_check(&router, &id).await;
        assert_reports(
            &read,
            row.case,
            &[
                (path.to_string(), fill(expected, &child, &child)),
                (
                    "HealthCheck/HealthCheckVersion".to_string(),
                    FIRST_HEALTH_CHECK_VERSION.to_string(),
                ),
            ],
        );
    }
}
