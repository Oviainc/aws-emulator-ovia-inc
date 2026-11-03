// SPDX-License-Identifier: CC0-1.0

//! What this service answers for a request it refuses as invalid, end to end over the
//! wire (the same `MultiServiceRouter` the emulator boots, through the harness in
//! `canary_lifecycle`).
//!
//! Two rules are pinned here.
//!
//! 1. A request that breaks a rule the MODEL declares — a value outside a `@length`,
//!    `@range` or `@pattern` constraint, or a required member left out — is refused with
//!    this service's own modelled `ValidationException`: HTTP 400,
//!    `x-amzn-errortype: ValidationException`, and the shape's one modelled member,
//!    `Message`, carrying the constraint-violation envelope built from the member path,
//!    the offending value and the violated constraint. The rows below therefore span the
//!    classes the rule generalises over — constraint kind, member, binding (body, URI
//!    label, query string), operation, and violation count — and the recorded
//!    `CreateCanary` name is only one of them.
//!
//! 2. `ArtifactS3Location` must be a well-formed Amazon S3 location; a value that is not
//!    is refused with the same modelled `ValidationException` and leaves no canary
//!    behind, while a well-formed one round-trips through `GetCanary`.
//!
//! Every constraint quoted below is the model's own, read with
//! `smithy_query(query="operation",operation="…",model_path="services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json")`;
//! nothing here restates a rule the model does not carry.

use serde_json::{json, Value};

use super::canary_lifecycle::{create_body, pattern_requirement as pattern_clause, Synthetics};

/// `CanaryName`'s `@pattern` (`CreateCanary$Name`, `GetCanary$Name`), as the model
/// DECLARES it — the whole-value `^` / `$` pair included, which is what the operations that
/// declare `ValidationException` quote. See `tests/all/server_side_validation.rs` for the
/// two dialects and which operation takes which.
const CANARY_NAME_PATTERN: &str = r"^[0-9a-z_\-]+$";

/// `RoleArn`'s `@pattern` (`CreateCanary$ExecutionRoleArn`), likewise as declared.
const ROLE_ARN_PATTERN: &str = r"^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$";

/// `UUID`'s `@pattern` (`GetCanary$DryRunId`), likewise as declared.
const UUID_PATTERN: &str = r"^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$";

/// `ResourceArn`'s `@pattern` (`TagResource$ResourceArn`), likewise as declared. The
/// tagging operations, whose error set declares no `ValidationException`, quote it WITHOUT
/// the whole-value pair — see `tests/all/pattern_requirement_wording.rs`, which owns that
/// rule and both dialects.
const RESOURCE_ARN_PATTERN: &str = r"^arn:(aws[a-zA-Z-]*)?:synthetics:[a-z]{2,4}(-[a-z]{2,4})?-[a-z]+-\d{1}:\d{12}:(canary|group):[0-9a-z_\-]+$";

/// How Synthetics words a malformed artifact location (`src/state.rs`).
const ARTIFACT_LOCATION_MALFORMED: &str = "ArtifactS3Location: The location is malformed.";

/// One clause of the constraint envelope: the value as echoed (`None` for an absent
/// member), the member's path, and the requirement it failed.
fn clause(value: Option<&str>, path: &str, requirement: &str) -> String {
    match value {
        Some(shown) => format!(
            "Value '{shown}' at '{path}' failed to satisfy constraint: {requirement}"
        ),
        None => format!("Value null at '{path}' failed to satisfy constraint: {requirement}"),
    }
}

/// The envelope AWS wraps constraint clauses in: `1 validation error detected: …` for one,
/// `N validation errors detected: …; …` for several.
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The requirement clause an operation that declares `ValidationException` states: the
/// expression as the model declares it, character for character.
fn pattern_requirement(declared: &str) -> String {
    pattern_clause(declared)
}

/// The requirement clause an operation whose error set declares NO `ValidationException`
/// states — the three tagging operations, refused by the generic request validator ahead of
/// them, which publishes the expression without the model's whole-value pair
/// (`service_common::validation::unanchored`, the tree's one reading of that rule).
fn published_pattern_requirement(declared: &str) -> String {
    pattern_clause(&service_common::validation::unanchored(declared))
}

fn min_length_requirement(min: u32) -> String {
    format!("Member must have length greater than or equal to {min}")
}

fn min_value_requirement(min: u32) -> String {
    format!("Member must have value greater than or equal to {min}")
}

fn max_value_requirement(max: u32) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// One refused request: how it is sent, and the `Message` the modelled
/// `ValidationException` must carry.
struct Refused {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    message: String,
}

/// A `CreateCanary` body that satisfies every modelled constraint, with `member` replaced
/// by `value` — so each row differs from a valid request in exactly the one way it names.
fn create_with(name: &str, member: &str, value: Value) -> Value {
    let mut body = create_body(name);
    body[member] = value;
    body
}

#[tokio::test]
async fn a_refused_request_carries_the_modelled_validation_exception_message() {
    let aws = Synthetics::new("us-east-1");

    let mut cases = vec![
        // The recorded instance: a name outside `CanaryName`'s pattern, in the body.
        Refused {
            case: "create-name-pattern",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with(
                "placeholder",
                "Name",
                json!("Invalid_Canary_NAME!"),
            )),
            message: envelope(&[clause(
                Some("Invalid_Canary_NAME!"),
                "name",
                &pattern_requirement(CANARY_NAME_PATTERN),
            )]),
        },
        // A different member of the same operation, with a different pattern.
        Refused {
            case: "create-execution-role-arn-pattern",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with(
                "role-arn-case",
                "ExecutionRoleArn",
                json!("not-an-arn"),
            )),
            message: envelope(&[clause(
                Some("not-an-arn"),
                "executionRoleArn",
                &pattern_requirement(ROLE_ARN_PATTERN),
            )]),
        },
        // A different constraint KIND: `@length` on a string.
        Refused {
            case: "create-runtime-version-min-length",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with("runtime-case", "RuntimeVersion", json!(""))),
            message: envelope(&[clause(
                Some(""),
                "runtimeVersion",
                &min_length_requirement(1),
            )]),
        },
        // `@range` on an integer, below the published minimum (`MaxSize1024`, 1..1024).
        Refused {
            case: "create-success-retention-below-range",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with(
                "retention-low",
                "SuccessRetentionPeriodInDays",
                json!(0),
            )),
            message: envelope(&[clause(
                Some("0"),
                "successRetentionPeriodInDays",
                &min_value_requirement(1),
            )]),
        },
        // And one past the published maximum, on the other retention member.
        Refused {
            case: "create-failure-retention-above-range",
            method: "POST",
            path: "/canary".to_string(),
            body: Some(create_with(
                "retention-high",
                "FailureRetentionPeriodInDays",
                json!(1025),
            )),
            message: envelope(&[clause(
                Some("1025"),
                "failureRetentionPeriodInDays",
                &max_value_requirement(1024),
            )]),
        },
        // A required member left out: the other refusal class, found while READING the
        // request rather than while walking the value that was read.
        Refused {
            case: "create-missing-required-code",
            method: "POST",
            path: "/canary".to_string(),
            body: Some({
                let mut body = create_body("missing-code");
                body.as_object_mut()
                    .expect("the create body is a JSON object")
                    .remove("Code");
                body
            }),
            message: envelope(&[clause(None, "code", "Member must not be null")]),
        },
        // Two violations in one request: the plural envelope, clauses in schema order.
        Refused {
            case: "create-two-violations",
            method: "POST",
            path: "/canary".to_string(),
            body: Some({
                let mut body = create_with("placeholder", "Name", json!("BAD NAME"));
                body["ExecutionRoleArn"] = json!("nope");
                body
            }),
            message: envelope(&[
                clause(
                    Some("BAD NAME"),
                    "name",
                    &pattern_requirement(CANARY_NAME_PATTERN),
                ),
                clause(
                    Some("nope"),
                    "executionRoleArn",
                    &pattern_requirement(ROLE_ARN_PATTERN),
                ),
            ]),
        },
        // A URI-LABEL-bound member, on a read operation.
        Refused {
            case: "get-name-pattern-uri-label",
            method: "GET",
            path: "/canary/BADNAME".to_string(),
            body: None,
            message: envelope(&[clause(
                Some("BADNAME"),
                "name",
                &pattern_requirement(CANARY_NAME_PATTERN),
            )]),
        },
        // A QUERY-STRING-bound member.
        Refused {
            case: "get-dry-run-id-pattern-query",
            method: "GET",
            path: "/canary/probe-canary?dryRunId=not-a-uuid".to_string(),
            body: None,
            message: envelope(&[clause(
                Some("not-a-uuid"),
                "dryRunId",
                &pattern_requirement(UUID_PATTERN),
            )]),
        },
        // A paging member on a different operation, past the ceiling the SERVICE applies
        // (1000) rather than the modelled `MaxCanaryResults` maximum of 20, which the
        // recorded service does not enforce — see `tests/all/server_side_validation.rs`.
        Refused {
            case: "describe-max-results-above-ceiling",
            method: "POST",
            path: "/canaries".to_string(),
            body: Some(json!({ "MaxResults": 1001 })),
            message: envelope(&[clause(
                Some("1001"),
                "maxResults",
                &max_value_requirement(1000),
            )]),
        },
        // A tag operation's ARN: a constraint belongs to the INPUT SHAPE, so the refusal
        // comes before the handler and before any lookup in the store. `TagResource`'s own
        // error set declares no `ValidationException`, so the refusal is the generic
        // validator's and quotes the expression WITHOUT the model's whole-value pair.
        Refused {
            case: "tag-resource-arn-pattern",
            method: "POST",
            path: "/tags/not-an-arn".to_string(),
            body: Some(json!({ "Tags": { "team": "synthetics" } })),
            message: envelope(&[clause(
                Some("not-an-arn"),
                "resourceArn",
                &published_pattern_requirement(RESOURCE_ARN_PATTERN),
            )]),
        },
    ];

    // A value one past `CanaryName`'s published maximum length (255), built here so the
    // echoed value stays the value itself.
    let too_long = "a".repeat(256);
    cases.push(Refused {
        case: "create-name-above-max-length",
        method: "POST",
        path: "/canary".to_string(),
        body: Some(create_with("placeholder", "Name", json!(too_long.clone()))),
        message: envelope(&[clause(
            Some(&too_long),
            "name",
            "Member must have length less than or equal to 255",
        )]),
    });

    for Refused {
        case,
        method,
        path,
        body,
        message,
    } in cases
    {
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
        assert_eq!(
            answer.at("__type").as_str(),
            Some("ValidationException"),
            "case {case}: expected the modelled error shape in __type, got {:?}",
            answer.body
        );
        // The modelled member is `Message`; a client reads the failure through it, so a
        // body carrying the text anywhere else is the divergence this suite exists for.
        assert_eq!(
            answer.at("Message").as_str(),
            Some(message.as_str()),
            "case {case}: expected Message {message:?}, got {:?}",
            answer.body
        );
    }
}

/// The gate in front of every operation is transparent to a request the model accepts:
/// an implemented operation serves it and a scaffolded one still answers its 501.
#[tokio::test]
async fn a_request_the_model_accepts_reaches_its_operation() {
    let aws = Synthetics::new("us-east-1");

    let created = aws.create(create_body("gate-transparent")).await;
    assert_eq!(
        created.status, 200,
        "case valid-create: expected 200, got {} ({:?})",
        created.status, created.body
    );

    // A model-valid request to an operation that has no implementation yet must still
    // reach the scaffold channel rather than being refused as invalid. `ListGroups` is
    // one of the operations still carrying the scaffold marker.
    let stub = aws.call("POST", "/groups", Some(json!({}))).await;
    assert_eq!(
        stub.status, 501,
        "case valid-request-to-a-stub: expected the scaffold's 501, got {} ({:?})",
        stub.status, stub.body
    );
}

/// `ArtifactS3Location` is "The location in Amazon S3 where Synthetics stores artifacts
/// … The name of the Amazon S3 bucket can't include a period (.)", so a value that is not
/// a well-formed S3 location is refused — and nothing is stored for it.
///
/// The bucket-name rows follow Amazon S3's published general-purpose bucket naming rules
/// (3–63 characters, lowercase letters, numbers, periods and hyphens, beginning and
/// ending with a letter or number, no reserved prefix or suffix), each bound tested at
/// the bound and one past it.
#[tokio::test]
async fn a_malformed_artifact_location_is_refused_and_a_well_formed_one_round_trips() {
    let aws = Synthetics::new("us-east-1");

    let sixty_three = "b".repeat(63);
    let sixty_four = "b".repeat(64);
    let malformed: Vec<(&str, String)> = vec![
        ("no-scheme", "artifacts/canary".to_string()),
        ("wrong-scheme", "https://artifacts/canary".to_string()),
        ("scheme-only", "s3://".to_string()),
        // The member's own rule: the bucket name can't include a period.
        ("bucket-with-a-period", "s3://my.artifacts/canary".to_string()),
        // One short of S3's three-character minimum.
        ("bucket-too-short", "s3://ab/canary".to_string()),
        // One past S3's sixty-three-character maximum.
        ("bucket-too-long", format!("s3://{sixty_four}/canary")),
        ("bucket-uppercase", "s3://Artifacts/canary".to_string()),
        ("bucket-underscore", "s3://my_artifacts/canary".to_string()),
        ("bucket-leading-hyphen", "s3://-artifacts/canary".to_string()),
        ("bucket-trailing-hyphen", "s3://artifacts-/canary".to_string()),
        // Reserved prefix and suffix, neither of which the issue cited.
        ("bucket-reserved-prefix", "s3://xn--artifacts/canary".to_string()),
        ("bucket-reserved-suffix", "s3://artifacts-s3alias/canary".to_string()),
    ];

    for (case, location) in malformed {
        let name = format!("bad-{case}");
        let answer = aws
            .create(create_with(&name, "ArtifactS3Location", json!(location)))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({location:?}): expected 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case} ({location:?}): expected x-amzn-errortype: ValidationException, got {:?}",
            answer.error_code
        );
        assert_eq!(
            answer.at("Message").as_str(),
            Some(ARTIFACT_LOCATION_MALFORMED),
            "case {case} ({location:?}): expected Message {ARTIFACT_LOCATION_MALFORMED:?}, got {:?}",
            answer.body
        );
        // A refused create leaves no record behind.
        let read = aws.get(&name).await;
        assert_eq!(
            read.status, 404,
            "case {case} ({location:?}): a refused create must store nothing, got {} ({:?})",
            read.status, read.body
        );
    }

    let well_formed: Vec<(&str, String)> = vec![
        ("bucket-and-prefix", "s3://artifacts/canary".to_string()),
        // At S3's three-character minimum, with no key prefix at all.
        ("bucket-at-min-length", "s3://abc".to_string()),
        // At S3's sixty-three-character maximum, with a deep prefix.
        ("bucket-at-max-length", format!("s3://{sixty_three}/deep/prefix/2026")),
        (
            "bucket-with-digits-and-hyphens",
            "s3://cw-syn-results-123456789012-us-east-1/canary/2026".to_string(),
        ),
        ("bucket-ending-in-a-digit", "s3://artifacts1".to_string()),
    ];

    for (case, location) in well_formed {
        let name = format!("good-{case}");
        let created = aws
            .create(create_with(&name, "ArtifactS3Location", json!(location.clone())))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case} ({location:?}): expected 200, got {} ({:?})",
            created.status, created.body
        );
        // Read it back through the implemented reader: the location the create accepted
        // is the location the store holds.
        let read = aws.get(&name).await;
        assert_eq!(
            read.status, 200,
            "case {case} ({location:?}): expected the canary to be readable, got {} ({:?})",
            read.status, read.body
        );
        assert_eq!(
            read.at("Canary.ArtifactS3Location").as_str(),
            Some(location.as_str()),
            "case {case}: expected the stored artifact location {location:?}, got {:?}",
            read.at("Canary.ArtifactS3Location")
        );
    }
}
