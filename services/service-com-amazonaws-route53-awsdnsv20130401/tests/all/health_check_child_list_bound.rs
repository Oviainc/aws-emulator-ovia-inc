// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `ChildHealthChecks` list's published length.
//!
//! The rule under test: a `CALCULATED` health check may name at most the
//! published maximum of child health checks
//! (`HealthCheckConfig$ChildHealthChecks` targets `ChildHealthCheckList`,
//! `len 0..256` — `smithy_query(query="shape",shape="HealthCheckConfig")`), and a
//! request naming more is refused with `InvalidInput` and persists nothing. The
//! list is read for every assessment of the check
//! ([`service_com_amazonaws_route53_awsdnsv20130401::state::health_check_status`]),
//! so its length is a bound this service enforces rather than one it assumes.
//!
//! Beside the bound sits what the list MEANS: a child it names twice is still one
//! health check, so a list of repeats names exactly the children it mentions.
//!
//! The rows are both published bounds of the list and one past the maximum, at
//! the two forms a list takes (distinct entries and repeats of one entry), driven
//! through the real router with the same harness the other health-check suites
//! use.

use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_CHILD_HEALTH_CHECKS;

use crate::health_checks::{create_health_check, get_health_check};
use crate::hosted_zone_records::router;

/// The one input error this service's model declares (`InvalidInput`, HTTP 400).
const INVALID_INPUT: &str = "InvalidInput";

/// A `CALCULATED` configuration naming `children`, with a threshold of one.
fn calculated(children: &[String]) -> String {
    let entries: String = children
        .iter()
        .map(|child| format!("<ChildHealthCheck>{child}</ChildHealthCheck>"))
        .collect();
    format!(
        "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
         <ChildHealthChecks>{entries}</ChildHealthChecks>"
    )
}

#[tokio::test]
async fn a_calculated_check_names_at_most_the_published_number_of_children() {
    let router = router();

    // Two children to build the lists out of, each a health check of the account.
    let mut children: Vec<String> = Vec::new();
    for index in 0..2 {
        let created = create_health_check(
            &router,
            &format!("child-{index}"),
            &format!(
                "<Type>TCP</Type><IPAddress>192.0.2.{}</IPAddress><Port>443</Port>",
                index + 1
            ),
        )
        .await;
        children.push(crate::health_checks::health_check_id(&created, "a child"));
    }

    // (case, how the list is built, whether the service takes it)
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        ("no child at all, the published minimum", Vec::new(), true),
        ("one child", vec![children[0].clone()], true),
        ("two distinct children", children.clone(), true),
        (
            "the published maximum, as repeats of one child",
            vec![children[0].clone(); MAX_CHILD_HEALTH_CHECKS],
            true,
        ),
        (
            "one past the published maximum",
            vec![children[0].clone(); MAX_CHILD_HEALTH_CHECKS + 1],
            false,
        ),
    ];

    for (index, (case, list, accepted)) in rows.into_iter().enumerate() {
        let before = crate::health_checks::health_check_count(&router, case).await;
        let created =
            create_health_check(&router, &format!("parent-{index}"), &calculated(&list)).await;
        if accepted {
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected 201 from CreateHealthCheck, got {} with body {}",
                created.status, created.body
            );
            let id = crate::health_checks::health_check_id(&created, case);
            // Read back through the reader, so the list the service stored is the
            // one it reports — a repeat is persisted as the entry it is, and the
            // check is addressable afterwards.
            let read = get_health_check(&router, &id).await;
            let stored = read.all("HealthCheckConfig/ChildHealthChecks/ChildHealthCheck");
            assert_eq!(
                stored.len(),
                list.len(),
                "case {case:?}: expected the {} entries the request named, got {stored:?}",
                list.len()
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected 400, got {} with body {}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                INVALID_INPUT,
                "case {case:?}: expected {INVALID_INPUT}, got {:?}; body {}",
                created.error_code(),
                created.body
            );
            let count = crate::health_checks::health_check_count(&router, case).await;
            assert_eq!(
                count, before,
                "case {case:?}: a refused create must persist nothing, got \
                 HealthCheckCount {count} against {before}"
            );
        }
    }
}
