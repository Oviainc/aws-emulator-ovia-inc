// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Amazon DocumentDB cluster parameter groups.
//!
//! Every test drives the service over its real wire protocol (awsQuery form bodies in, XML
//! out) through the same [`MultiServiceRouter`] the production router uses, so a response
//! that is not computed from persisted state cannot pass: a create is read back through a
//! describe, a modify through the describe that must report the new value, and a delete
//! through the describe that must no longer find the group.
//!
//! The rules are asserted over TABLES spanning the class each rule generalises over, not
//! over one recorded request:
//!
//! * the value rule is driven by the published catalog itself
//!   (`state::DOCDB_CLUSTER_PARAMETERS`), every entry of it, with each enumerated value,
//!   each published range bound and one step past each bound;
//! * the pending-change rule is asserted over both apply types (static and dynamic) and
//!   both apply methods;
//! * the not-found, already-exists and in-use refusals are asserted per operation.

use std::sync::Arc;

use service_com_amazonaws_docdb_amazonrdsv19::state;
use service_com_amazonaws_docdb_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The form encoder and the XML reader this suite drives the wire with are the sibling
// suite's: one copy per test target, not one per suite.
use crate::cluster_lifecycle::{count, form_encode, opt_text, text, texts};

/// The region every request below signs for; the ARNs the service reports must sit in it.
const REGION: &str = "us-east-1";

/// The awsQuery API version an SDK sends with every DocumentDB request.
const API_VERSION: &str = "2014-10-31";

/// A DocumentDB cluster parameter group family, which publishes a parameter catalog.
const DOCDB_FAMILY: &str = "docdb5.0";

/// One emulated DocumentDB behind a router, driven over the wire.
struct Docdb {
    router: MultiServiceRouter,
}

/// What the service answered: the HTTP status, the `content-type` and the XML body.
struct Reply {
    status: u16,
    content_type: String,
    xml: String,
}

impl Docdb {
    /// A router carrying only this service, over one shared engine — exactly what
    /// `core/router` builds, minus the other services.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4 verification.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `action` with the given query parameters and return the raw reply.
    async fn call(&self, action: &str, params: &[(&str, &str)]) -> Reply {
        let mut body = format!("Action={action}&Version={API_VERSION}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://rds.us-east-1.amazonaws.com/")
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            // The routing key every front door reads: the SigV4 credential scope. It names
            // `docdb`, the key that names THIS service (`ROUTE_KEY` in `src/service.rs`):
            // the `rds` signing name all three `AmazonRDSv19` models share belongs to
            // Amazon RDS, which serves an SDK client's traffic.
            .header(
                http::header::AUTHORIZATION,
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDDOCDBTESTKEY/20260101/{REGION}/docdb/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        Reply {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            xml: String::from_utf8_lossy(response.body()).into_owned(),
        }
    }

    /// Call `action` and require a 200 in the XML envelope awsQuery answers with.
    async fn ok(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let reply = self.call(action, params).await;
        assert_eq!(
            (reply.status, reply.content_type.as_str()),
            (200, "text/xml"),
            "case {case:?}: expected {action} to answer 200 text/xml, got {} {:?} with {}",
            reply.status,
            reply.content_type,
            reply.xml
        );
        reply.xml
    }

    /// Call `action` and require the modelled refusal: the status and `<Code>` named, in
    /// the awsQuery `<ErrorResponse>` envelope with a `Sender` fault.
    async fn refused(
        &self,
        case: &str,
        action: &str,
        params: &[(&str, &str)],
        status: u16,
        code: &str,
    ) -> String {
        let reply = self.call(action, params).await;
        assert_eq!(
            (
                reply.status,
                reply.content_type.as_str(),
                error_code(&reply.xml).as_str()
            ),
            (status, "text/xml", code),
            "case {case:?}: expected {action} to answer {status} text/xml {code}, got {} {:?} \
             with {}",
            reply.status,
            reply.content_type,
            reply.xml
        );
        assert!(
            reply.xml.contains("<ErrorResponse") && reply.xml.contains("<Type>Sender</Type>"),
            "case {case:?}: expected an awsQuery ErrorResponse with a Sender fault, got {}",
            reply.xml
        );
        reply.xml
    }

    /// Create a group and return its (lowercased) name.
    async fn create_group(&self, case: &str, name: &str, family: &str) -> String {
        self.ok(
            case,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", name),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await;
        name.to_ascii_lowercase()
    }
}

/// The awsQuery error envelope's `<Code>`, or the empty string on a success.
fn error_code(xml: &str) -> String {
    opt_text(xml, "Code").unwrap_or_default()
}

/// Wait out the window in which the clock still owes a new cluster its `available` status:
/// the sibling suite's helper, not a second copy of it.
use crate::cluster_lifecycle::let_the_clock_catch_up;

// ---------------------------------------------------------------------------
// The lifecycle the issue describes, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_cluster_parameter_group_lifecycle_is_computed_from_persisted_state() {
    let docdb = Docdb::new();

    // 1. CreateDBClusterParameterGroup stores the group and answers with it.
    let created = docdb
        .ok(
            "create",
            "CreateDBClusterParameterGroup",
            &[
                // A mixed-case name: `DBClusterParameterGroupName` is "stored as a
                // lowercase string", so every later read reports it lowercased.
                ("DBClusterParameterGroupName", "Probe-CPG"),
                ("DBParameterGroupFamily", DOCDB_FAMILY),
                ("Description", "probe"),
            ],
        )
        .await;
    assert!(
        created.contains("<CreateDBClusterParameterGroupResponse"),
        "case \"create\": expected the awsQuery CreateDBClusterParameterGroupResponse \
         envelope, got {created}"
    );
    let expected_arn = format!(
        "arn:aws:rds:{REGION}:{}:cluster-pg:probe-cpg",
        engine::emulator_account_id()
    );
    assert_eq!(
        (
            text(&created, "DBClusterParameterGroupName"),
            text(&created, "DBParameterGroupFamily"),
            text(&created, "Description"),
            text(&created, "DBClusterParameterGroupArn"),
        ),
        (
            "probe-cpg".to_string(),
            DOCDB_FAMILY.to_string(),
            "probe".to_string(),
            expected_arn.clone(),
        ),
        "case \"create\": the group AWS answers with must be the one the request asked for, \
         got {created}"
    );

    // 2. ...and it reads back through DescribeDBClusterParameterGroups, by the lowercased
    //    name and in the unfiltered listing, which also reports the engine's own
    //    `default.<family>` groups (AWS provides one per family without a create).
    for (case, params) in [
        (
            "describe by name",
            vec![("DBClusterParameterGroupName", "probe-cpg")],
        ),
        (
            "describe by the name as sent, mixed case",
            vec![("DBClusterParameterGroupName", "Probe-CPG")],
        ),
    ] {
        let listed = docdb
            .ok(case, "DescribeDBClusterParameterGroups", &params)
            .await;
        assert_eq!(
            (
                count(&listed, "DBClusterParameterGroup"),
                text(&listed, "DBClusterParameterGroupArn"),
            ),
            (1, expected_arn.clone()),
            "case {case:?}: expected exactly the group just created, got {listed}"
        );
    }
    let listed = docdb
        .ok("describe every group", "DescribeDBClusterParameterGroups", &[])
        .await;
    assert!(
        texts(&listed, "DBClusterParameterGroupArn").contains(&expected_arn),
        "case \"describe every group\": the created group must be in the listing beside the \
         engine's defaults, got {listed}"
    );

    // 3. A freshly created group carries no parameters of its own: the defaults belong to
    //    the family, so `Parameters` is empty.
    let fresh = docdb
        .ok(
            "describe parameters of a fresh group",
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", "probe-cpg")],
        )
        .await;
    assert_eq!(
        count(&fresh, "Parameter"),
        0,
        "case \"describe parameters of a fresh group\": a group carries no parameter until \
         one is set, got {fresh}"
    );

    // 4. ModifyDBClusterParameterGroup sets one and echoes the group name...
    let modified = docdb
        .ok(
            "modify",
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "disabled"),
                (
                    "Parameters.Parameter.1.ApplyMethod",
                    state::APPLY_METHOD_PENDING_REBOOT,
                ),
            ],
        )
        .await;
    assert_eq!(
        text(&modified, "DBClusterParameterGroupName"),
        "probe-cpg",
        "case \"modify\": the answer must echo the group it wrote, got {modified}"
    );

    // 5. ...and the describe reports the value, with the metadata the family publishes.
    let after = docdb
        .ok(
            "describe parameters after a modify",
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", "probe-cpg")],
        )
        .await;
    let tls = state::published_parameter(DOCDB_FAMILY, "tls").expect("the family publishes tls");
    assert_eq!(
        (
            count(&after, "Parameter"),
            text(&after, "ParameterName"),
            text(&after, "ParameterValue"),
            text(&after, "Source"),
            text(&after, "ApplyMethod"),
            text(&after, "AllowedValues"),
        ),
        (
            1,
            "tls".to_string(),
            "disabled".to_string(),
            state::PARAMETER_SOURCE_USER.to_string(),
            state::APPLY_METHOD_PENDING_REBOOT.to_string(),
            tls.allowed_values.to_string(),
        ),
        "case \"describe parameters after a modify\": expected the parameter just set, got \
         {after}"
    );

    // 6. A second create under the same name is the modelled already-exists refusal.
    docdb
        .refused(
            "create the same name twice",
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("DBParameterGroupFamily", DOCDB_FAMILY),
                ("Description", "probe"),
            ],
            400,
            "DBParameterGroupAlreadyExists",
        )
        .await;

    // 7. The group is waiting on a reboot for a static parameter, so it cannot be reset —
    //    and the message names what it is pending on.
    let refused = docdb
        .refused(
            "reset while a change is pending",
            "ResetDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "probe-cpg"),
                ("ResetAllParameters", "true"),
            ],
            400,
            "InvalidDBParameterGroupState",
        )
        .await;
    assert!(
        text(&refused, "Message").contains("tls"),
        "case \"reset while a change is pending\": the refusal must name the pending \
         parameter, got {refused}"
    );

    // 8. Both describes answer the modelled 404 for a group that is not there.
    for (case, action, params) in [
        (
            "describe groups for a missing group",
            "DescribeDBClusterParameterGroups",
            vec![("DBClusterParameterGroupName", "no-such-group")],
        ),
        (
            "describe parameters for a missing group",
            "DescribeDBClusterParameters",
            vec![("DBClusterParameterGroupName", "no-such-group")],
        ),
        (
            "modify a missing group",
            "ModifyDBClusterParameterGroup",
            vec![
                ("DBClusterParameterGroupName", "no-such-group"),
                ("Parameters.Parameter.1.ParameterName", "tls"),
                ("Parameters.Parameter.1.ParameterValue", "enabled"),
            ],
        ),
        (
            "reset a missing group",
            "ResetDBClusterParameterGroup",
            vec![
                ("DBClusterParameterGroupName", "no-such-group"),
                ("ResetAllParameters", "true"),
            ],
        ),
        (
            "delete a missing group",
            "DeleteDBClusterParameterGroup",
            vec![("DBClusterParameterGroupName", "no-such-group")],
        ),
    ] {
        docdb
            .refused(case, action, &params, 404, "DBParameterGroupNotFound")
            .await;
    }

    // 9. DeleteDBClusterParameterGroup removes it, and the describe no longer finds it.
    let deleted = docdb
        .ok(
            "delete",
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", "probe-cpg")],
        )
        .await;
    assert!(
        deleted.contains("<DeleteDBClusterParameterGroupResponse"),
        "case \"delete\": expected the awsQuery DeleteDBClusterParameterGroupResponse \
         envelope, got {deleted}"
    );
    docdb
        .refused(
            "describe the deleted group",
            "DescribeDBClusterParameterGroups",
            &[("DBClusterParameterGroupName", "probe-cpg")],
            404,
            "DBParameterGroupNotFound",
        )
        .await;
}

// ---------------------------------------------------------------------------
// The value rule, over the whole published catalog
// ---------------------------------------------------------------------------

/// A value a parameter admits, and one it does not, derived from the parameter's OWN
/// published `AllowedValues` — an enumeration is split, a range is taken at each published
/// bound and one step past it.
fn admitted_and_refused(spec: &state::ClusterParameterSpec) -> (Vec<String>, Vec<String>) {
    if spec.allowed_values.contains(',') {
        let admitted: Vec<String> = spec
            .allowed_values
            .split(',')
            .map(|value| value.trim().to_string())
            .collect();
        return (
            admitted,
            vec!["not-a-published-value".to_string(), String::new()],
        );
    }
    let (low, high) = spec
        .allowed_values
        .split_once('-')
        .expect("a non-enumerated AllowedValues is a <low>-<high> range");
    match spec.data_type {
        "integer" => {
            let low: i64 = low.parse().expect("an integer range bound");
            let high: i64 = high.parse().expect("an integer range bound");
            (
                vec![low.to_string(), high.to_string()],
                vec![
                    (low - 1).to_string(),
                    (high + 1).to_string(),
                    "not-a-number".to_string(),
                ],
            )
        }
        "float" => {
            let low: f64 = low.parse().expect("a float range bound");
            let high: f64 = high.parse().expect("a float range bound");
            (
                vec![low.to_string(), high.to_string()],
                vec![
                    (low - 1.0).to_string(),
                    (high + 1.0).to_string(),
                    "not-a-number".to_string(),
                ],
            )
        }
        other => panic!("unhandled DataType {other:?} for parameter {}", spec.name),
    }
}

/// Every parameter the DocumentDB families publish admits exactly the values its
/// `AllowedValues` names, and refuses anything else with `InvalidParameterValue` — a
/// message naming the parameter and the values it admits.
///
/// The table is the published catalog itself, so it covers the parameters the issue never
/// mentioned (`profiler`, `ttl_monitor`, `change_stream_log_retention_duration`,
/// `profiler_sampling_rate`, `profiler_threshold_ms`, `audit_logs`) as well as the one it
/// did (`tls`), and every range is exercised at both published bounds and one step past
/// each.
#[tokio::test]
async fn a_parameter_value_outside_the_published_set_is_refused() {
    let docdb = Docdb::new();
    let group = docdb.create_group("value rule", "value-rule-cpg", DOCDB_FAMILY).await;

    for spec in state::DOCDB_CLUSTER_PARAMETERS {
        let (admitted, refused) = admitted_and_refused(spec);
        for value in &admitted {
            let case = format!("{} = {value:?} (admitted)", spec.name);
            let answered = docdb
                .ok(
                    &case,
                    "ModifyDBClusterParameterGroup",
                    &[
                        ("DBClusterParameterGroupName", &group),
                        ("Parameters.Parameter.1.ParameterName", spec.name),
                        ("Parameters.Parameter.1.ParameterValue", value),
                    ],
                )
                .await;
            assert_eq!(
                text(&answered, "DBClusterParameterGroupName"),
                group,
                "case {case:?}: expected the group name echoed, got {answered}"
            );
            // The write is readable: the group now carries exactly this value.
            let reported = docdb
                .ok(
                    &case,
                    "DescribeDBClusterParameters",
                    &[("DBClusterParameterGroupName", &group)],
                )
                .await;
            let values = texts(&reported, "ParameterValue");
            assert!(
                values.contains(value),
                "case {case:?}: expected the stored value to be reported, got {values:?}"
            );
        }
        for value in &refused {
            let case = format!("{} = {value:?} (refused)", spec.name);
            let answered = docdb
                .refused(
                    &case,
                    "ModifyDBClusterParameterGroup",
                    &[
                        ("DBClusterParameterGroupName", &group),
                        ("Parameters.Parameter.1.ParameterName", spec.name),
                        ("Parameters.Parameter.1.ParameterValue", value),
                    ],
                    400,
                    "InvalidParameterValue",
                )
                .await;
            let message = text(&answered, "Message");
            assert!(
                message.contains(spec.name) && message.contains(spec.allowed_values),
                "case {case:?}: the refusal must name the parameter and its allowed values, \
                 got {message:?}"
            );
        }
    }

    // A parameter name the family publishes nothing for is refused the same way, naming
    // the parameters it does publish.
    let answered = docdb
        .refused(
            "a parameter the family does not publish",
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", &group),
                ("Parameters.Parameter.1.ParameterName", "no_such_parameter"),
                ("Parameters.Parameter.1.ParameterValue", "enabled"),
            ],
            400,
            "InvalidParameterValue",
        )
        .await;
    assert!(
        text(&answered, "Message").contains("no_such_parameter"),
        "the refusal must name the parameter the request asked for, got {answered}"
    );
}

/// A family this emulator publishes no catalog for — the one `rds` front door also serves
/// the Amazon RDS and Neptune engine families — carries whatever the caller sets, because
/// refusing a value there would invent a constraint. The value is still read back from
/// state, without the metadata only a published parameter has.
#[tokio::test]
async fn a_family_with_no_published_catalog_carries_any_parameter() {
    let docdb = Docdb::new();
    // (case, family, publishes a catalog)
    let rows: [(&str, &str, bool); 3] = [
        ("a docdb family", "docdb4.0", true),
        ("an aurora family", "aurora-postgresql15", false),
        ("a neptune family", "neptune1.3", false),
    ];

    for (case, family, publishes) in rows {
        assert_eq!(
            state::published_parameters(family).is_some(),
            publishes,
            "case {case:?}: expected published_parameters({family:?}).is_some() == {publishes}"
        );
        let group = docdb
            .create_group(case, &format!("catalog-{}", family.replace('.', "-")), family)
            .await;
        // `tls` is published only by the docdb families; for the others the name is one the
        // emulator knows nothing about, which is exactly the point.
        let name = if publishes { "tls" } else { "max_connections" };
        let value = if publishes { "enabled" } else { "4096" };
        docdb
            .ok(
                case,
                "ModifyDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", &group),
                    ("Parameters.Parameter.1.ParameterName", name),
                    ("Parameters.Parameter.1.ParameterValue", value),
                ],
            )
            .await;
        let reported = docdb
            .ok(
                case,
                "DescribeDBClusterParameters",
                &[("DBClusterParameterGroupName", &group)],
            )
            .await;
        assert_eq!(
            (
                texts(&reported, "ParameterName"),
                texts(&reported, "ParameterValue"),
                opt_text(&reported, "AllowedValues").is_some(),
            ),
            (vec![name.to_string()], vec![value.to_string()], publishes),
            "case {case:?}: expected the stored parameter reported, with published metadata \
             only for a family that has a catalog, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pending changes, and what they refuse
// ---------------------------------------------------------------------------

/// A change the engine cannot apply without a reboot leaves the group pending, and a
/// pending group cannot be reset; a change applied immediately leaves nothing pending, so
/// the reset goes through and the value is gone from state afterwards.
///
/// The rows span both apply types the catalog publishes and both apply methods the model's
/// `ApplyMethod` enum names, plus the absent method.
#[tokio::test]
async fn a_change_waiting_on_a_reboot_blocks_a_reset() {
    // (case, parameter, value, apply method, resettable)
    let rows: [(&str, &str, &str, Option<&str>, bool); 6] = [
        // `tls` is static: "changes to static parameters require a reboot", so the change
        // is pending whatever the caller asks for.
        ("a static parameter, pending-reboot", "tls", "disabled", Some("pending-reboot"), false),
        ("a static parameter, immediate", "tls", "disabled", Some("immediate"), false),
        // `profiler` is dynamic: "changes to dynamic parameters are applied immediately".
        ("a dynamic parameter, immediate", "profiler", "enabled", Some("immediate"), true),
        ("a dynamic parameter, pending-reboot", "profiler", "enabled", Some("pending-reboot"), false),
        // With no method asked for, the change gets the method its apply type implies: a
        // dynamic parameter is applied immediately, so nothing is left pending and the
        // reset goes through.
        ("a dynamic parameter, no method", "profiler", "enabled", None, true),
        // A static parameter with no method asked for still waits for the reboot.
        ("a static parameter, no method", "tls", "disabled", None, false),
    ];

    for (case, parameter, value, apply_method, resettable) in rows {
        // One group per row: a reset is a state change, so the rows must not see each
        // other's.
        let docdb = Docdb::new();
        let group = docdb.create_group(case, "pending-cpg", DOCDB_FAMILY).await;
        let mut params = vec![
            ("DBClusterParameterGroupName", group.as_str()),
            ("Parameters.Parameter.1.ParameterName", parameter),
            ("Parameters.Parameter.1.ParameterValue", value),
        ];
        if let Some(method) = apply_method {
            params.push(("Parameters.Parameter.1.ApplyMethod", method));
        }
        docdb
            .ok(case, "ModifyDBClusterParameterGroup", &params)
            .await;

        if resettable {
            let reset = docdb
                .ok(
                    case,
                    "ResetDBClusterParameterGroup",
                    &[
                        ("DBClusterParameterGroupName", &group),
                        ("Parameters.Parameter.1.ParameterName", parameter),
                    ],
                )
                .await;
            assert_eq!(
                text(&reset, "DBClusterParameterGroupName"),
                group,
                "case {case:?}: the reset must echo the group it cleared, got {reset}"
            );
            let reported = docdb
                .ok(
                    case,
                    "DescribeDBClusterParameters",
                    &[("DBClusterParameterGroupName", &group)],
                )
                .await;
            assert_eq!(
                count(&reported, "Parameter"),
                0,
                "case {case:?}: the reset parameter must be gone from state, got {reported}"
            );
        } else {
            docdb
                .refused(
                    case,
                    "ResetDBClusterParameterGroup",
                    &[
                        ("DBClusterParameterGroupName", &group),
                        ("ResetAllParameters", "true"),
                    ],
                    400,
                    "InvalidDBParameterGroupState",
                )
                .await;
            // The refusal changed nothing: the value is still there.
            let reported = docdb
                .ok(
                    case,
                    "DescribeDBClusterParameters",
                    &[("DBClusterParameterGroupName", &group)],
                )
                .await;
            assert_eq!(
                texts(&reported, "ParameterValue"),
                vec![value.to_string()],
                "case {case:?}: a refused reset must leave the group as it was, got {reported}"
            );
        }
    }
}

/// `ResetAllParameters` and a list of `Parameters` cannot be used together, and a reset of
/// a parameter the family does not publish is refused.
#[tokio::test]
async fn a_reset_refuses_a_combination_the_api_forbids() {
    let docdb = Docdb::new();
    let group = docdb.create_group("reset guards", "reset-cpg", DOCDB_FAMILY).await;

    let both = docdb
        .call(
            "ResetDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", &group),
                ("ResetAllParameters", "true"),
                ("Parameters.Parameter.1.ParameterName", "profiler"),
            ],
        )
        .await;
    assert_eq!(
        (both.status, both.content_type.as_str(), error_code(&both.xml)),
        (400, "text/xml", "ValidationException".to_string()),
        "ResetAllParameters together with a Parameters list must be refused, got {} {}",
        both.status,
        both.xml
    );

    docdb
        .refused(
            "reset a parameter the family does not publish",
            "ResetDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", &group),
                ("Parameters.Parameter.1.ParameterName", "no_such_parameter"),
            ],
            400,
            "InvalidParameterValue",
        )
        .await;
}

// ---------------------------------------------------------------------------
// What a describe reports, by source
// ---------------------------------------------------------------------------

/// `Source` selects what `DescribeDBClusterParameters` reports: the group's own parameters
/// (`customer`/`user`, and the default when none is named), the family's published defaults
/// (`engine`/`engine-default`), or nothing (`service`/`system`, which this emulator sets no
/// parameter under, and any value naming no source at all — a `Source` is a filter, so it
/// narrows the answer instead of refusing the request).
#[tokio::test]
async fn the_source_selects_which_parameters_a_describe_reports() {
    let docdb = Docdb::new();
    let group = docdb.create_group("sources", "source-cpg", DOCDB_FAMILY).await;
    docdb
        .ok(
            "sources",
            "ModifyDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", &group),
                ("Parameters.Parameter.1.ParameterName", "ttl_monitor"),
                ("Parameters.Parameter.1.ParameterValue", "disabled"),
            ],
        )
        .await;

    let published = state::published_parameters(DOCDB_FAMILY)
        .expect("a docdb family publishes a catalog")
        .len();
    // (case, Source, parameters expected, the Source each carries). Both names of each
    // source are rows: the request vocabulary the model documents (`customer`, `engine`,
    // `service`) and the one a response reports (`user`, `engine-default`, `system`).
    let rows: [(&str, Option<&str>, usize, &str); 7] = [
        ("no source named", None, 1, state::PARAMETER_SOURCE_USER),
        (
            "the customer's own parameters",
            Some("customer"),
            1,
            state::PARAMETER_SOURCE_USER,
        ),
        (
            "the same source by its reported name",
            Some(state::PARAMETER_SOURCE_USER),
            1,
            state::PARAMETER_SOURCE_USER,
        ),
        (
            "the engine's published defaults",
            Some("engine"),
            published,
            state::PARAMETER_SOURCE_ENGINE_DEFAULT,
        ),
        (
            "the same source by its reported name",
            Some(state::PARAMETER_SOURCE_ENGINE_DEFAULT),
            published,
            state::PARAMETER_SOURCE_ENGINE_DEFAULT,
        ),
        (
            "the service's own parameters",
            Some("service"),
            0,
            state::PARAMETER_SOURCE_SYSTEM,
        ),
        (
            "a value naming no source this API knows",
            Some("not-a-source"),
            0,
            state::PARAMETER_SOURCE_SYSTEM,
        ),
    ];

    for (case, source, expected, source_reported) in rows {
        let mut params = vec![("DBClusterParameterGroupName", group.as_str())];
        if let Some(source) = source {
            params.push(("Source", source));
        }
        let reported = docdb
            .ok(case, "DescribeDBClusterParameters", &params)
            .await;
        assert_eq!(
            count(&reported, "Parameter"),
            expected,
            "case {case:?} (Source={source:?}): expected {expected} parameter(s), got {reported}"
        );
        if expected > 0 {
            let sources = texts(&reported, "Source");
            assert!(
                sources.iter().all(|value| value == source_reported),
                "case {case:?}: every parameter must carry Source {source_reported:?}, got \
                 {sources:?}"
            );
        }
    }

    // A `Source` cannot turn a missing group's 404 into anything else: the group is looked
    // up whatever the filter says.
    docdb
        .refused(
            "a source on a group that is not there",
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", "no-such-group"),
                ("Source", state::PARAMETER_SOURCE_USER),
            ],
            404,
            "DBParameterGroupNotFound",
        )
        .await;

    // `Filters` is "not currently supported": a request that carries one is REFUSED by both
    // describes rather than served as though the filter had been applied.
    for (case, action) in [
        ("filters on the group describe", "DescribeDBClusterParameterGroups"),
        ("filters on the parameter describe", "DescribeDBClusterParameters"),
    ] {
        docdb
            .refused(
                case,
                action,
                &[
                    ("DBClusterParameterGroupName", &group),
                    ("Filters.Filter.1.Name", "a-filter-the-api-ignores"),
                    ("Filters.Filter.1.Values.Value.1", "anything"),
                ],
                400,
                "ValidationException",
            )
            .await;
    }

    // One of the engine's OWN `default.<family>` groups exists without a create, so a
    // describe of its parameters reports the family's published defaults with `Source`
    // `engine-default` rather than the 404 a group that is not there gets, and a delete of
    // it is refused with the modelled state error rather than reported absent.
    let default_group = format!(
        "{}{DOCDB_FAMILY}",
        state::DEFAULT_PARAMETER_GROUP_PREFIX
    );
    let reported = docdb
        .ok(
            "the engine's default group",
            "DescribeDBClusterParameters",
            &[("DBClusterParameterGroupName", &default_group)],
        )
        .await;
    let published = state::published_parameters(DOCDB_FAMILY)
        .expect("a docdb family publishes a catalog")
        .len();
    assert_eq!(
        count(&reported, "Parameter"),
        published,
        "case \"the engine's default group\": expected the family's {published} published \
         parameters, got {reported}"
    );
    assert!(
        texts(&reported, "Source")
            .iter()
            .all(|value| value == state::PARAMETER_SOURCE_ENGINE_DEFAULT),
        "case \"the engine's default group\": a default group's parameters are the engine's, \
         got {reported}"
    );
    docdb
        .refused(
            "deleting the engine's default group",
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", &default_group)],
            400,
            "InvalidDBParameterGroupState",
        )
        .await;
}

// ---------------------------------------------------------------------------
// What a create admits as a name and a family
// ---------------------------------------------------------------------------

/// A `DBClusterParameterGroupName` must satisfy the rule the model states for it
/// (`DBClusterParameterGroupNameMessage$DBClusterParameterGroupName`: "from 1 to 255 letters
/// or numbers. The first character must be a letter. Cannot end with a hyphen or contain
/// two consecutive hyphens"), and a `docdb`-prefixed `DBParameterGroupFamily` must name one
/// of the DocumentDB families; a family of another engine on the shared `rds` front door is
/// accepted, because this service publishes no catalog to refuse it against.
///
/// The rows span every clause of both rules, each published bound and one step past it.
#[tokio::test]
async fn a_create_refuses_a_name_or_a_family_the_service_does_not_admit() {
    let docdb = Docdb::new();
    let longest = format!("a{}", "b".repeat(state::MAX_PARAMETER_GROUP_NAME_LEN - 1));
    let too_long = format!("a{}", "b".repeat(state::MAX_PARAMETER_GROUP_NAME_LEN));

    // (case, name, family, admitted)
    let rows: [(&str, &str, &str, bool); 14] = [
        ("letters, digits and hyphens", "probe-cpg-1", "docdb5.0", true),
        ("the longest admitted name", &longest, "docdb5.0", true),
        ("one character past the bound", &too_long, "docdb5.0", false),
        ("an empty name", "", "docdb5.0", false),
        ("a name starting with a digit", "1-probe", "docdb5.0", false),
        ("a name starting with a hyphen", "-probe", "docdb5.0", false),
        ("a name ending with a hyphen", "probe-", "docdb5.0", false),
        ("two consecutive hyphens", "pro--be", "docdb5.0", false),
        // A `default.<family>` group's name carries a `.`, which the rule does not admit.
        ("a default group's name", "default.docdb5.0", "docdb5.0", false),
        ("an underscore", "probe_cpg", "docdb5.0", false),
        // Every family DocumentDB publishes is admitted...
        ("the 3.6 family", "probe-cpg-36", "docdb3.6", true),
        ("the 4.0 family", "probe-cpg-40", "docdb4.0", true),
        // ...and a `docdb` family it does not publish is not.
        ("a docdb family that does not exist", "probe-cpg-99", "docdb9.9", false),
        // Another engine's family on the same front door is accepted.
        ("another engine's family", "probe-cpg-aurora", "aurora-mysql8.0", true),
    ];

    for (case, name, family, admitted) in rows {
        let params = [
            ("DBClusterParameterGroupName", name),
            ("DBParameterGroupFamily", family),
            ("Description", "a group under test"),
        ];
        if admitted {
            let created = docdb
                .ok(case, "CreateDBClusterParameterGroup", &params)
                .await;
            assert_eq!(
                text(&created, "DBParameterGroupFamily"),
                family,
                "case {case:?}: the family the request named must be the one stored, got \
                 {created}"
            );
        } else {
            docdb
                .refused(
                    case,
                    "CreateDBClusterParameterGroup",
                    &params,
                    400,
                    "InvalidParameterValue",
                )
                .await;
            // A refused create stored nothing: the name is not there to describe — unless
            // it is one of the engine's OWN `default.<family>` groups, which exist in every
            // account without a create and are reported whether or not this call was made.
            if !name.starts_with("default.") {
                docdb
                    .refused(
                        case,
                        "DescribeDBClusterParameterGroups",
                        &[("DBClusterParameterGroupName", name)],
                        404,
                        "DBParameterGroupNotFound",
                    )
                    .await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A group a cluster carries
// ---------------------------------------------------------------------------

/// "The cluster parameter group to be deleted can't be associated with any clusters": while
/// a cluster carries the group the delete is refused, and once that cluster is gone the
/// delete goes through.
#[tokio::test]
async fn a_group_a_cluster_carries_cannot_be_deleted() {
    let docdb = Docdb::new();
    let group = docdb.create_group("in use", "in-use-cpg", DOCDB_FAMILY).await;
    docdb
        .ok(
            "in use",
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "carrier-cluster"),
                ("Engine", "docdb"),
                ("MasterUsername", "docdbadmin"),
                ("MasterUserPassword", "docdbpassword"),
                ("DBClusterParameterGroupName", &group),
            ],
        )
        .await;

    let refused = docdb
        .refused(
            "delete a group a cluster carries",
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", &group)],
            400,
            "InvalidDBParameterGroupState",
        )
        .await;
    assert!(
        text(&refused, "Message").contains("carrier-cluster"),
        "the refusal must name the cluster that carries the group, got {refused}"
    );

    // The cluster has to finish provisioning before it can be deleted; the clock owes
    // that transition, and nothing sleeps for it inside the emulator.
    let_the_clock_catch_up();
    docdb
        .ok(
            "in use",
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", "carrier-cluster"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await;
    docdb
        .ok(
            "delete the group once no cluster carries it",
            "DeleteDBClusterParameterGroup",
            &[("DBClusterParameterGroupName", &group)],
        )
        .await;
}

// ---------------------------------------------------------------------------
// The metadata a reported parameter carries
// ---------------------------------------------------------------------------

/// Every `Parameter` a describe reports carries the metadata the group's FAMILY publishes
/// for that parameter — `Description`, `AllowedValues`, `ApplyType`, `DataType`,
/// `IsModifiable` and `MinimumEngineVersion` — beside the value and the apply method the
/// group itself holds, with `Source` naming who set the value: `user` for the caller's own
/// value, `engine-default` for the family's published one.
///
/// The table IS the published catalog (`state::DOCDB_CLUSTER_PARAMETERS`), so every entry is
/// a row rather than only the one a recorded request happened to set: one case per entry for
/// the group's own projection, and one case over the whole catalog, in catalog order, for the
/// engine-default projection. A row's value is the entry's own published default, which its
/// `AllowedValues` admits whatever the entry's data type is.
#[tokio::test]
async fn a_reported_parameter_carries_the_metadata_its_family_publishes() {
    let docdb = Docdb::new();
    let published =
        state::published_parameters(DOCDB_FAMILY).expect("a docdb family publishes a catalog");

    for (row, spec) in published.iter().enumerate() {
        let case = spec.name;
        // One group per row: a change to a static parameter stays pending, which is exactly
        // what blocks the reset that would otherwise clear the previous row.
        let group = docdb
            .create_group(case, &format!("metadata-cpg-{row}"), DOCDB_FAMILY)
            .await;
        docdb
            .ok(
                case,
                "ModifyDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", &group),
                    ("Parameters.Parameter.1.ParameterName", spec.name),
                    ("Parameters.Parameter.1.ParameterValue", spec.default_value),
                ],
            )
            .await;
        let reported = docdb
            .ok(
                case,
                "DescribeDBClusterParameters",
                &[("DBClusterParameterGroupName", &group)],
            )
            .await;
        assert_eq!(
            count(&reported, "Parameter"),
            1,
            "case {case:?}: the group carries exactly the one parameter the modify set, got \
             {reported}"
        );
        // (element, the text the catalog publishes for this row)
        let elements: [(&str, String); 9] = [
            ("ParameterName", spec.name.to_string()),
            ("ParameterValue", spec.default_value.to_string()),
            ("Description", spec.description.to_string()),
            ("AllowedValues", spec.allowed_values.to_string()),
            ("ApplyType", spec.apply_type.to_string()),
            ("DataType", spec.data_type.to_string()),
            ("IsModifiable", spec.is_modifiable.to_string()),
            (
                "MinimumEngineVersion",
                // The earliest version the parameter can apply at inside THIS group's
                // family, which is what the member answers about (see
                // `service_common::rds_cluster_parameters::minimum_engine_version_in_family`)
                // — not the version the parameter first appeared in anywhere.
                state::minimum_engine_version_in_family(DOCDB_FAMILY, spec),
            ),
            ("Source", state::PARAMETER_SOURCE_USER.to_string()),
        ];
        for (element, want) in elements {
            let got = opt_text(&reported, element);
            assert_eq!(
                got.as_deref(),
                Some(want.as_str()),
                "case {case:?}: expected <{element}> {want:?}, got {got:?}"
            );
        }
        // With no `ApplyMethod` asked for, the change gets the one its apply type implies.
        let want_method = if spec.apply_type == state::APPLY_TYPE_DYNAMIC {
            state::APPLY_METHOD_IMMEDIATE
        } else {
            state::APPLY_METHOD_PENDING_REBOOT
        };
        let got = opt_text(&reported, "ApplyMethod");
        assert_eq!(
            got.as_deref(),
            Some(want_method),
            "case {case:?}: a change with no method asked for takes the one its {:?} apply \
             type implies ({want_method:?}), got {got:?}",
            spec.apply_type
        );
    }

    // The engine-default projection: the family's whole catalog, in catalog order, with every
    // metadata element reported for every entry and the entry's own default as the value.
    let case = "the family's published defaults";
    let group = docdb
        .create_group(case, "metadata-defaults-cpg", DOCDB_FAMILY)
        .await;
    let reported = docdb
        .ok(
            case,
            "DescribeDBClusterParameters",
            &[
                ("DBClusterParameterGroupName", &group),
                ("Source", "engine"),
            ],
        )
        .await;
    let columns: [(&str, Vec<String>); 9] = [
        (
            "ParameterName",
            published.iter().map(|spec| spec.name.to_string()).collect(),
        ),
        (
            "ParameterValue",
            published
                .iter()
                .map(|spec| spec.default_value.to_string())
                .collect(),
        ),
        (
            "Description",
            published
                .iter()
                .map(|spec| spec.description.to_string())
                .collect(),
        ),
        (
            "AllowedValues",
            published
                .iter()
                .map(|spec| spec.allowed_values.to_string())
                .collect(),
        ),
        (
            "ApplyType",
            published
                .iter()
                .map(|spec| spec.apply_type.to_string())
                .collect(),
        ),
        (
            "DataType",
            published
                .iter()
                .map(|spec| spec.data_type.to_string())
                .collect(),
        ),
        (
            "IsModifiable",
            published
                .iter()
                .map(|spec| spec.is_modifiable.to_string())
                .collect(),
        ),
        (
            "MinimumEngineVersion",
            published
                .iter()
                .map(|spec| state::minimum_engine_version_in_family(DOCDB_FAMILY, spec))
                .collect(),
        ),
        (
            "Source",
            published
                .iter()
                .map(|_| state::PARAMETER_SOURCE_ENGINE_DEFAULT.to_string())
                .collect(),
        ),
    ];
    for (element, want) in columns {
        let got = texts(&reported, element);
        assert_eq!(
            got, want,
            "case {case:?}: expected <{element}> {want:?} over the family's published \
             parameters, got {got:?}"
        );
    }
    // A published default is nobody's pending change, so it carries no `ApplyMethod`.
    assert_eq!(
        count(&reported, "ApplyMethod"),
        0,
        "case {case:?}: a published default is no caller's change, so it waits on no reboot, \
         got {reported}"
    );
}
