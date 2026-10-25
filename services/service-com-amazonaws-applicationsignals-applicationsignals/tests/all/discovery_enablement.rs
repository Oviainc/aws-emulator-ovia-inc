// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `StartDiscovery` over HTTP: enabling an account to use
//! Application Signals.
//!
//! The rule under test: `StartDiscovery` enables the caller's account by creating the
//! account's service-linked role, and such a role is created only while the account holds
//! none — so the FIRST call records the account's enablement (the role's ARN, and the
//! instant the role was created) and every later call finds the account already enabled,
//! changes nothing and answers the same empty HTTP 200.
//!
//! Two consequences are asserted beside it: nothing but this call enables an account (no
//! other operation, read or write, leaves the enablement behind), and the recorded role ARN
//! is an IAM ARN, so it carries no Region whichever Region the caller signed for.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the route, the status code and the JSON body are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state;
use smithy_server_runtime::MultiServiceRouter;

// The SLI config an SLO is created with, reused from the SLO lifecycle suite rather than
// re-derived here.
use super::slo_lifecycle::sli_config;

/// The ARN the account's enablement must record.
///
/// Written out here rather than read from the service's own helper, so the test pins the
/// string a client sees: `AWSServiceRoleForCloudWatchApplicationSignals` is the role the
/// operation's model doc names ("by creating the
/// AWSServiceRoleForCloudWatchApplicationSignals service-linked role"),
/// `role/aws-service-role/<service principal>/<name>` is how IAM files a service-linked
/// role, and `000000000000` is this emulator's canonical account
/// (`engine::emulator_account_id`). The role is an IAM resource and IAM is global, so the
/// Region field is EMPTY.
const EXPECTED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/aws-service-role/\
     application-signals.amazonaws.com/AWSServiceRoleForCloudWatchApplicationSignals";

/// A router with just this service mounted on a fresh engine, together with that engine —
/// the account's enablement is a store record rather than a response member, so the suite
/// reads it back through the same store the handlers write.
fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Send one request signed for `region` and return its status, its raw body and the parsed
/// JSON body (`Value::Null` when the body is empty or not JSON).
async fn raw_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Vec<u8>, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    // The SigV4 credential scope routes the request to this service and names the Region
    // the caller signed for. Signature verification is off (see
    // `set_request_authentication`), so the signature bytes are never read.
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/application-signals/\
         aws4_request, SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{region}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, body, parsed)
}

/// [`raw_call`] without the raw bytes.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _raw, parsed) = raw_call(router, region, method, uri, body).await;
    (status, parsed)
}

/// `POST /start-discovery` — the operation's input is an empty structure, so the request
/// carries no members.
async fn start_discovery(
    router: &MultiServiceRouter,
    region: &str,
) -> (u16, Vec<u8>, Value) {
    raw_call(router, region, "POST", "/start-discovery", None).await
}

/// The account's enablement as the store holds it: the recorded role ARN and the instant
/// the role was created, or `None` while the account has never been enabled.
fn recorded_enablement(engine: &Arc<engine::InMemoryEngine>) -> Option<(String, f64)> {
    let store = state::service_state(&**engine).expect("this service's store");
    let guard = store.lock();
    guard.discovery().map(|discovery| {
        (
            discovery.service_linked_role_arn.clone(),
            discovery.started_at.as_secs_f64(),
        )
    })
}

/// Seconds since the epoch, the scale `DateTime::as_secs_f64` reports an instant on.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// What the account's store already holds when discovery is started, so the rule is
/// asserted on an account that is empty and on ones that already hold records.
#[derive(Debug, Clone, Copy)]
enum Existing {
    /// An account that holds nothing at all.
    Nothing,
    /// An account that already holds a service level objective.
    Slo,
    /// An account that already holds a grouping configuration.
    GroupingConfiguration,
}

/// One row of the table: the Region the caller signs for, how many times the call is
/// repeated, and what the account already holds.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    region: &'static str,
    calls: usize,
    existing: Existing,
}

/// The rows the rule is asserted over.
///
/// They differ along what the rule generalises over: how MANY times the call is repeated
/// (once, twice, several times), the Region the caller signs for — Regions from three
/// partitions' worth of prefixes, none of which may reach the recorded IAM ARN — and what
/// the account's store already holds when the call arrives.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one call on an account that holds nothing",
            region: "us-east-1",
            calls: 1,
            existing: Existing::Nothing,
        },
        Case {
            label: "a repeated call on an account that holds nothing",
            region: "eu-west-2",
            calls: 2,
            existing: Existing::Nothing,
        },
        Case {
            label: "five calls on an account that holds nothing",
            region: "ap-southeast-1",
            calls: 5,
            existing: Existing::Nothing,
        },
        Case {
            label: "an account that already holds an SLO",
            region: "us-west-2",
            calls: 2,
            existing: Existing::Slo,
        },
        Case {
            label: "an account that already holds a grouping configuration",
            region: "sa-east-1",
            calls: 3,
            existing: Existing::GroupingConfiguration,
        },
    ]
}

/// The SLO one row stores before enabling the account.
fn existing_slo() -> Value {
    json!({
        "Name": "checkout-latency",
        "SliConfig": sli_config("checkout", "POST /checkout", 250.0),
        "Goal": { "AttainmentGoal": 99.9 }
    })
}

/// The grouping configuration one row stores before enabling the account.
fn existing_grouping_configuration() -> Value {
    json!([{ "GroupingName": "BusinessUnit", "GroupingSourceKeys": ["business_unit"] }])
}

/// Put `existing` in the store, and hand back the record a later read must still report.
async fn set_up(router: &MultiServiceRouter, region: &str, existing: Existing) {
    match existing {
        Existing::Nothing => {}
        Existing::Slo => {
            let (status, answer) =
                call(router, region, "POST", "/slo", Some(existing_slo())).await;
            assert_eq!(
                status, 200,
                "setting up an SLO must succeed, got {status} with {answer}"
            );
        }
        Existing::GroupingConfiguration => {
            let (status, answer) = call(
                router,
                region,
                "PUT",
                "/grouping-configuration",
                Some(json!({
                    "GroupingAttributeDefinitions": existing_grouping_configuration()
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "setting up a grouping configuration must succeed, got {status} with {answer}"
            );
        }
    }
}

/// Whatever `existing` put in the store is still there, unchanged by the enablement.
async fn assert_existing_survives(
    router: &MultiServiceRouter,
    region: &str,
    existing: Existing,
    label: &str,
) {
    match existing {
        Existing::Nothing => {}
        Existing::Slo => {
            let (status, answer) =
                call(router, region, "GET", "/slo/checkout-latency", None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the stored SLO to survive the enablement, got \
                 {status} with {answer}"
            );
            assert_eq!(
                answer["Slo"]["Name"].as_str(),
                Some("checkout-latency"),
                "case {label:?}: expected the stored SLO to be reported unchanged, got {answer}"
            );
        }
        Existing::GroupingConfiguration => {
            let (status, answer) = call(
                router,
                region,
                "POST",
                "/grouping-attribute-definitions",
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the grouping configuration read to succeed, got \
                 {status} with {answer}"
            );
            assert_eq!(
                answer["GroupingAttributeDefinitions"],
                existing_grouping_configuration(),
                "case {label:?}: expected the stored definitions to survive the enablement, \
                 got {:?}",
                answer["GroupingAttributeDefinitions"]
            );
        }
    }
}

/// The first call enables the account — recording the service-linked role it created and
/// the instant it created it — and every later call answers the same empty 200 without
/// moving that record.
#[tokio::test]
async fn the_first_call_enables_the_account_and_later_calls_change_nothing() {
    for case in cases() {
        let (router, engine) = harness();
        set_up(&router, case.region, case.existing).await;

        assert!(
            recorded_enablement(&engine).is_none(),
            "case {:?}: expected no enablement before the first call, got {:?}",
            case.label,
            recorded_enablement(&engine)
        );

        let before = now_seconds();
        let mut first: Option<(String, f64)> = None;
        for attempt in 1..=case.calls {
            let (status, raw, body) = start_discovery(&router, case.region).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected HTTP 200, got {status} with {body}",
                case.label
            );
            assert_eq!(
                String::from_utf8_lossy(&raw),
                "{}",
                "case {:?} attempt {attempt}: expected the empty body {{}}, got {:?}",
                case.label,
                String::from_utf8_lossy(&raw)
            );

            let recorded = recorded_enablement(&engine).unwrap_or_else(|| {
                panic!(
                    "case {:?} attempt {attempt}: expected the call to enable the account, \
                     got no record",
                    case.label
                )
            });
            assert_eq!(
                recorded.0, EXPECTED_ROLE_ARN,
                "case {:?} attempt {attempt}: expected the enablement to record \
                 {EXPECTED_ROLE_ARN}, got {:?}",
                case.label, recorded.0
            );

            match &first {
                None => first = Some(recorded),
                Some(first) => assert_eq!(
                    *first, recorded,
                    "case {:?} attempt {attempt}: expected the record of the first call to \
                     be left as it is, got {recorded:?}",
                    case.label
                ),
            }
        }
        let after = now_seconds();

        let (_, started_at) = first.expect("every row makes at least one call");
        assert!(
            started_at >= before && started_at <= after,
            "case {:?}: expected the role to be created at the clock reading of the call, \
             between {before} and {after}, got {started_at}",
            case.label
        );

        assert_existing_survives(&router, case.region, case.existing, case.label).await;
    }
}

/// The recorded ARN names IAM, which is global: the Region field is empty whichever Region
/// the caller signed for, so two accounts' worth of callers in different Regions read the
/// same role.
#[tokio::test]
async fn the_recorded_role_arn_carries_no_region() {
    // Regions spanning the prefixes a caller signs with, including one that is also the
    // emulator's documented default (`state::DEFAULT_REGION`).
    for region in [
        state::DEFAULT_REGION,
        "eu-central-1",
        "ap-northeast-3",
        "ca-central-1",
    ] {
        let (router, engine) = harness();
        let (status, _raw, body) = start_discovery(&router, region).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {body}"
        );

        let (arn, _) = recorded_enablement(&engine).unwrap_or_else(|| {
            panic!("case {region:?}: expected the call to enable the account, got no record")
        });
        assert_eq!(
            arn, EXPECTED_ROLE_ARN,
            "case {region:?}: expected the Region-free ARN {EXPECTED_ROLE_ARN}, got {arn:?}"
        );
        assert!(
            !arn.contains(region),
            "case {region:?}: an IAM ARN carries no Region, got {arn:?}"
        );
    }
}

/// Only this call enables an account: every other operation — a create, a read, a put and a
/// delete — leaves the account unenabled, and no read of the store enables it either.
#[tokio::test]
async fn no_other_operation_enables_the_account() {
    let region = "eu-west-2";
    let (router, engine) = harness();
    let now = now_seconds() as i64;
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    // One row per other operation this suite can drive end to end: a create, two reads of
    // what it wrote, a topology read, and the grouping configuration's own put and delete.
    let rows: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("CreateServiceLevelObjective", "POST", "/slo", Some(existing_slo())),
        (
            "GetServiceLevelObjective",
            "GET",
            "/slo/checkout-latency",
            None,
        ),
        ("ListServiceLevelObjectives", "POST", "/slos", Some(json!({}))),
        ("ListServices", "GET", "/services", None),
        (
            "PutGroupingConfiguration",
            "PUT",
            "/grouping-configuration",
            Some(json!({
                "GroupingAttributeDefinitions": existing_grouping_configuration()
            })),
        ),
        (
            "ListGroupingAttributeDefinitions",
            "POST",
            "/grouping-attribute-definitions",
            None,
        ),
        (
            "DeleteGroupingConfiguration",
            "DELETE",
            "/grouping-configuration",
            None,
        ),
    ];

    for (operation, method, uri, body) in rows {
        // `ListServices` binds its window to the query string; every other row addresses
        // its route as it stands.
        let uri = if operation == "ListServices" {
            format!("{uri}?{window}")
        } else {
            uri.to_string()
        };
        let (status, answer) = call(&router, region, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected HTTP 200, got {status} with {answer}"
        );
        assert!(
            recorded_enablement(&engine).is_none(),
            "case {operation:?}: expected the account to stay unenabled, got {:?}",
            recorded_enablement(&engine)
        );
    }

    // And the call that does enable it still does, on exactly the same store.
    let (status, _raw, body) = start_discovery(&router, region).await;
    assert_eq!(status, 200, "expected HTTP 200 from StartDiscovery, got {status} with {body}");
    let recorded = recorded_enablement(&engine);
    assert_eq!(
        recorded.as_ref().map(|(arn, _)| arn.as_str()),
        Some(EXPECTED_ROLE_ARN),
        "expected StartDiscovery to enable the account, got {recorded:?}"
    );
}
