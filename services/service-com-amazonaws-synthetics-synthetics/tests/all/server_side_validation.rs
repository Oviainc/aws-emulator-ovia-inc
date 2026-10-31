// SPDX-License-Identifier: CC0-1.0

//! Three rules this service applies that are NOT the model's own constraint traits, each
//! asserted end to end over the wire through the harness in `canary_lifecycle`.
//!
//! 1. **The runtime catalogue.** `RuntimeVersion` is modelled only as `String`
//!    `@length 1..1024` and documents its value set as something the service publishes
//!    ("For a list of valid runtime versions … see Canary Runtime Versions"), so the
//!    allow-list is a server-side check: a version the service publishes is accepted and
//!    round-trips, and any other value is refused with the modelled `ValidationException`
//!    (400) reporting the rejected value, leaving no canary behind.
//!
//! 2. **The page-size ceiling.** `DescribeCanaries$MaxResults` targets `MaxCanaryResults`
//!    (`@range 1..20`), which the recorded service does not enforce: page sizes above that
//!    modelled maximum are served, and only a value past the service's own ceiling of 1000
//!    is refused — naming the member `maxResults` and that ceiling.
//!
//! 3. **A `@pattern` as the model declares it.** A pattern-constrained member is reported
//!    against the EXPRESSION the model declares, character for character — the `^` / `$`
//!    pair a model writes to make the match total included, because those characters are
//!    part of the expression the service evaluated. The rule belongs to the service's
//!    refusal wording, not to one operation, so the rows below span body, URI-label and
//!    query-string members of several operations — implemented and still scaffolded.
//!
//! Every modelled constraint quoted here is read with
//! `smithy_query(query="operation",operation="…",model_path="services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json")`.

use serde_json::{json, Value};

use super::canary_lifecycle::{create_body, Synthetics};

/// Runtime versions the service publishes, taken from the catalogue in `src/state.rs`
/// (`RUNTIME_VERSION_CATALOG`, written from the CloudWatch Synthetics "Canary Runtime
/// Versions" documentation): the newest and the oldest entry of each family it lists, plus
/// the retired ones. None of these is the version the issue cited.
const PUBLISHED_RUNTIME_VERSIONS: [&str; 8] = [
    "syn-nodejs-playwright-3.0",
    "syn-nodejs-playwright-1.0",
    "syn-nodejs-puppeteer-11.0",
    "syn-nodejs-puppeteer-3.0",
    "syn-python-selenium-5.1",
    "syn-python-selenium-1.0",
    "syn-nodejs-2.0",
    "syn-1.0",
];

/// Values that are not runtime versions this service publishes. They differ along what the
/// rule generalises over — the family, the version, the case, the surrounding whitespace
/// and the naming scheme — and the first row is the one the issue recorded.
const UNPUBLISHED_RUNTIME_VERSIONS: [(&str, &str); 8] = [
    ("the recorded value", "syn-nodejs-puppeteer-not-a-real-version"),
    (
        "an unreleased version of a published family",
        "syn-nodejs-puppeteer-99.0",
    ),
    ("a family with no version at all", "syn-nodejs-puppeteer"),
    (
        "a published version in the wrong case",
        "SYN-NODEJS-PUPPETEER-9.0",
    ),
    (
        "a published version with trailing whitespace",
        "syn-nodejs-puppeteer-9.0 ",
    ),
    (
        "a published version with one component too many",
        "syn-nodejs-puppeteer-9.0.0",
    ),
    ("another service's runtime naming", "nodejs18.x"),
    ("a family this service has none of", "syn-java-selenium-1.0"),
];

/// The page-size ceiling the service applies, which is not the modelled maximum of 20.
const MAX_RESULTS_CEILING: i32 = 1000;

/// The page-size floor, which the model and the service agree on.
const MAX_RESULTS_FLOOR: i32 = 1;

/// One clause of the constraint envelope, wrapped in the single-violation envelope AWS
/// returns: `1 validation error detected: Value '<value>' at '<path>' …`.
fn one_violation(value: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// A `CreateCanary` body that satisfies every modelled constraint, with `member` replaced
/// by `value`.
fn create_with(name: &str, member: &str, value: Value) -> Value {
    let mut body = create_body(name);
    body[member] = value;
    body
}

/// A runtime version the service publishes is created and reported back; anything else is
/// refused with the modelled `ValidationException`, naming the rejected value, and leaves
/// the store untouched.
#[tokio::test]
async fn a_runtime_version_outside_the_published_catalogue_is_refused() {
    let aws = Synthetics::new("us-west-2");

    for (index, runtime) in PUBLISHED_RUNTIME_VERSIONS.iter().enumerate() {
        let name = format!("runtime-ok-{index}");
        let created = aws
            .create(create_with(&name, "RuntimeVersion", json!(runtime)))
            .await;
        assert_eq!(
            created.status, 200,
            "case {runtime:?}: expected 200, got {} ({:?})",
            created.status, created.body
        );
        // Read it back through the implemented reader: the runtime the create accepted is
        // the runtime the store holds.
        let read = aws.get(&name).await;
        assert_eq!(
            read.status, 200,
            "case {runtime:?}: expected the canary to be readable, got {} ({:?})",
            read.status, read.body
        );
        assert_eq!(
            read.at("Canary.RuntimeVersion").as_str(),
            Some(*runtime),
            "case {runtime:?}: expected the stored runtime version, got {:?}",
            read.at("Canary.RuntimeVersion")
        );
    }

    for (index, (case, runtime)) in UNPUBLISHED_RUNTIME_VERSIONS.iter().enumerate() {
        let name = format!("runtime-bad-{index}");
        let answer = aws
            .create(create_with(&name, "RuntimeVersion", json!(runtime)))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({runtime:?}): expected 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case} ({runtime:?}): expected x-amzn-errortype: ValidationException, got {:?}",
            answer.error_code
        );
        // The message is built from the request: it is the only place the service reports
        // WHICH value it rejected, so it has to name that value.
        let message = answer.at("Message").as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(runtime),
            "case {case} ({runtime:?}): expected the Message to name the rejected runtime \
             version, got {message:?}"
        );
        // A refused create leaves no record behind.
        let read = aws.get(&name).await;
        assert_eq!(
            read.status, 404,
            "case {case} ({runtime:?}): a refused create must store nothing, got {} ({:?})",
            read.status, read.body
        );
    }
}

/// `DescribeCanaries` serves every page size from the floor to the ceiling the SERVICE
/// applies — including the ones the modelled `@range` maximum of 20 would refuse — and
/// refuses only a value outside those bounds, naming the member and the bound it broke.
#[tokio::test]
async fn describe_canaries_honours_page_sizes_up_to_the_service_ceiling() {
    let aws = Synthetics::new("us-west-2");
    let names = ["alpha", "bravo", "charlie"];
    for name in names {
        let created = aws.create(create_body(name)).await;
        assert_eq!(
            created.status, 200,
            "case seed {name}: expected 200, got {} ({:?})",
            created.status, created.body
        );
    }

    // The floor, the modelled maximum, one past it, two values in between, and the
    // service's own ceiling: each must be served, with at most that many canaries on the
    // page.
    for size in [
        MAX_RESULTS_FLOOR,
        20,
        21,
        100,
        MAX_RESULTS_CEILING - 1,
        MAX_RESULTS_CEILING,
    ] {
        let answer = aws.describe(json!({ "MaxResults": size })).await;
        assert_eq!(
            answer.status, 200,
            "case max-results-{size}: expected 200, got {} ({:?})",
            answer.status, answer.body
        );
        let listed = answer
            .at("Canaries")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        let expected = usize::try_from(size).unwrap_or(names.len()).min(names.len());
        assert_eq!(
            listed, expected,
            "case max-results-{size}: expected {expected} canaries on the page, got {listed}"
        );
    }

    // Outside the bounds: one past the ceiling (the only rejection the recorded service
    // makes), values far past it, and the floor broken from below.
    let above = format!("Member must have value less than or equal to {MAX_RESULTS_CEILING}");
    let below = format!("Member must have value greater than or equal to {MAX_RESULTS_FLOOR}");
    let refused: [(&str, i32, &String); 5] = [
        ("one past the ceiling", MAX_RESULTS_CEILING + 1, &above),
        ("far past the ceiling", 100_000, &above),
        ("the largest integer", i32::MAX, &above),
        ("one below the floor", 0, &below),
        ("negative", -1, &below),
    ];
    for (case, size, requirement) in refused {
        let answer = aws.describe(json!({ "MaxResults": size })).await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({size}): expected 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case} ({size}): expected x-amzn-errortype: ValidationException, got {:?}",
            answer.error_code
        );
        let expected = one_violation(&size.to_string(), "maxResults", requirement);
        assert_eq!(
            answer.at("Message").as_str(),
            Some(expected.as_str()),
            "case {case} ({size}): expected Message {expected:?}, got {:?}",
            answer.body
        );
    }
}

/// One refused request and the shape whose modelled `@pattern` its member carries.
struct PatternCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member_path: &'static str,
    value: &'static str,
    /// The shape the member targets. Its `@pattern` is READ FROM the shipped model rather
    /// than written out here, so a row cannot pass against a re-spelled expression.
    shape: &'static str,
}

/// The `@pattern` the model declares for `shape`, read from the model this crate ships —
/// the one source for the expression the service must quote back.
fn declared_pattern(shape: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-synthetics-synthetics.json"
    );
    let text = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
    let model: Value = serde_json::from_str(&text).expect("the Smithy model is JSON");
    let id = format!("com.amazonaws.synthetics#{shape}");
    model["shapes"][&id]["traits"]["smithy.api#pattern"]
        .as_str()
        .unwrap_or_else(|| panic!("shape {shape} declares no @pattern in the shipped model"))
        .to_string()
}

/// A `@pattern` requirement is reported as the expression the model declares, anchors
/// included — for every pattern-constrained member, whatever the operation and whatever
/// the member's binding.
#[tokio::test]
async fn a_reported_pattern_is_the_declared_expression() {
    let aws = Synthetics::new("us-east-1");

    let cases = vec![
        // The recorded instance: an httpLabel member of a still-scaffolded operation. A
        // constraint belongs to the INPUT SHAPE, so the refusal does not wait for a
        // handler to exist.
        PatternCase {
            case: "tag-resource-arn",
            method: "POST",
            path: "/tags/not-an-arn".to_string(),
            body: Some(json!({ "Tags": { "team": "synthetics" } })),
            member_path: "resourceArn",
            value: "not-an-arn",
            shape: "ResourceArn",
        },
        // A BODY member of an implemented operation.
        PatternCase {
            case: "create-name",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with("placeholder", "Name", json!("Bad Name!"))),
            member_path: "name",
            value: "Bad Name!",
            shape: "CanaryName",
        },
        // A second body member, with a different expression.
        PatternCase {
            case: "create-execution-role-arn",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with(
                "pattern-role",
                "ExecutionRoleArn",
                json!("role/CanaryRole"),
            )),
            member_path: "executionRoleArn",
            value: "role/CanaryRole",
            shape: "RoleArn",
        },
        // A third, whose expression carries escapes and a character class.
        PatternCase {
            case: "create-kms-key-arn",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with(
                "pattern-kms",
                "KmsKeyArn",
                json!("arn:aws:kms:us-east-1:123456789012:alias/synthetics"),
            )),
            member_path: "kmsKeyArn",
            value: "arn:aws:kms:us-east-1:123456789012:alias/synthetics",
            shape: "KmsKeyArn",
        },
        // A body member of ANOTHER operation, targeting the canary-only ARN shape.
        PatternCase {
            case: "associate-resource-arn",
            method: "PATCH",
            path: "/group/live-group/associate".to_string(),
            body: Some(json!({ "ResourceArn": "not-an-arn" })),
            member_path: "resourceArn",
            value: "not-an-arn",
            shape: "CanaryArn",
        },
        // A URI-LABEL member on a read operation.
        PatternCase {
            case: "get-canary-name",
            method: "GET",
            path: "/canary/BADNAME".to_string(),
            body: None,
            member_path: "name",
            value: "BADNAME",
            shape: "CanaryName",
        },
        // A QUERY-STRING member.
        PatternCase {
            case: "get-canary-dry-run-id",
            method: "GET",
            path: "/canary/probe-canary?dryRunId=not-a-uuid".to_string(),
            body: None,
            member_path: "dryRunId",
            value: "not-a-uuid",
            shape: "UUID",
        },
    ];

    for PatternCase {
        case,
        method,
        path,
        body,
        member_path,
        value,
        shape,
    } in cases
    {
        let declared = declared_pattern(shape);
        // The row proves something only if the model's own expression IS anchored: that is
        // what the emulator used to strip.
        assert!(
            declared.starts_with('^') && declared.ends_with('$'),
            "case {case}: the modelled pattern {declared:?} is not anchored, so this row \
             asserts nothing"
        );

        let answer = aws.call(method, &path, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case}: expected x-amzn-errortype: ValidationException, got {:?}",
            answer.error_code
        );
        let expected = one_violation(
            value,
            member_path,
            &format!("Member must satisfy regular expression pattern: {declared}"),
        );
        assert_eq!(
            answer.at("Message").as_str(),
            Some(expected.as_str()),
            "case {case}: expected Message {expected:?}, got {:?}",
            answer.body
        );
    }
}
