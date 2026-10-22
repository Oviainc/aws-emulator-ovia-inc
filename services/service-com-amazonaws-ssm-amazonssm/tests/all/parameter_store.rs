// SPDX-License-Identifier: CC0-1.0

//! Parameter Store end to end: `PutParameter` writes, `GetParameter` reads back.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"). Nothing reaches into the
//! state struct, so a response that is not computed from persisted state cannot pass.
//!
//! The suites are tables: each row is one input along the axis the rule under test
//! generalises over (name form and depth, parameter type, data type, value size,
//! tier, version count, selector form), and the single `/app/...` parameter of the
//! recorded unit is only one row.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. Deliberately NOT the emulator's
/// default (`state::DEFAULT_REGION`), so an ARN that carries it proves the region came
/// from the request's SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The `ParameterType` values the model's enum declares
/// (`smithy_query(query="shape", shape="ParameterType")`).
const PARAMETER_TYPES: &[&str] = &["String", "StringList", "SecureString"];

/// The `DataType` values `PutParameter` documents
/// (`smithy_query(query="doc", shape="PutParameterRequest", member="DataType")`:
/// "text aws:ec2:image aws:ssm:integration").
const DATA_TYPES: &[&str] = &["text", "aws:ec2:image", "aws:ssm:integration"];

/// The published content-size limits of the two tiers
/// (`smithy_query(query="doc", shape="PutParameterRequest", member="Tier")`:
/// standard 4 KB, advanced 8 KB).
const STANDARD_LIMIT: usize = 4 * 1024;
const ADVANCED_LIMIT: usize = 8 * 1024;

/// The published hierarchy-depth limit (`PutParameterRequest$Name`: "Parameter
/// hierarchies are limited to a maximum depth of fifteen levels").
const MAX_LEVELS: usize = 15;

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where an ARN's region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonSSM.{operation}"))
        .header("host", format!("ssm.{SIGNED_REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDSSMTEST/20260101/{SIGNED_REGION}\
                 /ssm/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the named wire error code.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected a {expected_code} from {operation}, got {parsed}"
    );
}

/// A required string member of a response body.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// A `long` member whose modelled default is `0`, so an absent member reads as zero.
fn number(body: &Value, member: &str, case: &str) -> i64 {
    match &body[member] {
        Value::Null => 0,
        value => value
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected an integer {member}, got {body}")),
    }
}

/// The ARN a parameter named `name` has in [`SIGNED_REGION`]: the name's leading
/// hierarchy slash is the one `parameter/` already carries.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:ssm:{SIGNED_REGION}:{ACCOUNT}:parameter/{}",
        name.trim_start_matches('/')
    )
}

/// One `PutParameter` body, with only the members a row sets.
fn put(name: &str, value: &str, parameter_type: Option<&str>) -> Value {
    let mut body = json!({ "Name": name, "Value": value });
    if let Some(parameter_type) = parameter_type {
        body["Type"] = json!(parameter_type);
    }
    body
}

/// `PutParameter` writes a parameter, and `GetParameter` reports exactly what was
/// written: name, type, value, version 1, the minted ARN and the data type.
#[tokio::test]
async fn a_written_parameter_is_read_back_whole() {
    let router = service();

    // The rows differ along name form and depth, parameter type, value shape and
    // data type. `aws:ec2:image` is a catalog entry the issue did not cite.
    let rows = [
        ("/app/config/db-string13", "db-string-value", "String", None),
        ("/app/config/hosts", "a.example,b.example", "StringList", None),
        ("/app/config/password", "s3cr3t", "SecureString", None),
        ("flat-name", "no-hierarchy", "String", None),
        ("/dots.and_dashes/a-b.c_d", "punctuation", "String", None),
        (
            "/app/config/ami",
            "ami-12345abcdeEXAMPLE",
            "String",
            Some("aws:ec2:image"),
        ),
        (
            "/app/config/integration",
            "arn:aws:events:us-west-2:000000000000:rule/Example",
            "String",
            Some("aws:ssm:integration"),
        ),
        // The published hierarchy bound: fifteen levels is accepted.
        (
            "/l1/l2/l3/l4/l5/l6/l7/l8/l9/l10/l11/l12/l13/l14/l15",
            "deepest-accepted",
            "String",
            None,
        ),
    ];

    for (name, value, parameter_type, data_type) in rows {
        let case = format!("{name} {parameter_type} data_type={data_type:?}");
        assert!(
            PARAMETER_TYPES.contains(&parameter_type),
            "case {case:?}: the row's type must be one of the modelled {PARAMETER_TYPES:?}"
        );
        let mut body = put(name, value, Some(parameter_type));
        if let Some(data_type) = data_type {
            assert!(
                DATA_TYPES.contains(&data_type),
                "case {case:?}: the row's data type must be one of the documented {DATA_TYPES:?}"
            );
            body["DataType"] = json!(data_type);
        }

        let written = ok(&router, "PutParameter", body, &case).await;
        assert_eq!(
            number(&written, "Version", &case),
            1,
            "case {case:?}: a created parameter is version 1, got {written}"
        );

        let read = ok(
            &router,
            "GetParameter",
            json!({ "Name": name }),
            &case,
        )
        .await;
        let parameter = &read["Parameter"];
        assert_eq!(
            string(parameter, "Name", &case),
            name,
            "case {case:?}: expected the stored name, got {parameter}"
        );
        assert_eq!(
            string(parameter, "Type", &case),
            parameter_type,
            "case {case:?}: expected the written type, got {parameter}"
        );
        assert_eq!(
            string(parameter, "Value", &case),
            value,
            "case {case:?}: expected the written value, got {parameter}"
        );
        assert_eq!(
            number(parameter, "Version", &case),
            1,
            "case {case:?}: expected version 1, got {parameter}"
        );
        assert_eq!(
            string(parameter, "ARN", &case),
            expected_arn(name),
            "case {case:?}: expected the parameter ARN, got {parameter}"
        );
        assert_eq!(
            string(parameter, "DataType", &case),
            data_type.unwrap_or("text"),
            "case {case:?}: expected the data type (text by default), got {parameter}"
        );
        assert!(
            parameter.get("LastModifiedDate").is_some_and(|v| !v.is_null()),
            "case {case:?}: expected a LastModifiedDate, got {parameter}"
        );
        assert!(
            parameter.get("Selector").is_none_or(Value::is_null),
            "case {case:?}: a read with no selector reports none, got {parameter}"
        );
    }
}

/// Reading a parameter changes nothing a later reader sees.
#[tokio::test]
async fn reading_a_parameter_leaves_it_as_it_was() {
    let router = service();
    let case = "repeat reads";
    ok(
        &router,
        "PutParameter",
        put("/app/read/twice", "stable", Some("String")),
        case,
    )
    .await;

    let first = ok(
        &router,
        "GetParameter",
        json!({ "Name": "/app/read/twice" }),
        case,
    )
    .await;
    let second = ok(
        &router,
        "GetParameter",
        json!({ "Name": "/app/read/twice" }),
        case,
    )
    .await;
    assert_eq!(
        first["Parameter"], second["Parameter"],
        "case {case:?}: two reads of one parameter must report the same thing, got {first} then {second}"
    );
}

/// An overwrite numbers a new version, and every retained version stays readable by
/// its `name:version` selector.
#[tokio::test]
async fn overwriting_a_parameter_versions_it() {
    let router = service();
    let name = "/app/versioned/value";
    let values = ["v1", "v2", "v3-longer", "v4"];

    for (index, value) in values.iter().enumerate() {
        let case = format!("write {value}");
        let expected_version = index as i64 + 1;
        let mut body = put(name, value, Some("String"));
        if index > 0 {
            body["Overwrite"] = json!(true);
        }
        let written = ok(&router, "PutParameter", body, &case).await;
        assert_eq!(
            number(&written, "Version", &case),
            expected_version,
            "case {case:?}: expected version {expected_version}, got {written}"
        );
    }

    // No selector reports the newest version.
    let case = "latest";
    let latest = ok(&router, "GetParameter", json!({ "Name": name }), case).await;
    assert_eq!(
        number(&latest["Parameter"], "Version", case),
        values.len() as i64,
        "case {case:?}: expected the newest version, got {latest}"
    );
    assert_eq!(
        string(&latest["Parameter"], "Value", case),
        *values.last().expect("the table has rows"),
        "case {case:?}: expected the newest value, got {latest}"
    );

    // Each version selector reports the value that call wrote.
    for (index, value) in values.iter().enumerate() {
        let version = index + 1;
        let case = format!("selector :{version}");
        let read = ok(
            &router,
            "GetParameter",
            json!({ "Name": format!("{name}:{version}") }),
            &case,
        )
        .await;
        let parameter = &read["Parameter"];
        assert_eq!(
            string(parameter, "Value", &case),
            *value,
            "case {case:?}: expected the value version {version} was written with, got {parameter}"
        );
        assert_eq!(
            number(parameter, "Version", &case),
            version as i64,
            "case {case:?}: expected version {version}, got {parameter}"
        );
        assert_eq!(
            string(parameter, "Selector", &case),
            version.to_string(),
            "case {case:?}: expected the selector the read used, got {parameter}"
        );
    }

    // A version number no version has.
    for selector in ["5", "0", "-1"] {
        let case = format!("absent version selector :{selector}");
        refused(
            &router,
            "GetParameter",
            json!({ "Name": format!("{name}:{selector}") }),
            "ParameterVersionNotFound",
            &case,
        )
        .await;
    }

    // A label selector resolves no version — nothing can attach a label while
    // `LabelParameterVersion` is a scaffold — so the `name:label` pair names no
    // parameter.
    for selector in ["release", "prod-2026", "v1.0"] {
        let case = format!("absent label selector :{selector}");
        refused(
            &router,
            "GetParameter",
            json!({ "Name": format!("{name}:{selector}") }),
            "ParameterNotFound",
            &case,
        )
        .await;
    }
}

/// Creating a parameter that exists is refused unless the request overwrites it.
#[tokio::test]
async fn creating_an_existing_parameter_needs_overwrite() {
    let router = service();

    for (name, value, parameter_type) in [
        ("/app/dup/string", "first", "String"),
        ("/app/dup/list", "a,b", "StringList"),
        ("flat-dup", "first", "String"),
    ] {
        let case = format!("{name} duplicate");
        ok(
            &router,
            "PutParameter",
            put(name, value, Some(parameter_type)),
            &case,
        )
        .await;

        // Default `Overwrite` (absent) and an explicit false are both refusals.
        refused(
            &router,
            "PutParameter",
            put(name, "second", Some(parameter_type)),
            "ParameterAlreadyExists",
            &case,
        )
        .await;
        let mut explicit = put(name, "second", Some(parameter_type));
        explicit["Overwrite"] = json!(false);
        refused(
            &router,
            "PutParameter",
            explicit,
            "ParameterAlreadyExists",
            &case,
        )
        .await;

        // The refused calls persisted nothing: the first value is still there at
        // version 1.
        let read = ok(&router, "GetParameter", json!({ "Name": name }), &case).await;
        assert_eq!(
            string(&read["Parameter"], "Value", &case),
            value,
            "case {case:?}: a refused overwrite must not change the value, got {read}"
        );
        assert_eq!(
            number(&read["Parameter"], "Version", &case),
            1,
            "case {case:?}: a refused overwrite must not mint a version, got {read}"
        );
    }
}

/// `Type` is required to create a parameter and optional to update one, in which
/// case the parameter keeps the type it was created with.
#[tokio::test]
async fn type_is_required_only_when_creating() {
    let router = service();

    for parameter_type in PARAMETER_TYPES {
        let name = format!("/app/types/{}", parameter_type.to_ascii_lowercase());
        let case = format!("{name} {parameter_type}");

        refused(
            &router,
            "PutParameter",
            put(&name, "no-type", None),
            "ValidationException",
            &case,
        )
        .await;

        ok(
            &router,
            "PutParameter",
            put(&name, "typed", Some(parameter_type)),
            &case,
        )
        .await;

        let mut update = put(&name, "updated", None);
        update["Overwrite"] = json!(true);
        ok(&router, "PutParameter", update, &case).await;

        let read = ok(&router, "GetParameter", json!({ "Name": name }), &case).await;
        assert_eq!(
            string(&read["Parameter"], "Type", &case),
            *parameter_type,
            "case {case:?}: an update with no Type keeps the created type, got {read}"
        );
        assert_eq!(
            string(&read["Parameter"], "Value", &case),
            "updated",
            "case {case:?}: the update's value is what is read back, got {read}"
        );
    }
}

/// The tier a parameter is created in: the requested one, except
/// `Intelligent-Tiering`, which is advanced only when the request carries an option
/// that requires an advanced parameter.
#[tokio::test]
async fn the_assigned_tier_follows_the_request() {
    let router = service();
    let small = "small-value";
    let over_standard = "x".repeat(STANDARD_LIMIT + 1);

    // (case, requested tier, value, policies, expected assigned tier)
    let rows = [
        ("no tier", None, small, None, "Standard"),
        ("Standard", Some("Standard"), small, None, "Standard"),
        ("Advanced", Some("Advanced"), small, None, "Advanced"),
        (
            "Intelligent-Tiering, nothing advanced needed",
            Some("Intelligent-Tiering"),
            small,
            None,
            "Standard",
        ),
        (
            "Intelligent-Tiering, value over 4 KB",
            Some("Intelligent-Tiering"),
            over_standard.as_str(),
            None,
            "Advanced",
        ),
        (
            "Intelligent-Tiering, with a policy",
            Some("Intelligent-Tiering"),
            small,
            Some(r#"[{"Type":"Expiration","Version":"1.0","Attributes":{"Timestamp":"2031-01-01T00:00:00.000Z"}}]"#),
            "Advanced",
        ),
        (
            "Advanced, with a policy",
            Some("Advanced"),
            small,
            Some(r#"[{"Type":"NoChangeNotification","Version":"1.0","Attributes":{"After":"15","Unit":"Days"}}]"#),
            "Advanced",
        ),
    ];

    for (index, (case, tier, value, policies, expected)) in rows.into_iter().enumerate() {
        let name = format!("/app/tiers/p{index}");
        let mut body = put(&name, value, Some("String"));
        if let Some(tier) = tier {
            body["Tier"] = json!(tier);
        }
        if let Some(policies) = policies {
            body["Policies"] = json!(policies);
        }
        let written = ok(&router, "PutParameter", body, case).await;
        assert_eq!(
            string(&written, "Tier", case),
            expected,
            "case {case:?}: expected the {expected} tier, got {written}"
        );
    }

    // A standard parameter cannot carry a policy at all.
    let mut standard_with_policy = put("/app/tiers/policy-on-standard", small, Some("String"));
    standard_with_policy["Tier"] = json!("Standard");
    standard_with_policy["Policies"] =
        json!(r#"[{"Type":"Expiration","Version":"1.0","Attributes":{"Timestamp":"2031-01-01T00:00:00.000Z"}}]"#);
    refused(
        &router,
        "PutParameter",
        standard_with_policy,
        "ValidationException",
        "Standard tier with a policy",
    )
    .await;
}

/// An update keeps the tier the parameter already carries when it names none, and an
/// advanced parameter can't be written back into the standard tier.
#[tokio::test]
async fn an_advanced_parameter_keeps_its_tier() {
    let router = service();
    let name = "/app/tiers/stays-advanced";
    // Larger than the standard limit, so the value itself only fits the advanced tier.
    let value = "y".repeat(STANDARD_LIMIT + 512);

    let case = "created Advanced";
    let mut create = put(name, &value, Some("String"));
    create["Tier"] = json!("Advanced");
    let created = ok(&router, "PutParameter", create, case).await;
    assert_eq!(
        string(&created, "Tier", case),
        "Advanced",
        "case {case:?}: expected the requested Advanced tier, got {created}"
    );

    // An update that names no tier keeps it advanced, so the oversized value is
    // still accepted and the reported tier does not change. `Intelligent-Tiering`
    // asks the service to evaluate the tier, and an advanced parameter stays
    // advanced even when the new value would fit the standard tier.
    for (case, body_tier, body_value) in [
        ("updated with no Tier", None, value.clone()),
        ("updated as Advanced", Some("Advanced"), value.clone()),
        (
            "updated as Intelligent-Tiering",
            Some("Intelligent-Tiering"),
            value.clone(),
        ),
        (
            "updated as Intelligent-Tiering with a small value",
            Some("Intelligent-Tiering"),
            "small-again".to_string(),
        ),
        ("updated with no Tier and a small value", None, "tiny".to_string()),
    ] {
        let mut update = put(name, &body_value, None);
        update["Overwrite"] = json!(true);
        if let Some(tier) = body_tier {
            update["Tier"] = json!(tier);
        }
        let updated = ok(&router, "PutParameter", update, case).await;
        assert_eq!(
            string(&updated, "Tier", case),
            "Advanced",
            "case {case:?}: an advanced parameter stays advanced, got {updated}"
        );
    }

    // Asking for the standard tier back is refused, whatever the new value's size.
    for (case, new_value) in [
        ("reverted with a small value", "small".to_string()),
        ("reverted with the same value", value.clone()),
    ] {
        let mut revert = put(name, &new_value, None);
        revert["Overwrite"] = json!(true);
        revert["Tier"] = json!("Standard");
        refused(&router, "PutParameter", revert, "ValidationException", case).await;
    }

    // The parameter is still advanced and still readable after all of that.
    let case = "after the refused reverts";
    let read = ok(&router, "GetParameter", json!({ "Name": name }), case).await;
    assert_eq!(
        string(&read["Parameter"], "Value", case),
        "tiny",
        "case {case:?}: expected the last accepted value, got {read}"
    );
}

/// A value has to fit the content-size limit of the tier it is stored in: each
/// published bound is accepted and one byte past it is refused.
#[tokio::test]
async fn the_value_must_fit_the_tier_content_limit() {
    let router = service();

    // (case, requested tier, value length, accepted?)
    let rows = [
        ("standard at the 4 KB bound", None, STANDARD_LIMIT, true),
        ("standard one byte over", None, STANDARD_LIMIT + 1, false),
        (
            "explicit Standard one byte over",
            Some("Standard"),
            STANDARD_LIMIT + 1,
            false,
        ),
        (
            "advanced at the 8 KB bound",
            Some("Advanced"),
            ADVANCED_LIMIT,
            true,
        ),
        (
            "advanced one byte over",
            Some("Advanced"),
            ADVANCED_LIMIT + 1,
            false,
        ),
        (
            "Intelligent-Tiering past 4 KB is advanced, so it fits",
            Some("Intelligent-Tiering"),
            STANDARD_LIMIT + 1,
            true,
        ),
        (
            "Intelligent-Tiering one byte over 8 KB",
            Some("Intelligent-Tiering"),
            ADVANCED_LIMIT + 1,
            false,
        ),
    ];

    for (index, (case, tier, length, accepted)) in rows.into_iter().enumerate() {
        let name = format!("/app/sizes/p{index}");
        let value = "v".repeat(length);
        let mut body = put(&name, &value, Some("String"));
        if let Some(tier) = tier {
            body["Tier"] = json!(tier);
        }
        if accepted {
            ok(&router, "PutParameter", body, case).await;
            let read = ok(&router, "GetParameter", json!({ "Name": name }), case).await;
            assert_eq!(
                string(&read["Parameter"], "Value", case).len(),
                length,
                "case {case:?}: expected the whole {length}-byte value back, got {read}"
            );
        } else {
            refused(&router, "PutParameter", body, "ValidationException", case).await;
        }
    }
}

/// `AllowedPattern` is a regular expression the value has to satisfy; a pattern
/// that is not a regular expression is refused as such.
#[tokio::test]
async fn allowed_pattern_validates_the_value() {
    let router = service();

    // (case, pattern, value, accepted?)
    let rows = [
        ("digits accept digits", r"^\d+$", "8080", true),
        ("digits reject letters", r"^\d+$", "eighty-eighty", false),
        ("alternation accepts a branch", "^(red|green|blue)$", "green", true),
        ("alternation rejects another word", "^(red|green|blue)$", "mauve", false),
        ("bounded length accepts", "^[a-z]{3,5}$", "abcd", true),
        ("bounded length rejects", "^[a-z]{3,5}$", "ab", false),
    ];

    for (index, (case, pattern, value, accepted)) in rows.into_iter().enumerate() {
        let name = format!("/app/patterns/p{index}");
        let mut body = put(&name, value, Some("String"));
        body["AllowedPattern"] = json!(pattern);
        if accepted {
            ok(&router, "PutParameter", body, case).await;
            let read = ok(&router, "GetParameter", json!({ "Name": name }), case).await;
            assert_eq!(
                string(&read["Parameter"], "Value", case),
                value,
                "case {case:?}: expected the accepted value back, got {read}"
            );
        } else {
            refused(
                &router,
                "PutParameter",
                body,
                "ParameterPatternMismatchException",
                case,
            )
            .await;
            refused(
                &router,
                "GetParameter",
                json!({ "Name": name }),
                "ParameterNotFound",
                case,
            )
            .await;
        }
    }

    // A pattern that does not compile is a different failure from a value that does
    // not match one.
    for (index, (case, pattern)) in [
        ("unclosed class", "[a-z"),
        ("dangling repeat", "*abc"),
        ("unclosed group", "(abc"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut body = put(&format!("/app/patterns/bad-{index}"), "x", Some("String"));
        body["AllowedPattern"] = json!(pattern);
        refused(
            &router,
            "PutParameter",
            body,
            "InvalidAllowedPatternException",
            case,
        )
        .await;
    }
}

/// The naming constraints, each with the failure it produces.
#[tokio::test]
async fn parameter_names_are_constrained() {
    let router = service();

    // (case, name, expected wire error code)
    let rows = [
        (
            "reserved aws prefix",
            "/aws/reserved/name".to_string(),
            "ValidationException",
        ),
        (
            "reserved ssm prefix",
            "/ssm/reserved/name".to_string(),
            "ValidationException",
        ),
        (
            "reserved prefix is case-insensitive",
            "/SSM/Reserved".to_string(),
            "ValidationException",
        ),
        (
            "reserved prefix without a hierarchy slash",
            "awsSomething".to_string(),
            "ValidationException",
        ),
        (
            "a space between characters",
            "/app/two words".to_string(),
            "ValidationException",
        ),
        (
            "a symbol outside a-zA-Z0-9_.-",
            "/app/dollar$sign".to_string(),
            "ParameterPatternMismatchException",
        ),
        (
            "a colon, which addresses a version",
            "/app/colon:name".to_string(),
            "ParameterPatternMismatchException",
        ),
        (
            "one level past the hierarchy depth limit",
            (1..=MAX_LEVELS + 1)
                .map(|level| format!("/l{level}"))
                .collect::<String>(),
            "HierarchyLevelLimitExceededException",
        ),
    ];

    for (case, name, expected) in rows {
        refused(
            &router,
            "PutParameter",
            put(&name, "value", Some("String")),
            expected,
            case,
        )
        .await;
    }

    // Spaces around a name are removed rather than refused, and the trimmed name is
    // the one the parameter is stored and read under.
    let case = "spaces around the name";
    let written = ok(
        &router,
        "PutParameter",
        put("  /app/trimmed/name  ", "trimmed", Some("String")),
        case,
    )
    .await;
    assert_eq!(
        number(&written, "Version", case),
        1,
        "case {case:?}: expected the trimmed name to be created, got {written}"
    );
    let read = ok(
        &router,
        "GetParameter",
        json!({ "Name": "/app/trimmed/name" }),
        case,
    )
    .await;
    assert_eq!(
        string(&read["Parameter"], "Name", case),
        "/app/trimmed/name",
        "case {case:?}: expected the trimmed name, got {read}"
    );
}

/// `DataType` accepts the documented values and refuses anything else; an absent
/// one is `text`.
#[tokio::test]
async fn data_type_is_one_of_the_documented_values() {
    let router = service();

    for data_type in DATA_TYPES {
        let name = format!("/app/datatypes/{}", data_type.replace(':', "-"));
        let case = format!("data type {data_type}");
        let mut body = put(&name, "ami-0abcdef1234567890", Some("String"));
        body["DataType"] = json!(data_type);
        ok(&router, "PutParameter", body, &case).await;
        let read = ok(&router, "GetParameter", json!({ "Name": name }), &case).await;
        assert_eq!(
            string(&read["Parameter"], "DataType", &case),
            *data_type,
            "case {case:?}: expected the written data type, got {read}"
        );
    }

    for data_type in ["aws:ec2:instance", "binary", "AWS:EC2:IMAGE"] {
        let case = format!("unsupported data type {data_type}");
        let mut body = put("/app/datatypes/refused", "x", Some("String"));
        body["DataType"] = json!(data_type);
        refused(&router, "PutParameter", body, "ValidationException", &case).await;
    }
}

/// A parameter can be read by its full ARN as well as by its name, and a name no
/// parameter has is `ParameterNotFound`.
#[tokio::test]
async fn a_parameter_is_addressed_by_name_or_arn() {
    let router = service();

    for name in ["/app/arn/hierarchical", "flat-arn-name"] {
        let case = format!("{name} by ARN");
        ok(
            &router,
            "PutParameter",
            put(name, "addressable", Some("String")),
            &case,
        )
        .await;
        let read = ok(
            &router,
            "GetParameter",
            json!({ "Name": expected_arn(name) }),
            &case,
        )
        .await;
        assert_eq!(
            string(&read["Parameter"], "Name", &case),
            name,
            "case {case:?}: an ARN addresses the same parameter, got {read}"
        );
    }

    for name in [
        "/app/arn/never-written",
        "never-written",
        "/app/arn/hierarchical/deeper",
    ] {
        refused(
            &router,
            "GetParameter",
            json!({ "Name": name }),
            "ParameterNotFound",
            name,
        )
        .await;
    }
}

/// Tags belong to the create: a request that also overwrites is refused, which is
/// what sends a caller to `AddTagsToResource`.
#[tokio::test]
async fn tags_cannot_be_written_by_an_overwrite() {
    let router = service();
    let name = "/app/tagged/parameter";
    let tags = json!([
        { "Key": "Environment", "Value": "Test" },
        { "Key": "Owner", "Value": "platform" },
    ]);

    let case = "tags on create";
    let mut create = put(name, "tagged", Some("String"));
    create["Tags"] = tags.clone();
    ok(&router, "PutParameter", create, case).await;

    let case = "tags on overwrite";
    let mut overwrite = put(name, "retagged", Some("String"));
    overwrite["Overwrite"] = json!(true);
    overwrite["Tags"] = tags;
    refused(
        &router,
        "PutParameter",
        overwrite,
        "ValidationException",
        case,
    )
    .await;

    // The refused call wrote no version.
    let read = ok(&router, "GetParameter", json!({ "Name": name }), case).await;
    assert_eq!(
        number(&read["Parameter"], "Version", case),
        1,
        "case {case:?}: a refused overwrite must not mint a version, got {read}"
    );
    assert_eq!(
        string(&read["Parameter"], "Value", case),
        "tagged",
        "case {case:?}: a refused overwrite must not change the value, got {read}"
    );

    // `Tags` and `Overwrite` are exclusive in the request itself, so the pair is
    // refused on a name no parameter has yet too — and nothing is created.
    let case = "tags with overwrite on a name that does not exist";
    let absent = "/app/tagged/absent";
    let mut body = put(absent, "tagged", Some("String"));
    body["Overwrite"] = json!(true);
    body["Tags"] = json!([{ "Key": "Environment", "Value": "Test" }]);
    refused(&router, "PutParameter", body, "ValidationException", case).await;
    refused(
        &router,
        "GetParameter",
        json!({ "Name": absent }),
        "ParameterNotFound",
        case,
    )
    .await;
}

/// Only the most recent versions are retained: once the retention bound is reached
/// the oldest version is dropped as a new one is created, and its selector then
/// names nothing.
#[tokio::test]
async fn only_the_retained_versions_stay_readable() {
    let router = service();
    let name = "/app/retained/value";

    // One more write than the bound, so exactly the oldest version falls out.
    // `state::MAX_VERSIONS_RETAINED` is 100 (`ParameterMaxVersionLimitExceeded`:
    // "Parameter Store retains the 100 most recently created versions").
    let retained: i64 = 100;
    for version in 1..=retained + 1 {
        let case = format!("write version {version}");
        let mut body = put(name, &format!("value-{version}"), Some("String"));
        if version > 1 {
            body["Overwrite"] = json!(true);
        }
        let written = ok(&router, "PutParameter", body, &case).await;
        assert_eq!(
            number(&written, "Version", &case),
            version,
            "case {case:?}: version numbers keep counting past the retention bound, got {written}"
        );
    }

    // The evicted oldest version.
    refused(
        &router,
        "GetParameter",
        json!({ "Name": format!("{name}:1") }),
        "ParameterVersionNotFound",
        "the evicted oldest version",
    )
    .await;

    // Every retained version, at both ends of the window and in the middle.
    for version in [2, 3, retained / 2, retained, retained + 1] {
        let case = format!("retained version {version}");
        let read = ok(
            &router,
            "GetParameter",
            json!({ "Name": format!("{name}:{version}") }),
            &case,
        )
        .await;
        assert_eq!(
            string(&read["Parameter"], "Value", &case),
            format!("value-{version}"),
            "case {case:?}: expected the value that version was written with, got {read}"
        );
    }
}
