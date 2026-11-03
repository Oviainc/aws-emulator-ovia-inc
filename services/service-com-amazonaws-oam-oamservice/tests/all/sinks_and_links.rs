// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the sink/link lifecycle of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! sink/link record, so anything a create stored is what a later Get or
//! ListAttachedLinks reports, anything never stored (or since deleted) is the
//! modelled refusal, and nothing is echoed back out of the request that was not
//! stored first.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_oam_oamservice::state::MAX_LINKS_PER_SOURCE_ACCOUNT;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The access key id most cases sign with. A link belongs to the source account the
/// request speaks for, and that is read from this credential, so a case that needs a
/// second source account signs with a second key id.
pub(crate) const AKID: &str = "AKIDOAMTEST1";

/// The access key id [`create_sink_ok`] signs with: the MONITORING account that owns
/// every sink these suites create.
///
/// A link joins a source account to a sink in a monitoring account, so it is never
/// created by the account that owns the sink — a sink is created by this credential
/// and the links to it by the source-account credentials, which are all distinct from
/// it.
pub(crate) const MONITORING_AKID: &str = "AKIDOAMSINKOWNER";

/// Every value the modelled `ResourceType` enum declares, in the model's own order.
///
/// From `smithy_query(query="shape",shape="ResourceType")` — the same catalog the
/// handler checks against.
pub(crate) const RESOURCE_TYPES: &[&str] = &[
    "AWS::CloudWatch::Metric",
    "AWS::Logs::LogGroup",
    "AWS::XRay::Trace",
    "AWS::ApplicationInsights::Application",
    "AWS::InternetMonitor::Monitor",
    "AWS::ApplicationSignals::Service",
    "AWS::ApplicationSignals::ServiceLevelObjective",
];

/// One emulator with an empty store, mounted exactly as the router mounts it.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// One emulator mounted on `engine`, so a suite that reads or seeds the store
/// directly holds the very engine the handlers answer from.
pub(crate) fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_oam_oamservice::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `oam` in `region` by `akid`: the signal the router routes
/// on, and the one the handlers read the region and the source account from.
pub(crate) fn request(
    uri: &str,
    body: &Value,
    akid: &str,
    region: &str,
) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("oam.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/oam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call_as(
    router: &MultiServiceRouter,
    uri: &str,
    body: &Value,
    akid: &str,
    region: &str,
) -> (u16, Value) {
    dispatch_parsed(router, request(uri, body, akid, region)).await
}

/// Status and parsed body of one dispatched request, whatever method and URI it
/// carries.
///
/// Most of this service's operations are `POST /Operation`, which [`call_as`] builds;
/// the three tagging operations are REST-bound (`GET`/`PUT`/`DELETE
/// /tags/{ResourceArn}`), so a suite that builds its own request still reads the
/// answer through this one parser rather than a second copy of it.
pub(crate) async fn dispatch_parsed(
    router: &MultiServiceRouter,
    request: http::Request<bytes::Bytes>,
) -> (u16, Value) {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One call signed by [`AKID`] in `region`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    uri: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    call_as(router, uri, body, AKID, region).await
}

/// The modelled error shape a failed call reported.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.oam#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A member of a response body, as a string.
pub(crate) fn field(case: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {body:?}"))
        .to_string()
}

/// Assert `arn` is an ARN of this service's `resource` kind in `region`, and answer
/// its id segment.
fn assert_arn(case: &str, arn: &str, resource: &str, region: &str) -> String {
    let prefix = format!("arn:aws:oam:{region}:{ACCOUNT}:{resource}/");
    let id = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN starting {prefix}, got {arn:?}"));
    // AWS allocates "the random ID string ... generated as part of the sink ARN": a
    // v4 UUID, 8-4-4-4-12 lowercase hex.
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<usize>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a UUID id segment, got {id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit())),
        "case {case:?}: expected a hexadecimal UUID id segment, got {id:?}"
    );
    id.to_string()
}

/// Create a sink owned by the monitoring account ([`MONITORING_AKID`]), which must
/// succeed, and answer its ARN.
pub(crate) async fn create_sink_ok(
    router: &MultiServiceRouter,
    name: &str,
    region: &str,
) -> String {
    let (status, created) = call_as(
        router,
        "/CreateSink",
        &json!({ "Name": name }),
        MONITORING_AKID,
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSink {name}: expected 200, got {status} {created:?}"
    );
    field(name, &created, "Arn")
}

/// Create a link from `akid`'s source account to `sink_arn`, which must succeed, and
/// answer its ARN.
pub(crate) async fn create_link_ok(
    router: &MultiServiceRouter,
    sink_arn: &str,
    akid: &str,
    region: &str,
) -> String {
    let body = json!({
        "LabelTemplate": "$AccountName",
        "ResourceTypes": ["AWS::CloudWatch::Metric"],
        "SinkIdentifier": sink_arn,
    });
    let (status, created) = call_as(router, "/CreateLink", &body, akid, region).await;
    assert_eq!(
        status, 200,
        "CreateLink as {akid}: expected 200, got {status} {created:?}"
    );
    field(akid, &created, "Arn")
}

/// Every `LinkArn` `ListAttachedLinks` reports for `sink_identifier`, read one page
/// at a time with `max_results`, asserting the page contract as it walks: no page is
/// longer than asked for, every page but the last carries a `NextToken`, and the
/// last carries none.
pub(crate) async fn attached_link_arns(
    router: &MultiServiceRouter,
    case: &str,
    sink_identifier: &str,
    max_results: Option<i64>,
    region: &str,
) -> Vec<String> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 1..=32 {
        let mut body = Map::new();
        body.insert(
            "SinkIdentifier".to_string(),
            Value::String(sink_identifier.to_string()),
        );
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), Value::from(max_results));
        }
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), Value::String(token.clone()));
        }
        let (status, listed) =
            call(router, "/ListAttachedLinks", &Value::Object(body), region).await;
        assert_eq!(
            status, 200,
            "case {case:?} page {page}: expected 200, got {status} {listed:?}"
        );
        let items = listed
            .get("Items")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?} page {page}: Items is @required, got {listed:?}")
            });
        if let Some(max_results) = max_results {
            assert!(
                items.len() as i64 <= max_results,
                "case {case:?} page {page}: expected at most {max_results} items, got {}",
                items.len()
            );
        }
        for item in items {
            collected.push(field(case, item, "LinkArn"));
        }
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: pagination did not terminate after 32 pages");
}

#[tokio::test]
async fn create_sink_stores_the_sink_and_get_sink_reads_it_back() {
    // Rows differ along what the rule generalises over: the name's characters and
    // length (the modelled `^[a-zA-Z0-9_\.\-]{1,255}$`), the region the ARN carries,
    // and how many tags the sink holds.
    let max_length_name = "s".repeat(255);
    let cases: &[(&str, &str, &str, Value)] = &[
        ("plain name, no tags", "demo-sink", "us-west-2", json!({})),
        (
            "dots, dashes and underscores",
            "Team.Sink_01-prod",
            "us-east-1",
            json!({ "env": "prod" }),
        ),
        (
            "maximum-length name",
            max_length_name.as_str(),
            "eu-central-1",
            json!({ "a": "1", "b": "2", "c": "3" }),
        ),
        ("digits only", "20260101", "ap-southeast-2", json!({})),
        ("single character", "s", "sa-east-1", json!({ "k": "" })),
    ];

    for (case, name, region, tags) in cases {
        let router = emulator();
        let tag_map = tags.as_object().expect("the case's tags are an object");
        let mut body = json!({ "Name": name });
        if !tag_map.is_empty() {
            body["Tags"] = tags.clone();
        }

        let (status, created) = call(&router, "/CreateSink", &body, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateSink, got {status} {created:?}"
        );
        let arn = field(case, &created, "Arn");
        let id = assert_arn(case, &arn, "sink", region);
        assert_eq!(
            field(case, &created, "Id"),
            id,
            "case {case:?}: expected Id to be the ARN's id segment, got {created:?}"
        );
        assert_eq!(
            field(case, &created, "Name"),
            *name,
            "case {case:?}: expected the stored name, got {created:?}"
        );
        assert_eq!(
            created.get("Tags"),
            Some(tags),
            "case {case:?}: expected the stored tags, got {created:?}"
        );

        // Every identifier form the modelled `ResourceIdentifier` admits resolves to
        // the one stored sink: its ARN, its id, and its name.
        for identifier in [arn.as_str(), id.as_str(), name] {
            let (status, got) = call(
                &router,
                "/GetSink",
                &json!({ "Identifier": identifier, "IncludeTags": true }),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} via {identifier:?}: expected 200, got {status} {got:?}"
            );
            assert_eq!(
                (
                    field(case, &got, "Arn"),
                    field(case, &got, "Id"),
                    field(case, &got, "Name")
                ),
                (arn.clone(), id.clone(), (*name).to_string()),
                "case {case:?} via {identifier:?}: expected the stored sink, got {got:?}"
            );
            assert_eq!(
                got.get("Tags"),
                Some(tags),
                "case {case:?} via {identifier:?}: expected the stored tags, got {got:?}"
            );
        }

        // `IncludeTags` SUPPRESSES the projection rather than asking for it: a read
        // that leaves it out reports the sink's stored tag set, and only an explicit
        // `false` leaves `Tags` out.
        for (case_suffix, body, expected) in [
            (
                "with IncludeTags omitted",
                json!({ "Identifier": arn.clone() }),
                Some(tags.clone()),
            ),
            (
                "with IncludeTags false",
                json!({ "Identifier": arn.clone(), "IncludeTags": false }),
                None,
            ),
        ] {
            let (status, got) = call(&router, "/GetSink", &body, region).await;
            assert_eq!(
                status, 200,
                "case {case:?} {case_suffix}: expected 200, got {got:?}"
            );
            assert_eq!(
                got.get("Tags"),
                expected.as_ref(),
                "case {case:?} {case_suffix}: expected {expected:?}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_region_holds_one_sink() {
    // "Each account can contain one sink per Region" (CreateSink's documentation).
    let router = emulator();
    let first = create_sink_ok(&router, "sink-a", "us-east-1").await;

    // The same name again, and then a name nothing holds: the slot is taken either
    // way, so BOTH are the account/Region quota refusal and neither depends on the
    // name. The exhaustive table for this rule lives in `refusal_classes.rs`.
    for (case, name) in [("the name already in use", "sink-a"), ("an unused name", "sink-b")] {
        let (status, body) = call(&router, "/CreateSink", &json!({ "Name": name }), "us-east-1")
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (429, "ServiceQuotaExceededException"),
            "case {case:?}: expected 429 ServiceQuotaExceededException, got {status} {body:?}"
        );
    }
    let (status, body) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": "sink-b" }),
        "us-east-1",
    )
    .await;
    assert_eq!(
        status, 404,
        "the refused sink must not have been stored, got {body:?}"
    );

    // The first sink is untouched, and another Region may hold its own sink.
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": "sink-a" }),
        "us-east-1",
    )
    .await;
    assert_eq!(status, 200, "the first sink must still be there, got {got:?}");
    assert_eq!(
        field("first sink", &got, "Arn"),
        first,
        "expected the first sink's ARN, got {got:?}"
    );

    let second = create_sink_ok(&router, "sink-b", "eu-west-1").await;
    assert_arn("sink in another Region", &second, "sink", "eu-west-1");
    // And a regional endpoint only sees its own Region's sink.
    let (status, body) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": "sink-a" }),
        "eu-west-1",
    )
    .await;
    assert_eq!(
        status, 404,
        "a sink in another Region must not resolve, got {body:?}"
    );
}

#[tokio::test]
async fn get_sink_refuses_an_identifier_that_names_no_stored_sink() {
    let router = emulator();
    let arn = create_sink_ok(&router, "demo-sink", "us-west-2").await;
    let id = arn.rsplit('/').next().expect("the ARN's id segment");

    let cases: &[(&str, String)] = &[
        (
            "an id nothing was stored under",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("a name nothing was stored under", "other-sink".to_string()),
        (
            "the right id in another account",
            format!("arn:aws:oam:us-west-2:123456789012:sink/{id}"),
        ),
        (
            "the right id in another Region",
            format!("arn:aws:oam:eu-west-1:{ACCOUNT}:sink/{id}"),
        ),
        (
            "the right id in another partition",
            format!("arn:aws-cn:oam:us-west-2:{ACCOUNT}:sink/{id}"),
        ),
    ];

    for (case, identifier) in cases {
        let (status, body) = call(
            &router,
            "/GetSink",
            &json!({ "Identifier": identifier }),
            "us-west-2",
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body:?}"
        );
    }

    // The control: the stored sink's own ARN still resolves.
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": arn.clone() }),
        "us-west-2",
    )
    .await;
    assert_eq!(status, 200, "the stored sink must resolve, got {got:?}");
}

#[tokio::test]
async fn create_link_resolves_the_label_and_get_link_reads_the_link_back() {
    // Rows differ along the label template (each modelled variable, a variable
    // embedded in custom text, and a label with no variable at all) and along the
    // resource types shared — including values the issue did not cite.
    let longest_template = format!("{}-$AccountName", "l".repeat(64 - "-$AccountName".len()));
    let longest_label = longest_template.replace("$AccountName", ACCOUNT);
    let cases: &[(&str, &str, String, Vec<&str>, Value)] = &[
        (
            "$AccountName",
            "$AccountName",
            ACCOUNT.to_string(),
            vec!["AWS::CloudWatch::Metric"],
            Value::Null,
        ),
        (
            "$AccountEmail",
            "$AccountEmail",
            ACCOUNT.to_string(),
            vec!["AWS::Logs::LogGroup", "AWS::XRay::Trace"],
            Value::Null,
        ),
        (
            "$AccountEmailNoDomain",
            "$AccountEmailNoDomain",
            ACCOUNT.to_string(),
            vec!["AWS::InternetMonitor::Monitor"],
            Value::Null,
        ),
        (
            "a variable inside custom text",
            "prod-$AccountName-logs",
            format!("prod-{ACCOUNT}-logs"),
            vec![
                "AWS::ApplicationSignals::Service",
                "AWS::ApplicationSignals::ServiceLevelObjective",
                "AWS::ApplicationInsights::Application",
            ],
            json!({ "LogGroupConfiguration": { "Filter": "LogGroupName LIKE 'aws/lambda/%'" } }),
        ),
        (
            "a custom label with no variable",
            "the-source-account",
            "the-source-account".to_string(),
            RESOURCE_TYPES.to_vec(),
            json!({
                "LogGroupConfiguration": { "Filter": "LogGroupName NOT IN ('Private-Log-Group')" },
                "MetricConfiguration": { "Filter": "Namespace LIKE 'AWS/%'" },
            }),
        ),
        (
            "a maximum-length template",
            longest_template.as_str(),
            longest_label.clone(),
            vec!["AWS::CloudWatch::Metric"],
            Value::Null,
        ),
    ];

    for (case, template, expected_label, resource_types, configuration) in cases {
        let region = "us-west-2";
        let router = emulator();
        let sink_arn = create_sink_ok(&router, "demo-sink", region).await;

        let mut body = json!({
            "LabelTemplate": template,
            "ResourceTypes": resource_types,
            "SinkIdentifier": sink_arn.clone(),
            "Tags": { "team": "observability" },
        });
        if !configuration.is_null() {
            body["LinkConfiguration"] = configuration.clone();
        }

        let (status, created) = call(&router, "/CreateLink", &body, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateLink, got {status} {created:?}"
        );
        let arn = field(case, &created, "Arn");
        let id = assert_arn(case, &arn, "link", region);
        let expected_types = Value::from(resource_types.clone());
        for (member, expected) in [
            ("Id", Value::String(id.clone())),
            ("Label", Value::String(expected_label.clone())),
            ("LabelTemplate", Value::String((*template).to_string())),
            ("SinkArn", Value::String(sink_arn.clone())),
            ("ResourceTypes", expected_types.clone()),
            ("Tags", json!({ "team": "observability" })),
        ] {
            assert_eq!(
                created.get(member),
                Some(&expected),
                "case {case:?}: expected {member} to be {expected:?}, got {created:?}"
            );
        }
        assert_eq!(
            created.get("LinkConfiguration"),
            (!configuration.is_null()).then_some(configuration),
            "case {case:?}: expected the stored LinkConfiguration, got {created:?}"
        );

        // Read back through GetLink, by ARN and by bare id: the persisted link,
        // field for field.
        for identifier in [arn.as_str(), id.as_str()] {
            let (status, got) = call(
                &router,
                "/GetLink",
                &json!({ "Identifier": identifier, "IncludeTags": true }),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} via {identifier:?}: expected 200, got {status} {got:?}"
            );
            for (member, expected) in [
                ("Arn", Value::String(arn.clone())),
                ("Id", Value::String(id.clone())),
                ("Label", Value::String(expected_label.clone())),
                ("LabelTemplate", Value::String((*template).to_string())),
                ("SinkArn", Value::String(sink_arn.clone())),
                ("ResourceTypes", expected_types.clone()),
                ("Tags", json!({ "team": "observability" })),
            ] {
                assert_eq!(
                    got.get(member),
                    Some(&expected),
                    "case {case:?} via {identifier:?}: expected {member} to be {expected:?}, \
                     got {got:?}"
                );
            }
            assert_eq!(
                got.get("LinkConfiguration"),
                (!configuration.is_null()).then_some(configuration),
                "case {case:?} via {identifier:?}: expected the stored LinkConfiguration, \
                 got {got:?}"
            );
        }

        // `IncludeTags` SUPPRESSES the projection rather than asking for it: omitted,
        // the link's stored tags are reported; only an explicit `false` leaves them
        // out.
        for (case_suffix, body, expected) in [
            (
                "with IncludeTags omitted",
                json!({ "Identifier": arn.clone() }),
                Some(json!({ "team": "observability" })),
            ),
            (
                "with IncludeTags false",
                json!({ "Identifier": arn.clone(), "IncludeTags": false }),
                None,
            ),
        ] {
            let (status, got) = call(&router, "/GetLink", &body, region).await;
            assert_eq!(
                status, 200,
                "case {case:?} {case_suffix}: expected 200, got {got:?}"
            );
            assert_eq!(
                got.get("Tags"),
                expected.as_ref(),
                "case {case:?} {case_suffix}: expected {expected:?}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn create_link_refuses_a_request_it_cannot_store() {
    let region = "us-west-2";
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let unknown_sink = "22222222-3333-4444-5555-666666666666";

    // Each row is a request the service cannot store, with the code it is refused
    // with: the two modelled 400s this operation declares, and the request front
    // end's own code for an element outside the modelled `ResourceType` enum.
    let cases: &[(&str, Value, &str)] = &[
        (
            "a resource type the model does not declare",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": ["AWS::Logs::LogStream"],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "BadRequestException",
        ),
        (
            "a modelled resource type in the wrong case",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": ["aws::logs::loggroup"],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "BadRequestException",
        ),
        (
            "a SinkIdentifier naming no stored sink",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": ["AWS::CloudWatch::Metric"],
                "SinkIdentifier": unknown_sink,
            }),
            "InvalidParameterException",
        ),
        (
            "no resource types at all",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "InvalidParameterException",
        ),
        (
            "a LabelTemplate one character past the modelled maximum",
            json!({
                "LabelTemplate": "l".repeat(65),
                "ResourceTypes": ["AWS::CloudWatch::Metric"],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "InvalidParameterException",
        ),
        (
            "an empty LabelTemplate, one character below the modelled minimum",
            json!({
                "LabelTemplate": "",
                "ResourceTypes": ["AWS::CloudWatch::Metric"],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "InvalidParameterException",
        ),
        (
            "no LabelTemplate",
            json!({
                "ResourceTypes": ["AWS::CloudWatch::Metric"],
                "SinkIdentifier": sink_arn.clone(),
            }),
            "MissingRequiredParameterException",
        ),
        (
            "no SinkIdentifier",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": ["AWS::CloudWatch::Metric"],
            }),
            "MissingRequiredParameterException",
        ),
    ];

    for (case, body, expected_code) in cases {
        let (status, got) = call(&router, "/CreateLink", body, region).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            *expected_code,
            "case {case:?}: got {got:?}"
        );
    }

    // Not one of them stored anything.
    assert_eq!(
        attached_link_arns(&router, "after the refusals", &sink_arn, None, region).await,
        Vec::<String>::new(),
        "a refused CreateLink must not have stored a link"
    );

    // The control: a request the service CAN store is stored.
    let arn = create_link_ok(&router, &sink_arn, AKID, region).await;
    assert_eq!(
        attached_link_arns(&router, "the control", &sink_arn, None, region).await,
        vec![arn],
        "the accepted CreateLink must be the one stored link"
    );
}

#[tokio::test]
async fn a_source_account_links_to_a_sink_once() {
    let region = "eu-west-1";
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let first = create_link_ok(&router, &sink_arn, "AKIDSOURCEONE", region).await;

    // The same source account again: it is already attached to this sink.
    let body = json!({
        "LabelTemplate": "$AccountName",
        "ResourceTypes": ["AWS::Logs::LogGroup"],
        "SinkIdentifier": sink_arn.clone(),
    });
    let (status, got) = call_as(&router, "/CreateLink", &body, "AKIDSOURCEONE", region).await;
    assert_eq!(
        status, 409,
        "a second link from one source account: expected 409, got {status} {got:?}"
    );
    assert_eq!(
        error_code(&got),
        "ConflictException",
        "a second link from one source account: got {got:?}"
    );

    // Another source account may attach to the same sink.
    let second = create_link_ok(&router, &sink_arn, "AKIDSOURCETWO", region).await;
    assert_ne!(
        first, second,
        "two source accounts must hold two distinct links"
    );
    assert_eq!(
        attached_link_arns(&router, "two source accounts", &sink_arn, None, region)
            .await
            .into_iter()
            .collect::<BTreeSet<String>>(),
        BTreeSet::from([first, second]),
        "the sink must report exactly the two stored links"
    );
}

#[tokio::test]
async fn one_source_account_holds_at_most_the_documented_number_of_links() {
    // "Each source account can be linked to as many as five monitoring accounts"
    // (CreateLink's documentation). Each of this emulator's sinks lives in its own
    // Region, so the same source account attaches to one sink per Region until the
    // quota is reached — and the call past it is the modelled quota refusal.
    let router = emulator();
    let regions = [
        "us-east-1",
        "us-east-2",
        "us-west-1",
        "us-west-2",
        "eu-west-1",
        "eu-central-1",
    ];
    assert!(
        regions.len() > MAX_LINKS_PER_SOURCE_ACCOUNT,
        "the case needs one Region past the quota"
    );

    for (index, region) in regions.iter().enumerate() {
        let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
        let body = json!({
            "LabelTemplate": "$AccountName",
            "ResourceTypes": ["AWS::CloudWatch::Metric"],
            "SinkIdentifier": sink_arn.clone(),
        });
        let (status, got) = call_as(&router, "/CreateLink", &body, "AKIDONESOURCE", region).await;
        if index < MAX_LINKS_PER_SOURCE_ACCOUNT {
            assert_eq!(
                status, 200,
                "link {} of the quota: expected 200, got {status} {got:?}",
                index + 1
            );
        } else {
            assert_eq!(
                status, 429,
                "link {} is past the quota: expected 429, got {status} {got:?}",
                index + 1
            );
            assert_eq!(
                error_code(&got),
                "ServiceQuotaExceededException",
                "link {} is past the quota: got {got:?}",
                index + 1
            );
            // And nothing was stored for it.
            assert_eq!(
                attached_link_arns(&router, "past the quota", &sink_arn, None, region).await,
                Vec::<String>::new(),
                "a refused CreateLink must not have stored a link"
            );
        }
    }
}

#[tokio::test]
async fn list_attached_links_pages_over_the_stored_links() {
    let region = "us-west-2";
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let other_region_sink = create_sink_ok(&router, "other-sink", "us-east-1").await;

    // Four source accounts, so the one sink holds four links to page over.
    let mut expected = BTreeSet::new();
    for akid in ["AKIDSOURCE1", "AKIDSOURCE2", "AKIDSOURCE3", "AKIDSOURCE4"] {
        expected.insert(create_link_ok(&router, &sink_arn, akid, region).await);
    }
    // A link on another sink, which must never appear in this sink's enumeration.
    let other = create_link_ok(&router, &other_region_sink, "AKIDSOURCE1", "us-east-1").await;

    // Every page size from one item per page to well past the collection, plus the
    // published bounds of the modelled `MaxResults` range (1..1000) and no
    // `MaxResults` at all: the enumeration is the same set either way.
    for max_results in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(1000)] {
        let case = format!("MaxResults={max_results:?}");
        let collected =
            attached_link_arns(&router, &case, &sink_arn, max_results, region).await;
        assert_eq!(
            collected.len(),
            expected.len(),
            "case {case:?}: expected {} items with no repeats, got {collected:?}",
            expected.len()
        );
        assert_eq!(
            collected.into_iter().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: expected exactly the stored links"
        );
    }
    assert!(
        !expected.contains(&other),
        "the other sink's link must not be attached to this sink"
    );

    // The items report what was stored, not what was asked for.
    let (status, listed) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": 1 }),
        region,
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {listed:?}");
    let item = listed
        .get("Items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| panic!("expected one item, got {listed:?}"));
    assert_eq!(
        item.get("Label"),
        Some(&Value::String(ACCOUNT.to_string())),
        "expected the resolved label, got {item:?}"
    );
    assert_eq!(
        item.get("ResourceTypes"),
        Some(&json!(["AWS::CloudWatch::Metric"])),
        "expected the stored resource types, got {item:?}"
    );

    // Refusals: a sink that does not exist, a token this operation never issued, and
    // a `MaxResults` one past each published bound of the modelled 1..1000 range.
    let refusals: &[(&str, Value, u16, &str)] = &[
        (
            "a sink nothing was stored under",
            json!({ "SinkIdentifier": "33333333-4444-5555-6666-777777777777" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a sink in another Region",
            json!({ "SinkIdentifier": other_region_sink.clone() }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a NextToken this operation never issued",
            json!({ "SinkIdentifier": sink_arn.clone(), "NextToken": "not-a-token" }),
            400,
            "InvalidParameterException",
        ),
        (
            "MaxResults one below the modelled minimum",
            json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": 0 }),
            400,
            "InvalidParameterException",
        ),
        (
            "MaxResults one past the modelled maximum",
            json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": 1001 }),
            400,
            "InvalidParameterException",
        ),
        (
            "no SinkIdentifier",
            json!({ "MaxResults": 1 }),
            400,
            "MissingRequiredParameterException",
        ),
    ];
    for (case, body, expected_status, expected_code) in refusals {
        let (status, got) = call(&router, "/ListAttachedLinks", body, region).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            *expected_code,
            "case {case:?}: got {got:?}"
        );
    }
}

#[tokio::test]
async fn deletes_remove_the_resource_and_a_sink_keeps_its_links() {
    let region = "us-west-2";
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let first = create_link_ok(&router, &sink_arn, "AKIDSOURCE1", region).await;
    let second = create_link_ok(&router, &sink_arn, "AKIDSOURCE2", region).await;

    // "You must delete all links to a sink before you can delete that sink."
    let (status, got) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(
        status, 409,
        "a sink with links attached: expected 409, got {status} {got:?}"
    );
    assert_eq!(
        error_code(&got),
        "ConflictException",
        "a sink with links attached: got {got:?}"
    );
    let (status, _) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "the refused delete must have kept the sink");

    // Deleting a link empties it out of the sink's enumeration and out of GetLink.
    let (status, body) = call(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": first.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "DeleteLink: expected 200, got {body:?}");
    assert!(
        body.is_null() || body == json!({}),
        "DeleteLink models no output members, got {body:?}"
    );
    let (status, got) = call(
        &router,
        "/GetLink",
        &json!({ "Identifier": first.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 404, "the deleted link must be gone, got {got:?}");
    assert_eq!(
        error_code(&got),
        "ResourceNotFoundException",
        "the deleted link: got {got:?}"
    );
    assert_eq!(
        attached_link_arns(&router, "after one delete", &sink_arn, None, region).await,
        vec![second.clone()],
        "only the surviving link must be enumerated"
    );

    // Deleting it again is a 404: there is nothing left to delete.
    let (status, got) = call(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": first.clone() }),
        region,
    )
    .await;
    assert_eq!(
        status, 404,
        "a second DeleteLink: expected 404, got {status} {got:?}"
    );

    // With every link gone, `Items` is an empty list rather than an absent member,
    // and the sink can be deleted.
    let (status, _) = call(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": second.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "DeleteLink: expected 200");
    let (status, listed) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {listed:?}");
    assert_eq!(
        listed.get("Items"),
        Some(&json!([])),
        "Items is @required, so an empty enumeration is [], got {listed:?}"
    );

    let (status, body) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "DeleteSink: expected 200, got {body:?}");
    assert!(
        body.is_null() || body == json!({}),
        "DeleteSink models no output members, got {body:?}"
    );
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 404, "the deleted sink must be gone, got {got:?}");
    assert_eq!(
        error_code(&got),
        "ResourceNotFoundException",
        "the deleted sink: got {got:?}"
    );
    let (status, got) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(
        status, 404,
        "a deleted sink has no enumeration, got {status} {got:?}"
    );

    // And the name is free again: "If you delete a sink, you can then create a new
    // one in that Region."
    let replacement = create_sink_ok(&router, "demo-sink", region).await;
    assert_ne!(
        replacement, sink_arn,
        "the replacement sink must carry its own newly allocated id"
    );
}

#[tokio::test]
async fn a_page_token_resumes_only_the_enumeration_it_was_issued_for() {
    let region = "us-west-2";
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let first = create_link_ok(&router, &sink_arn, "AKIDSOURCE1", region).await;
    let second = create_link_ok(&router, &sink_arn, "AKIDSOURCE2", region).await;

    // A token from a two-link enumeration resumes it.
    let (status, page) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_arn.clone(), "MaxResults": 1 }),
        region,
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {page:?}");
    let token = field("first page", &page, "NextToken");
    let (status, resumed) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": sink_arn.clone(), "NextToken": token.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {resumed:?}");
    let resumed_arns: BTreeSet<String> = resumed
        .get("Items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("Items is @required, got {resumed:?}"))
        .iter()
        .map(|item| field("resumed page", item, "LinkArn"))
        .collect();
    let all = BTreeSet::from([first.clone(), second.clone()]);
    assert!(
        resumed_arns.is_subset(&all) && !resumed_arns.is_empty(),
        "the resumed page must carry the links the first page did not, got {resumed_arns:?}"
    );

    // Take the sink away and build another one with the same name: the token names
    // the enumeration of a sink that no longer exists, so it cannot resume the new
    // one's — it is refused rather than silently resuming at an unrelated position.
    for link in [&first, &second] {
        let (status, _) = call(
            &router,
            "/DeleteLink",
            &json!({ "Identifier": link }),
            region,
        )
        .await;
        assert_eq!(status, 200, "DeleteLink {link}: expected 200");
    }
    let (status, _) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": sink_arn.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "DeleteSink: expected 200");
    let replacement = create_sink_ok(&router, "demo-sink", region).await;
    create_link_ok(&router, &replacement, "AKIDSOURCE1", region).await;

    let (status, got) = call(
        &router,
        "/ListAttachedLinks",
        &json!({ "SinkIdentifier": replacement.clone(), "NextToken": token }),
        region,
    )
    .await;
    assert_eq!(
        status, 400,
        "another sink's token: expected 400, got {status} {got:?}"
    );
    assert_eq!(
        error_code(&got),
        "InvalidParameterException",
        "another sink's token: got {got:?}"
    );
}

/// Every id the service hands out is its own: opaque, version-4-UUID shaped, and
/// decided by the service rather than by anything the request carried.
///
/// The table spans what an id could wrongly be derived from — the resource NAME (the
/// same name in several regions), the REGION (several names in one region through
/// create/delete/create), the resource KIND (sinks beside links) and the ORDER of the
/// calls. Every row must still produce a distinct id, so no id is a function of the
/// request.
#[tokio::test]
async fn every_resource_gets_its_own_opaque_id() {
    let router = emulator();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut minted = 0usize;

    // The same sink name in several regions, and several names in one region: an id
    // built from the name, the region, or the two together would repeat here.
    let rows: &[(&str, &str)] = &[
        ("demo-sink", "us-east-1"),
        ("demo-sink", "us-west-2"),
        ("demo-sink", "eu-central-1"),
        ("another-sink", "ap-southeast-2"),
        ("a.name_with-punctuation", "sa-east-1"),
    ];
    for (name, region) in rows {
        let case = format!("sink {name} in {region}");
        let arn = create_sink_ok(&router, name, region).await;
        let id = assert_arn(&case, &arn, "sink", region);
        // A v4 UUID: the 13th hex digit is the version and the 17th carries the
        // variant in its top two bits (RFC 4122), which is what AWS's ids show.
        let digits: Vec<char> = id.chars().filter(|c| *c != '-').collect();
        assert_eq!(
            digits[12], '4',
            "case {case:?}: expected a version-4 id, got {id:?}"
        );
        assert!(
            ['8', '9', 'a', 'b'].contains(&digits[16]),
            "case {case:?}: expected an RFC 4122 variant id, got {id:?}"
        );
        assert!(
            !id.contains(name) && !id.contains(region),
            "case {case:?}: an id must not be derived from the request, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: id {id:?} was handed out twice"
        );
        minted += 1;

        // Links in the same region, from two source accounts, get ids of their own —
        // distinct from each other and from every sink's.
        for akid in ["AKIDSOURCE1", "AKIDSOURCE2"] {
            let case = format!("link from {akid} in {region}");
            let link_arn = create_link_ok(&router, &arn, akid, region).await;
            let link_id = assert_arn(&case, &link_arn, "link", region);
            assert!(
                ids.insert(link_id.clone()),
                "case {case:?}: id {link_id:?} was handed out twice"
            );
            minted += 1;
            let (status, _) = call(
                &router,
                "/DeleteLink",
                &json!({ "Identifier": link_arn }),
                region,
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: DeleteLink expected 200");
        }

        // Delete and re-create under the same name in the same region: a reused id
        // would mean a stale ARN resolving to a new resource.
        let (status, _) = call(
            &router,
            "/DeleteSink",
            &json!({ "Identifier": arn.clone() }),
            region,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DeleteSink expected 200");
        let again = create_sink_ok(&router, name, region).await;
        let again_id = assert_arn(&case, &again, "sink", region);
        assert!(
            ids.insert(again_id.clone()),
            "case {case:?}: re-creating reused the deleted sink's id {again_id:?}"
        );
        minted += 1;
        let (status, _) = call(
            &router,
            "/DeleteSink",
            &json!({ "Identifier": again }),
            region,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: DeleteSink expected 200");
    }

    assert_eq!(
        ids.len(),
        minted,
        "every create must mint its own id: {minted} creates produced {} distinct ids",
        ids.len()
    );
}
