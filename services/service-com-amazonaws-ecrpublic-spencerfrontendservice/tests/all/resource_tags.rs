// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, driven end to end over the wire.
//!
//! The rule under test: the tags of the repository a `resourceArn` names are the tags
//! that were persisted for it, reported in key order as the modelled `TagList`; a
//! `resourceArn` that is not a well-formed ECR Public repository ARN is refused as an
//! invalid parameter, and a well-formed one that names no repository of this registry is
//! refused as a missing repository.
//!
//! Every case goes through the same front door a client uses (`POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>`), reusing the router harness in
//! [`super::repository_image_lifecycle`], so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass. The tables span the
//! classes the rule generalises over — the size, ordering and shape of a tag set, every
//! way an ARN can be malformed, and every way a well-formed ARN can name nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::repository_image_lifecycle::{ok, refused, service, ARN_PREFIX};

/// The message the real service answers for a `resourceArn` that fails the one
/// constraint that member carries: the parameter-constraint form, naming the member and
/// the constraint it failed. The recorded `ecr-public.list-tags-invalid-arn` unit asserts
/// exactly this text, so it is spelled out here rather than imported from the code under
/// test.
const INVALID_ARN_MESSAGE: &str =
    "Invalid parameter at 'resourceArn' failed to satisfy constraint: 'Invalid ARN'";

/// The `message` member of an error body, whichever casing the protocol emitted.
fn error_message(body: &Value, case: &str) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// The `tags` of a `ListTagsForResource` response as `(Key, Value)` pairs, in the order
/// the response listed them.
fn tag_pairs(body: &Value, case: &str) -> Vec<(String, String)> {
    body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list in {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a tag Key in {body}"))
                    .to_string(),
                tag["Value"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a tag Value in {body}"))
                    .to_string(),
            )
        })
        .collect()
}

/// One repository and the tag set it was created with.
struct TagCase {
    case: &'static str,
    repository: String,
    /// The `tags` member of the `CreateRepository` request, exactly as a client sends it.
    sent: Value,
    /// What `ListTagsForResource` must report: every stored pair, in key order.
    expected: Vec<(String, String)>,
}

/// The tag sets a repository can carry, spanning what the rule generalises over: no tag
/// set at all, an empty one, one pair, a value that is empty (`Tag$Value` is modelled
/// `@length(0..256)`, so the empty string is a value), a pair sent without a value, keys
/// that arrive out of order, and the largest set a repository may hold (50, the limit
/// `TooManyTagsException` documents). The repository names also span the forms the
/// modelled `repositoryName` pattern admits, including a namespaced one, so the `/` in a
/// name is parsed back out of the ARN.
fn tag_cases() -> Vec<TagCase> {
    let mut cases = vec![
        TagCase {
            case: "no tags member at all",
            repository: "untagged".to_string(),
            sent: Value::Null,
            expected: Vec::new(),
        },
        TagCase {
            case: "an empty tag list",
            repository: "emptylist".to_string(),
            sent: json!([]),
            expected: Vec::new(),
        },
        TagCase {
            case: "a single pair",
            repository: "single".to_string(),
            sent: json!([{"Key": "team", "Value": "platform"}]),
            expected: vec![("team".to_string(), "platform".to_string())],
        },
        TagCase {
            case: "an explicitly empty value",
            repository: "emptyvalue".to_string(),
            sent: json!([{"Key": "marker", "Value": ""}]),
            expected: vec![("marker".to_string(), String::new())],
        },
        TagCase {
            case: "a pair sent without a value",
            repository: "value.less_name-1".to_string(),
            sent: json!([{"Key": "marker"}]),
            expected: vec![("marker".to_string(), String::new())],
        },
        TagCase {
            case: "keys sent out of order",
            repository: "team/app".to_string(),
            sent: json!([
                {"Key": "zebra", "Value": "last"},
                {"Key": "Alpha", "Value": "upper"},
                {"Key": "middle", "Value": "mid"},
            ]),
            // Key order, which is byte order: an upper-case key sorts before the
            // lower-case ones.
            expected: vec![
                ("Alpha".to_string(), "upper".to_string()),
                ("middle".to_string(), "mid".to_string()),
                ("zebra".to_string(), "last".to_string()),
            ],
        },
        TagCase {
            case: "a repeated key keeps the last value",
            repository: "deep/nested/name".to_string(),
            sent: json!([
                {"Key": "env", "Value": "staging"},
                {"Key": "env", "Value": "production"},
            ]),
            expected: vec![("env".to_string(), "production".to_string())],
        },
    ];

    // The largest set a repository may carry: 50 pairs, keyed so their key order is not
    // their insertion order.
    let many: Vec<(String, String)> = (0..50)
        .map(|i| (format!("key-{i:02}"), format!("value-{i}")))
        .collect();
    let mut sent: Vec<(String, String)> = many.clone();
    sent.reverse();
    cases.push(TagCase {
        case: "the 50-tag limit",
        repository: "fifty".to_string(),
        sent: Value::Array(
            sent.iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        ),
        expected: many,
    });
    cases
}

/// Create `case.repository` with `case.sent`, so the tags under test reach the store the
/// only way a client can put them there.
async fn create(router: &MultiServiceRouter, case: &TagCase) -> String {
    let mut body = json!({"repositoryName": case.repository});
    if !case.sent.is_null() {
        body["tags"] = case.sent.clone();
    }
    let created = ok(router, "CreateRepository", body, case.case).await;
    created["repository"]["repositoryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: expected a repositoryArn in {created}", case.case))
        .to_string()
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tag_set_that_was_persisted() {
    let router = service();

    for case in tag_cases() {
        let arn = create(&router, &case).await;
        assert_eq!(
            arn,
            format!("{ARN_PREFIX}{}", case.repository),
            "case {:?}: expected the repository's own ARN, got {arn:?}",
            case.case
        );

        // The ARN the create reported is the one the reader resolves.
        let listed = ok(
            &router,
            "ListTagsForResource",
            json!({"resourceArn": arn}),
            case.case,
        )
        .await;
        let actual = tag_pairs(&listed, case.case);
        assert_eq!(
            actual, case.expected,
            "case {:?}: expected {:?}, got {actual:?}",
            case.case, case.expected
        );

        // A read changes nothing a later caller sees: the second answer is the first.
        let again = ok(
            &router,
            "ListTagsForResource",
            json!({"resourceArn": format!("{ARN_PREFIX}{}", case.repository)}),
            case.case,
        )
        .await;
        assert_eq!(
            again, listed,
            "case {:?}: expected a repeated read to answer {listed}, got {again}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_region_segment_of_a_repository_arn_is_not_what_names_the_resource() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app", "tags": [{"Key": "env", "Value": "prod"}]}),
        "setup",
    )
    .await;

    // ECR Public is partition-wide, so the ARNs it mints carry no region; a caller that
    // supplies one still names the same repository.
    for (case, arn) in [
        ("no region, as minted", "arn:aws:ecr-public::000000000000:repository/team/app"),
        ("a region segment", "arn:aws:ecr-public:us-east-1:000000000000:repository/team/app"),
        ("another region segment", "arn:aws:ecr-public:eu-west-1:000000000000:repository/team/app"),
    ] {
        let listed = ok(
            &router,
            "ListTagsForResource",
            json!({"resourceArn": arn}),
            case,
        )
        .await;
        let actual = tag_pairs(&listed, case);
        assert_eq!(
            actual,
            vec![("env".to_string(), "prod".to_string())],
            "case {case:?}: expected the stored tag, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_arn_that_is_not_a_repository_arn_is_an_invalid_parameter() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "setup",
    )
    .await;

    // Every way the six-field ARN form
    // `arn:<partition>:ecr-public:<region>:<account>:repository/<name>` can be missed.
    // The repository named by the last few rows DOES exist, so only the ARN's form is
    // what refuses them.
    for (case, arn) in [
        ("a bare repository name", "team/app".to_string()),
        ("a URI instead of an ARN", "public.ecr.aws/emulator/team/app".to_string()),
        ("too few fields", "arn:aws:ecr-public::000000000000".to_string()),
        ("no resource field", "arn:aws:ecr-public::000000000000:".to_string()),
        (
            "a resource type with no name",
            ARN_PREFIX.to_string(),
        ),
        (
            "a resource type that is not repository",
            "arn:aws:ecr-public::000000000000:registry/team/app".to_string(),
        ),
        (
            "the private ECR service",
            "arn:aws:ecr:us-east-1:000000000000:repository/team/app".to_string(),
        ),
        (
            "another service entirely",
            "arn:aws:s3:::team/app".to_string(),
        ),
        (
            "an empty partition",
            "arn::ecr-public::000000000000:repository/team/app".to_string(),
        ),
        (
            "an empty account",
            "arn:aws:ecr-public:::repository/team/app".to_string(),
        ),
        (
            "the arn prefix miscased",
            format!("ARN{}", &format!("{ARN_PREFIX}team/app")[3..]),
        ),
        (
            "leading whitespace",
            format!(" {ARN_PREFIX}team/app"),
        ),
    ] {
        let body = refused(
            &router,
            "ListTagsForResource",
            json!({"resourceArn": arn}),
            "InvalidParameterException",
            case,
        )
        .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, INVALID_ARN_MESSAGE,
            "case {case:?}: expected the resourceArn constraint message, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_well_formed_arn_naming_no_repository_of_this_registry_is_not_found() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "setup",
    )
    .await;

    for (case, arn) in [
        ("a name that was never created", format!("{ARN_PREFIX}team/missing")),
        (
            "a prefix of a stored name",
            format!("{ARN_PREFIX}team"),
        ),
        (
            "another account's registry",
            "arn:aws:ecr-public::123456789012:repository/team/app".to_string(),
        ),
        (
            "another partition",
            "arn:aws-cn:ecr-public::000000000000:repository/team/app".to_string(),
        ),
        (
            "the registry alias in the account field",
            "arn:aws:ecr-public::emulator:repository/team/app".to_string(),
        ),
    ] {
        refused(
            &router,
            "ListTagsForResource",
            json!({"resourceArn": arn}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
    }

    // The stored repository answers until it is deleted, and is not found after.
    let arn = format!("{ARN_PREFIX}team/app");
    ok(
        &router,
        "ListTagsForResource",
        json!({"resourceArn": arn}),
        "before the delete",
    )
    .await;
    ok(
        &router,
        "DeleteRepository",
        json!({"repositoryName": "team/app"}),
        "delete",
    )
    .await;
    refused(
        &router,
        "ListTagsForResource",
        json!({"resourceArn": arn}),
        "RepositoryNotFoundException",
        "after the delete",
    )
    .await;
}

#[tokio::test]
async fn an_arn_outside_the_modelled_length_is_refused_before_the_handler_runs() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "setup",
    )
    .await;

    // `resourceArn` is modelled `@length(1..2048)`; the framework refuses both bounds'
    // violations itself, so neither reaches the handler and neither is a 501. The second
    // row is a well-formed ARN for a repository that EXISTS, so only its length is what
    // refuses it — and the message is the framework's constraint description, not either
    // message this handler words.
    for (case, arn) in [
        ("the empty string", String::new()),
        (
            "past the maximum, otherwise resolvable",
            format!("{ARN_PREFIX}team/app{}", "0".repeat(2048)),
        ),
    ] {
        let body = refused(
            &router,
            "ListTagsForResource",
            json!({"resourceArn": arn}),
            "InvalidParameterException",
            case,
        )
        .await;
        let message = error_message(&body, case);
        assert!(
            message.contains("'resourceArn'")
                && message.contains("failed to satisfy constraint")
                && message.contains("length"),
            "case {case:?}: expected the modelled length constraint to refuse it, \
             got {message:?}"
        );
        assert_ne!(
            message, INVALID_ARN_MESSAGE,
            "case {case:?}: expected the constraint check, not the handler's refusal"
        );
    }

    // The same read with an in-range ARN still works, so the two rows above were refused
    // for their length alone.
    let listed = ok(
        &router,
        "ListTagsForResource",
        json!({"resourceArn": format!("{ARN_PREFIX}team/app")}),
        "in range",
    )
    .await;
    let actual = tag_pairs(&listed, "in range");
    assert!(
        actual.is_empty(),
        "case \"in range\": expected no tags, got {actual:?}"
    );
}
