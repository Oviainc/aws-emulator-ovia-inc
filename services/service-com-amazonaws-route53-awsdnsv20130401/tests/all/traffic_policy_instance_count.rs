// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetTrafficPolicyInstanceCount`.
//!
//! The rule under test: the operation reports how many traffic policy instances
//! the account holds at the moment of the call — every instance, whichever
//! hosted zone it was created in, whichever policy and policy version generated
//! it, and whichever `State` it is reported in — counted from the service's
//! state, and it reports that number without changing anything.
//!
//! So the table below is not one recorded request: it walks a POPULATION over
//! several shapes — no instances at all, one instance, instances spread over two
//! hosted zones, instances generated from two policies and from several versions
//! of one policy, instances carrying different record types and TTLs, and a
//! create the service REFUSES (which must leave the number where it was) — and
//! after every step asserts the reported count against the instances that were
//! actually created and against what `GetAccountLimit` reports about the same
//! population.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::{
    TRAFFIC_POLICY_INSTANCE_APPLIED, TRAFFIC_POLICY_INSTANCE_CREATING,
    TRAFFIC_POLICY_INSTANCE_PROPAGATION,
};

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::traffic_policies::{create_policy, document_with_start_endpoint, policy_of};
use crate::traffic_policy_instances::{create_instance, get_instance, instance_count};
use crate::traffic_policy_versions::create_version;

/// How much longer than [`TRAFFIC_POLICY_INSTANCE_PROPAGATION`] a test waits
/// before it expects an instance to have reached `Applied`, so the assertion
/// does not race the span itself.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// Ask for the traffic-policy-instance count.
async fn get_traffic_policy_instance_count(router: &MultiServiceRouter) -> Reply {
    call(
        router,
        "GET",
        "/2013-04-01/trafficpolicyinstancecount",
        "",
    )
    .await
}

/// The `TrafficPolicyInstanceCount` a successful reply reports.
async fn reported_count(router: &MultiServiceRouter, case: &str) -> i32 {
    let reply = get_traffic_policy_instance_count(router).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetTrafficPolicyInstanceCount, got {} with body {}",
        reply.status, reply.body
    );
    let text = reply
        .first("TrafficPolicyInstanceCount")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry TrafficPolicyInstanceCount; body {}",
                reply.body
            )
        })
        .to_string();
    text.parse::<i32>().unwrap_or_else(|_| {
        panic!("case {case:?}: TrafficPolicyInstanceCount must be a number, got {text:?}")
    })
}

/// The `Id` of the instance a successful create reports.
fn created_instance_id(reply: &Reply, case: &str) -> String {
    reply
        .first("TrafficPolicyInstance/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create must report TrafficPolicyInstance/Id; body {}",
                reply.body
            )
        })
        .to_string()
}

/// Create a policy carrying `documents[0]` and a further version per remaining
/// document, and return its id.
async fn policy_with_versions(
    router: &MultiServiceRouter,
    name: &str,
    documents: &[String],
) -> String {
    let created = create_policy(router, name, &documents[0], None).await;
    assert_eq!(
        created.status, 201,
        "creating the policy {name:?}: expected 201, got {} with body {}",
        created.status, created.body
    );
    let id = policy_of(&created, name).id;
    for document in &documents[1..] {
        let added = create_version(router, &id, document, None).await;
        assert_eq!(
            added.status, 201,
            "adding a version to {name:?}: expected 201, got {} with body {}",
            added.status, added.body
        );
    }
    id
}

/// What one row of the population walk does to the account.
enum Step {
    /// Create an instance: the index of the zone it goes in, its `Name`, its
    /// `TTL`, the index of the policy it is generated from, and that policy's
    /// version.
    Create {
        zone: usize,
        name: &'static str,
        ttl: i64,
        policy: usize,
        version: i32,
    },
    /// A create the service refuses, which leaves the population where it was:
    /// the hosted zone id and `Name` the request carries, and the HTTP status
    /// and wire error code it is refused with.
    RefusedCreate {
        zone_id: &'static str,
        name: &'static str,
        status: u16,
        code: &'static str,
    },
}

/// One row: a step, and the number of instances the account holds once it has
/// been taken.
struct Row {
    case: &'static str,
    step: Step,
    expected: i32,
}

#[tokio::test]
async fn the_traffic_policy_instance_count_is_the_population_the_account_holds() {
    // The rows differ along the population's SIZE and its COMPOSITION — the
    // hosted zone an instance lives in, the policy and the policy VERSION it
    // was generated from, its record type and its TTL — and along a step that
    // does NOT move the population at all (a refused create), rather than
    // merely along an instance's name.
    let rows = [
        Row {
            case: "the first instance",
            step: Step::Create {
                zone: 0,
                name: "www.example.com",
                ttl: 60,
                policy: 0,
                version: 1,
            },
            expected: 1,
        },
        Row {
            case: "a second instance in the same zone, from a second version of the same policy",
            step: Step::Create {
                zone: 0,
                name: "ipv6.example.com",
                ttl: 0,
                policy: 0,
                version: 2,
            },
            expected: 2,
        },
        Row {
            case: "an instance in another hosted zone, which the total counts too",
            step: Step::Create {
                zone: 1,
                name: "www.example.net",
                ttl: 300,
                policy: 0,
                version: 1,
            },
            expected: 3,
        },
        Row {
            case: "an instance generated from a different policy",
            step: Step::Create {
                zone: 1,
                name: "text.example.net",
                ttl: 86_400,
                policy: 1,
                version: 1,
            },
            expected: 4,
        },
        Row {
            case: "a create naming a hosted zone the account does not hold",
            step: Step::RefusedCreate {
                zone_id: "Z0000000000000",
                name: "www.example.com",
                status: 404,
                code: "NoSuchHostedZone",
            },
            expected: 4,
        },
        Row {
            case: "a create naming a DNS name outside the hosted zone",
            step: Step::RefusedCreate {
                zone_id: "",
                name: "www.example.invalid",
                status: 400,
                code: "InvalidInput",
            },
            expected: 4,
        },
        Row {
            case: "a fifth instance, created after the refusals",
            step: Step::Create {
                zone: 0,
                name: "back-again.example.com",
                ttl: 120,
                policy: 0,
                version: 1,
            },
            expected: 5,
        },
    ];

    let router = router();
    let zones = [
        create_zone_ok(&router, "example.com", "instance-count-zone-a").await,
        create_zone_ok(&router, "example.net", "instance-count-zone-b").await,
    ];
    let policies = [
        policy_with_versions(
            &router,
            "instance-count-policy",
            &[
                document_with_start_endpoint("A", "192.0.2.1"),
                document_with_start_endpoint("AAAA", "2001:db8::1"),
            ],
        )
        .await,
        policy_with_versions(
            &router,
            "instance-count-other-policy",
            &[document_with_start_endpoint("TXT", "v=spf1 -all")],
        )
        .await,
    ];

    // An account that holds no instance counts none.
    let empty = reported_count(&router, "no instances at all").await;
    assert_eq!(
        empty, 0,
        "case {:?}: expected TrafficPolicyInstanceCount 0, got {empty}",
        "no instances at all"
    );

    let mut minted: Vec<String> = Vec::new();
    for row in &rows {
        match row.step {
            Step::Create {
                zone,
                name,
                ttl,
                policy,
                version,
            } => {
                let reply = create_instance(
                    &router,
                    &zones[zone],
                    name,
                    ttl,
                    &policies[policy],
                    version,
                )
                .await;
                assert_eq!(
                    reply.status, 201,
                    "case {:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
                    row.case, reply.status, reply.body
                );
                minted.push(created_instance_id(&reply, row.case));
            }
            Step::RefusedCreate {
                zone_id,
                name,
                status,
                code,
            } => {
                let addressed = if zone_id.is_empty() {
                    zones[0].clone()
                } else {
                    zone_id.to_string()
                };
                let reply =
                    create_instance(&router, &addressed, name, 60, &policies[0], 1).await;
                assert_eq!(
                    reply.status, status,
                    "case {:?}: expected HTTP {status} from the refused create, got {} with \
                     body {}",
                    row.case, reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    code,
                    "case {:?}: expected the error code {code:?}; body {}",
                    row.case,
                    reply.body
                );
            }
        }

        let count = reported_count(&router, row.case).await;
        assert_eq!(
            count, row.expected,
            "case {:?}: expected TrafficPolicyInstanceCount {}, got {count}",
            row.case, row.expected
        );
        // The row's own bookkeeping and the account limit describe the same
        // population, so the count has to agree with each.
        let held = i32::try_from(minted.len()).expect("the population fits an i32");
        assert_eq!(
            count, held,
            "case {:?}: expected the count to be the {held} instance(s) created so far, \
             got {count}",
            row.case
        );
        let limit_count = instance_count(&router).await;
        assert_eq!(
            count.to_string(),
            limit_count,
            "case {:?}: the count must agree with GetAccountLimit ({limit_count}), got {count}",
            row.case
        );
    }

    // Every instance the count claims is one the account really holds.
    for id in &minted {
        let read = get_instance(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {id:?}: a counted instance must read back, got {} with body {}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn an_instance_is_counted_whatever_state_it_is_reported_in() {
    // The count is the population, not the population's progress: an instance
    // whose record sets are still being put in place (`Creating`) counts
    // exactly as much as one that has reached `Applied`.
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-count-states").await;
    let policy = policy_with_versions(
        &router,
        "instance-count-state-policy",
        &[document_with_start_endpoint("A", "192.0.2.7")],
    )
    .await;

    let mut ids = Vec::new();
    for name in ["one.example.com", "two.example.com"] {
        let reply = create_instance(&router, &zone, name, 60, &policy, 1).await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.first("TrafficPolicyInstance/State"),
            Some(TRAFFIC_POLICY_INSTANCE_CREATING),
            "case {name:?}: a fresh instance must be reported {TRAFFIC_POLICY_INSTANCE_CREATING}; \
             body {}",
            reply.body
        );
        ids.push(created_instance_id(&reply, name));
    }

    let while_creating = reported_count(&router, "both instances still Creating").await;
    assert_eq!(
        while_creating, 2,
        "case {:?}: expected TrafficPolicyInstanceCount 2, got {while_creating}",
        "both instances still Creating"
    );

    tokio::time::sleep(TRAFFIC_POLICY_INSTANCE_PROPAGATION + SETTLE_MARGIN).await;
    for id in &ids {
        let read = get_instance(&router, id).await;
        assert_eq!(
            read.first("TrafficPolicyInstance/State"),
            Some(TRAFFIC_POLICY_INSTANCE_APPLIED),
            "case {id:?}: the instance must have reached {TRAFFIC_POLICY_INSTANCE_APPLIED}; \
             body {}",
            read.body
        );
    }

    let once_applied = reported_count(&router, "both instances Applied").await;
    assert_eq!(
        once_applied, while_creating,
        "case {:?}: the state transition must not move the count ({while_creating}), \
         got {once_applied}",
        "both instances Applied"
    );
}

#[tokio::test]
async fn reading_the_traffic_policy_instance_count_leaves_the_account_exactly_as_it_was() {
    // The rule is a report, so repeating it answers the same thing and creates,
    // destroys and consumes nothing.
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-count-idempotent").await;
    let policy = policy_with_versions(
        &router,
        "instance-count-idempotent-policy",
        &[document_with_start_endpoint("A", "192.0.2.9")],
    )
    .await;
    let mut ids = Vec::new();
    for name in ["a.example.com", "b.example.com", "c.example.com"] {
        let reply = create_instance(&router, &zone, name, 60, &policy, 1).await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
            reply.status, reply.body
        );
        ids.push(created_instance_id(&reply, name));
    }

    let first = reported_count(&router, "the first read").await;
    assert_eq!(
        first, 3,
        "case {:?}: expected TrafficPolicyInstanceCount 3, got {first}",
        "the first read"
    );
    for attempt in 1..=4 {
        let case = format!("read {attempt}");
        let again = reported_count(&router, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a repeated read must report what the first read did ({first}), \
             got {again}"
        );
    }

    // Nothing the reads did removed an instance or minted one.
    for id in &ids {
        let read = get_instance(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {id:?}: reading the count must not destroy an instance, got {} with body {}",
            read.status, read.body
        );
    }
    let limit_count = instance_count(&router).await;
    assert_eq!(
        limit_count, "3",
        "reading the count must not create or destroy an instance; GetAccountLimit reports \
         {limit_count}"
    );
}

#[tokio::test]
async fn the_traffic_policy_instance_count_is_not_the_unimplemented_refusal() {
    // A scaffolded operation answers 501, so this pins the operation as served:
    // a 200 carrying a count, whatever the population.
    let router = router();
    let reply = get_traffic_policy_instance_count(&router).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: GetTrafficPolicyInstanceCount must be served, got {} with body {}",
        "an untouched account", reply.status, reply.body
    );
    assert!(
        reply.first("TrafficPolicyInstanceCount").is_some(),
        "case {:?}: expected a TrafficPolicyInstanceCount element, got body {}",
        "an untouched account",
        reply.body
    );
}
