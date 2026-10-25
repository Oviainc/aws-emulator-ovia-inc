// SPDX-License-Identifier: CC0-1.0

//! `BatchCreateRumMetricDefinitions`, end to end, on the TARGET the request names rather
//! than on the definitions it carries.
//!
//! The rule under test: a definition is created under ONE extended-metrics destination of
//! one app monitor, so the call stores something only when the app monitor exists, the
//! `(Destination, DestinationArn)` pair the request names is a destination that monitor
//! already holds ("You must have already created a destination for the metrics before you
//! send them", and the pairing the model states in prose: an Evidently destination is
//! named by an experiment ARN and a CloudWatch one carries none), and the policies in
//! force allow this operation's own IAM action on that monitor. A request failing any of
//! those names no place to create anything, so it is refused WHOLE — with the modelled
//! error for that failure — and leaves every destination's definitions exactly as they
//! were; the per-definition `Errors` channel is for definitions inside an otherwise
//! acceptable request, never for a target that does not exist.
//!
//! The per-definition half of the operation — which definitions a good request stores,
//! which it refuses on their own, and the ids it reports them under — is pinned by
//! `metric_definitions.rs`. The tables here span what the target rule generalises over:
//! which half of the target is missing (the monitor, the destination kind, the experiment
//! the Evidently destination names), both forbidden `Destination`/`DestinationArn`
//! pairings, and where an allowing grant comes from (identity-based or resource-based).
//! Every row drives the real wire path — a signed restJson1 request at a
//! `MultiServiceRouter` — and the state a refusal must not have touched is read back
//! through the paginated `BatchGetRumMetricDefinitions`, never out of the store.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::authorization::{
    identity_policy, resource_policy, role_arn, statement, Emulator, ACCOUNT_KEY,
};
use crate::support::{call, create, emulator, message, percent_encoded, Reply, REGION};

/// The IAM action this operation is authorized against: the service's own action prefix
/// plus the operation name, as every other operation of this service is.
const ACTION: &str = "rum:BatchCreateRumMetricDefinitions";

/// Another action of the same service, for the principal that is allowed something else.
const OTHER_ACTION: &str = "rum:GetAppMonitor";

/// The wire error code, and the status, a request IAM does not allow is refused with.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// One extended metric definition that every acceptable request below carries: an
/// extended metric name the model lists, with no dimensions, so a refusal can only come
/// from the target the request names.
/// Source: `smithy_query(query="shape", shape="MetricDefinitionRequest")`, member `Name`.
fn definition() -> Value {
    json!({"Name": "PerformanceNavigationDuration"})
}

/// The ARN of an emulated Evidently experiment — the resource an `Evidently` destination
/// names — built with the shared `engine::emulator_arn` helper.
fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// The `Destination`/`DestinationArn` query string `BatchGetRumMetricDefinitions` binds
/// the pair to (`@httpQuery("destination")` / `@httpQuery("destinationArn")`).
fn destination_query(destination: &str, destination_arn: Option<&str>) -> String {
    let mut query = format!("destination={}", percent_encoded(destination));
    if let Some(arn) = destination_arn {
        query.push_str(&format!("&destinationArn={}", percent_encoded(arn)));
    }
    query
}

/// The `BatchCreateRumMetricDefinitions` request body naming one destination and carrying
/// `definitions`.
fn create_body(destination: &str, destination_arn: Option<&str>, definitions: &[Value]) -> Value {
    let mut body = json!({
        "Destination": destination,
        "MetricDefinitions": Value::Array(definitions.to_vec()),
    });
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
    }
    body
}

/// Create one app monitor to hang destinations off.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str) {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
}

/// `PutRumMetricsDestination`, the call that must have created a destination before any
/// definition can be created under it.
async fn put_destination(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) {
    let mut body = json!({"Destination": destination});
    if let Some(arn) = destination_arn {
        body["DestinationArn"] = json!(arn);
        body["IamRoleArn"] = json!(engine::emulator_arn("iam", "", "role/rum-writer"));
    }
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: seeding the destination must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// `BatchCreateRumMetricDefinitions`: `POST /rummetrics/{AppMonitorName}/metrics`.
async fn batch_create(
    router: &MultiServiceRouter,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
    definitions: &[Value],
) -> Reply {
    call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metrics"),
        Some(create_body(destination, destination_arn, definitions)),
    )
    .await
}

/// Every definition a destination holds, read back through the paginated
/// `BatchGetRumMetricDefinitions` and following its tokens to the end.
async fn stored_definitions(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &str,
    destination_arn: Option<&str>,
) -> Vec<Value> {
    let mut reported = Vec::new();
    let mut extra = String::new();
    loop {
        let reply = call(
            router,
            "GET",
            &format!(
                "/rummetrics/{monitor}/metrics?{}{extra}",
                destination_query(destination, destination_arn)
            ),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected BatchGetRumMetricDefinitions to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        reported.extend(
            page["MetricDefinitions"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a page reported no MetricDefinitions: {page}")
                })
                .clone(),
        );
        match page["NextToken"].as_str() {
            Some(token) => extra = format!("&nextToken={}", percent_encoded(token)),
            None => return reported,
        }
    }
}

/// One row of the target table: the `(app monitor, Destination, DestinationArn)` triple a
/// request names, and the answer the service owes it.
struct TargetCase {
    case: &'static str,
    /// The app monitor the request addresses.
    monitor: &'static str,
    destination: &'static str,
    destination_arn: Option<String>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn batch_create_naming_no_stored_destination_is_refused_and_creates_nothing() {
    let (router, _engine) = emulator();

    // Two monitors: one holding both kinds of destination, one holding none — so a row
    // can miss on the monitor, on the destination kind, or on the experiment the
    // Evidently destination names.
    let stocked = "targeted";
    let bare = "targeted-bare";
    create_monitor(&router, "setup", stocked).await;
    create_monitor(&router, "setup", bare).await;
    let experiment = experiment_arn("targeted");
    put_destination(&router, "setup", stocked, "CloudWatch", None).await;
    put_destination(&router, "setup", stocked, "Evidently", Some(&experiment)).await;

    let rows: Vec<TargetCase> = vec![
        TargetCase {
            case: "an app monitor nothing created",
            monitor: "targeted-absent",
            destination: "CloudWatch",
            destination_arn: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        TargetCase {
            case: "a CloudWatch destination this monitor never had",
            monitor: bare,
            destination: "CloudWatch",
            destination_arn: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        TargetCase {
            case: "an Evidently destination this monitor never had",
            monitor: bare,
            destination: "Evidently",
            destination_arn: Some(experiment_arn("never-put")),
            status: 404,
            code: "ResourceNotFoundException",
        },
        TargetCase {
            case: "an experiment other than the one this monitor's destination names",
            monitor: stocked,
            destination: "Evidently",
            destination_arn: Some(experiment_arn("another")),
            status: 404,
            code: "ResourceNotFoundException",
        },
        TargetCase {
            case: "CloudWatch with a DestinationArn",
            monitor: stocked,
            destination: "CloudWatch",
            destination_arn: Some(experiment.clone()),
            status: 400,
            code: "ValidationException",
        },
        TargetCase {
            case: "Evidently with no DestinationArn",
            monitor: stocked,
            destination: "Evidently",
            destination_arn: None,
            status: 400,
            code: "ValidationException",
        },
    ];

    for row in &rows {
        let case = row.case;
        let reply = batch_create(
            &router,
            row.monitor,
            row.destination,
            row.destination_arn.as_deref(),
            &[definition()],
        )
        .await;
        assert_eq!(
            reply.status, row.status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            row.status,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            row.code,
            "case {case:?}: expected {}, got {:?}",
            row.code,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        // The refusal says which target it could not resolve, so the message is built
        // from the request rather than being a bare status.
        let text = message(&body).unwrap_or_default();
        assert!(
            !text.is_empty(),
            "case {case:?}: expected the refusal to carry a message, got {body}"
        );
        // Nothing of a refused call is reported as created.
        assert!(
            body["MetricDefinitions"].is_null() && body["Errors"].is_null(),
            "case {case:?}: a refused call must report no definitions, got {body}"
        );

        // Neither destination of the monitor that HAS destinations gained anything.
        for (destination, arn) in [
            ("CloudWatch", None),
            ("Evidently", Some(experiment.as_str())),
        ] {
            let held = stored_definitions(&router, case, stocked, destination, arn).await;
            assert!(
                held.is_empty(),
                "case {case:?}: expected the {destination} destination to still hold no \
                 definitions, got {held:?}"
            );
        }
    }

    // The same request against a destination the monitor DOES hold is created, so the
    // refusals above are about the target and not about the definition.
    let case = "a destination this monitor holds";
    let reply = batch_create(&router, stocked, "CloudWatch", None, &[definition()]).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.json(case);
    assert_eq!(
        answered["Errors"],
        json!([]),
        "case {case:?}: expected no per-definition errors, got {:?}",
        answered["Errors"]
    );
    let held = stored_definitions(&router, case, stocked, "CloudWatch", None).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected the destination to hold exactly the created definition, \
         got {held:?}"
    );
    assert_eq!(
        held[0]["Name"],
        definition()["Name"],
        "case {case:?}: expected the stored definition to be the one created, got {:?}",
        held[0]
    );
    // The other destination of the same monitor is untouched by a create under this one.
    let other = stored_definitions(
        &router,
        case,
        stocked,
        "Evidently",
        Some(experiment.as_str()),
    )
    .await;
    assert!(
        other.is_empty(),
        "case {case:?}: expected the Evidently destination to hold no definitions, got {other:?}"
    );
}

/// One row of the grant table: where the grant allowing the operation comes from.
struct GrantCase {
    case: &'static str,
    /// The inline identity policy the calling role holds, when it holds one.
    identity: Option<String>,
    /// The resource policy on the app monitor, when it carries one. `{role}` stands for
    /// the calling role's ARN.
    resource: Option<String>,
    allowed: bool,
}

#[tokio::test]
async fn creating_metric_definitions_is_authorized_and_a_refusal_creates_nothing() {
    let rows: Vec<GrantCase> = vec![
        GrantCase {
            case: "a role allowed another action of this service",
            identity: Some(identity_policy(&statement("Allow", OTHER_ACTION, "*"))),
            resource: None,
            allowed: false,
        },
        GrantCase {
            case: "a role with no policy at all",
            identity: None,
            resource: None,
            allowed: false,
        },
        GrantCase {
            case: "an identity policy allowing exactly this action",
            identity: Some(identity_policy(&statement("Allow", ACTION, "*"))),
            resource: None,
            allowed: true,
        },
        GrantCase {
            case: "no identity policy, a resource policy naming the caller",
            identity: None,
            resource: Some(resource_policy("Allow", "{role}", ACTION, None)),
            allowed: true,
        },
        GrantCase {
            case: "an identity policy that allows and a resource policy that denies",
            identity: Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            resource: Some(resource_policy("Deny", "*", ACTION, None)),
            allowed: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A fresh emulator per row, so no row inherits another's grant or state.
        let authorized = Emulator::new();
        let monitor = format!("metric-auth-{index}");
        authorized.create_monitor(&monitor).await;
        let seed = authorized
            .rum(
                ACCOUNT_KEY,
                "POST",
                &format!("/rummetrics/{monitor}/metricsdestination"),
                Some(json!({"Destination": "CloudWatch"})),
            )
            .await;
        assert_eq!(
            seed.status, 200,
            "case {case:?}: seeding the destination must succeed, got {} {:?}",
            seed.status,
            String::from_utf8_lossy(&seed.body)
        );

        let role = format!("metric-definer-{index}");
        let key = authorized
            .assumed_role_key(&role, row.identity.as_deref())
            .await;
        if let Some(document) = row.resource.as_deref() {
            let document = document.replace("{role}", &role_arn(&role));
            authorized.put_resource_policy(&monitor, &document).await;
        }

        let reply = authorized
            .rum(
                &key,
                "POST",
                &format!("/rummetrics/{monitor}/metrics"),
                Some(create_body("CloudWatch", None, &[definition()])),
            )
            .await;
        if row.allowed {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            assert_eq!(
                reply.status, ACCESS_DENIED_STATUS,
                "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
        }

        // What the destination holds afterwards, read back with the account's own
        // credential: exactly the definition an allowed call created, and nothing at all
        // after a refused one.
        let read = authorized
            .rum(
                ACCOUNT_KEY,
                "GET",
                &format!(
                    "/rummetrics/{monitor}/metrics?{}",
                    destination_query("CloudWatch", None)
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the read-back to answer 200, got {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let held = read.json(case)["MetricDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected = usize::from(row.allowed);
        assert_eq!(
            held.len(),
            expected,
            "case {case:?}: expected the destination to hold {expected} definition(s), got \
             {held:?}"
        );
    }
}
