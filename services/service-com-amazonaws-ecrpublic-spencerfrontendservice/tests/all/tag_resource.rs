// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, driven end to end over the wire.
//!
//! The rule under test: `TagResource` associates the request's tags with the repository
//! its `resourceArn` names, keyed by tag key — a key the repository already carries takes
//! the requested value, a key it does not carry is added, and a key the request does not
//! name is left exactly as it was ("if existing tags on a resource aren't specified in the
//! request parameters, they aren't changed"). A tag key in the namespace AWS keeps for its
//! own tags (`aws:`) is a system tag an API caller may not write, a write that would leave
//! the repository carrying more than the 50 tags `TooManyTagsException` documents is
//! refused, a `resourceArn` that is not a well-formed ECR Public repository ARN is an
//! invalid parameter and a well-formed one naming no repository of this registry is a
//! missing repository. A refused call writes nothing at all.
//!
//! Every case goes through the same front door a client uses (`POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>`), reusing the router harness in
//! [`super::repository_image_lifecycle`], and every tag set is read back through
//! `ListTagsForResource`, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.
//!
//! The tables span the classes the rule generalises over: the size, ordering and shape of
//! the tag set a request carries and of the set the repository already holds, the casing
//! and position of the reserved prefix in a key, the tag limit at the bound and one past
//! it (reached both in one call and across calls), every way an ARN can be malformed, and
//! every way a well-formed ARN can name nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::error_message;
use super::repository_image_lifecycle::{ok, refused, service, ARN_PREFIX};

/// The fixed text ECR Public answers for a tag key in the namespace it keeps for its own
/// tags. Spelled out here rather than imported from the code under test.
const SYSTEM_TAG_MESSAGE: &str = "Caller is an end user and not allowed to mutate system tags";

/// The fixed text `TooManyTagsException` carries.
const TOO_MANY_TAGS_MESSAGE: &str = "The number of tags exceeds the limit";

/// The clause a tagging write answers for a `resourceArn` it cannot parse.
const UNPARSEABLE_ARN_MESSAGE: &str =
    "Invalid parameter at 'resourceArn' failed to satisfy constraint: 'Unable to parse ARN'";

/// The most tags a repository may carry: the limit `TooManyTagsException` documents ("the
/// maximum number of tags that can be applied to a repository is 50").
const MAX_TAGS: usize = 50;

/// A `tags` list, as a client sends it.
fn tags(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The tag set `ListTagsForResource` reports for `arn`, as `(Key, Value)` pairs in the
/// order the response listed them.
async fn listed(router: &MultiServiceRouter, arn: &str, case: &str) -> Vec<(String, String)> {
    let body = ok(
        router,
        "ListTagsForResource",
        json!({"resourceArn": arn}),
        case,
    )
    .await;
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

/// Create `repository` with `initial` and answer its ARN, so the tags a case starts from
/// reach the store the only way a client can put them there.
async fn given_repository(
    router: &MultiServiceRouter,
    repository: &str,
    initial: &[(&str, &str)],
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateRepository",
        json!({"repositoryName": repository, "tags": tags(initial)}),
        case,
    )
    .await;
    created["repository"]["repositoryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a repositoryArn in {created}"))
        .to_string()
}

/// One repository, the tags it was created with, the tags one `TagResource` call adds and
/// the set the repository must carry afterwards.
struct MergeCase {
    case: &'static str,
    repository: &'static str,
    initial: Vec<(&'static str, &'static str)>,
    /// The `tags` member of the `TagResource` request, exactly as a client sends it.
    sent: Value,
    expected: Vec<(&'static str, &'static str)>,
}

/// The merges a single `TagResource` call can make, spanning what the rule generalises
/// over: the size of the set the repository already holds (none, one, several) and of the
/// one the request carries (none, one, several), whether a requested key is new or already
/// held, the ordering of the keys in the request, a key repeated inside one request, a
/// value that is empty and a value sent without the member at all, and the forms a
/// repository name takes (flat, dotted, namespaced).
fn merge_cases() -> Vec<MergeCase> {
    vec![
        MergeCase {
            case: "the first tags of an untagged repository",
            repository: "untagged",
            initial: Vec::new(),
            sent: tags(&[("team", "platform")]),
            expected: vec![("team", "platform")],
        },
        MergeCase {
            case: "an empty list changes nothing",
            repository: "emptylist",
            initial: vec![("env", "prod")],
            sent: json!([]),
            expected: vec![("env", "prod")],
        },
        MergeCase {
            case: "a new key is added beside the ones held",
            repository: "team/app",
            initial: vec![("env", "prod")],
            sent: tags(&[("owner", "platform")]),
            expected: vec![("env", "prod"), ("owner", "platform")],
        },
        MergeCase {
            case: "a key already held takes the requested value",
            repository: "overwrite",
            initial: vec![("env", "staging"), ("owner", "platform")],
            sent: tags(&[("env", "production")]),
            expected: vec![("env", "production"), ("owner", "platform")],
        },
        MergeCase {
            case: "several keys at once, sent out of key order",
            repository: "deep/nested/name",
            initial: vec![("env", "prod")],
            sent: tags(&[("zebra", "last"), ("Alpha", "upper"), ("middle", "mid")]),
            // Key order, which is byte order: an upper-case key sorts first.
            expected: vec![
                ("Alpha", "upper"),
                ("env", "prod"),
                ("middle", "mid"),
                ("zebra", "last"),
            ],
        },
        MergeCase {
            case: "a key repeated inside one request keeps the last value",
            repository: "repeated.key_name-1",
            initial: Vec::new(),
            sent: tags(&[("env", "staging"), ("env", "production")]),
            expected: vec![("env", "production")],
        },
        MergeCase {
            case: "an explicitly empty value",
            repository: "emptyvalue",
            initial: vec![("marker", "set")],
            // `Tag$Value` is modelled `@length(0..256)`, so the empty string is a value.
            sent: tags(&[("marker", "")]),
            expected: vec![("marker", "")],
        },
        MergeCase {
            case: "a pair sent without a value member",
            repository: "valueless",
            initial: Vec::new(),
            sent: json!([{"Key": "marker"}]),
            expected: vec![("marker", "")],
        },
        MergeCase {
            case: "keys and values outside ASCII",
            repository: "unicode",
            initial: vec![("cost center", "platform")],
            sent: tags(&[("proyecto", "año-2026"), ("チーム", "基盤")]),
            expected: vec![
                ("cost center", "platform"),
                ("proyecto", "año-2026"),
                ("チーム", "基盤"),
            ],
        },
    ]
}

#[tokio::test]
async fn tag_resource_merges_the_requested_tags_into_the_ones_the_repository_holds() {
    let router = service();

    for case in merge_cases() {
        let arn = given_repository(&router, case.repository, &case.initial, case.case).await;
        assert_eq!(
            arn,
            format!("{ARN_PREFIX}{}", case.repository),
            "case {:?}: expected the repository's own ARN, got {arn:?}",
            case.case
        );

        let answered = ok(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": case.sent}),
            case.case,
        )
        .await;
        // `TagResourceResponse` models no members: the 200 itself is the whole answer.
        assert_eq!(
            answered,
            json!({}),
            "case {:?}: expected an empty response body, got {answered}",
            case.case
        );

        let expected: Vec<(String, String)> = case
            .expected
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let actual = listed(&router, &arn, case.case).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            case.case
        );

        // Tagging again with what the repository now carries leaves it there: the write is
        // a merge by key, so repeating it is not what adds a second copy of a tag.
        ok(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(&case.expected)}),
            case.case,
        )
        .await;
        let again = listed(&router, &arn, case.case).await;
        assert_eq!(
            again, expected,
            "case {:?}: expected a repeated write to leave {expected:?}, got {again:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn tags_added_by_separate_calls_all_survive() {
    let router = service();
    let arn = given_repository(&router, "merge/target", &[("created", "by-create")], "setup").await;

    // Each call names only its own key, so the set the repository carries is the union of
    // what every caller added: one caller's write does not drop another's tag.
    for (case, key, value) in [
        ("first caller", "owner", "platform"),
        ("second caller", "env", "prod"),
        ("third caller", "cost-center", "cc-1"),
    ] {
        ok(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(&[(key, value)])}),
            case,
        )
        .await;
    }

    let actual = listed(&router, &arn, "after three callers").await;
    let expected: Vec<(String, String)> = [
        ("cost-center", "cc-1"),
        ("created", "by-create"),
        ("env", "prod"),
        ("owner", "platform"),
    ]
    .iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    assert_eq!(
        actual, expected,
        "case \"after three callers\": expected {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn a_system_tag_key_is_not_one_a_caller_may_write() {
    let router = service();
    let arn = given_repository(&router, "system/tags", &[("env", "prod")], "setup").await;

    // The reserved namespace is `aws:` exactly: the rows span the prefix on its own, the
    // prefix with a tail, and a key whose further segments are colon-separated too.
    for (case, key) in [
        ("the reserved prefix alone", "aws:"),
        ("a key under the reserved prefix", "aws:cost-center"),
        (
            "a key under a reserved sub-namespace",
            "aws:cloudformation:stack-name",
        ),
    ] {
        let body = refused(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(&[(key, "value")])}),
            "InvalidTagParameterException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message, SYSTEM_TAG_MESSAGE,
            "case {case:?}: expected the system-tag refusal, got {message:?}"
        );
    }

    // A refused call writes nothing, not even the tags of the request it could have
    // stored: the whole list is refused together.
    let body = refused(
        &router,
        "TagResource",
        json!({
            "resourceArn": arn,
            "tags": tags(&[("owner", "platform"), ("aws:internal", "x")]),
        }),
        "InvalidTagParameterException",
        "a reserved key beside an ordinary one",
    )
    .await;
    let message = error_message(&body);
    assert_eq!(
        message, SYSTEM_TAG_MESSAGE,
        "case \"a reserved key beside an ordinary one\": expected the system-tag refusal, \
         got {message:?}"
    );
    let actual = listed(&router, &arn, "after the refusals").await;
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after the refusals\": expected the repository's tags to be untouched, \
         got {actual:?}"
    );

    // A key that only RESEMBLES the reserved namespace is an ordinary key: the prefix is a
    // literal, so another casing of it, the three letters without the colon, and the
    // prefix anywhere but the start are all the caller's own.
    for (case, key) in [
        ("the reserved prefix in upper case", "AWS:team"),
        ("the reserved prefix mixed case", "Aws:team"),
        ("the three letters without the colon", "aws"),
        ("the letters with an ordinary tail", "awsteam"),
        ("the prefix after another segment", "team/aws:owner"),
        ("the prefix at the end of the key", "owner-aws:"),
    ] {
        ok(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(&[(key, "value")])}),
            case,
        )
        .await;
        let actual = listed(&router, &arn, case).await;
        assert!(
            actual.contains(&(key.to_string(), "value".to_string())),
            "case {case:?}: expected the tag to be stored, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_create_may_not_carry_a_system_tag_key_either() {
    let router = service();

    // The rule is about the tag key, not about which call carries it, so the create that
    // also takes `tags` refuses the same key the same way.
    let body = refused(
        &router,
        "CreateRepository",
        json!({"repositoryName": "created/with-system-tag", "tags": tags(&[("aws:internal", "x")])}),
        "InvalidTagParameterException",
        "a create under the reserved prefix",
    )
    .await;
    let message = error_message(&body);
    assert_eq!(
        message, SYSTEM_TAG_MESSAGE,
        "case \"a create under the reserved prefix\": expected the system-tag refusal, \
         got {message:?}"
    );

    // And the repository the refused create named was not created.
    refused(
        &router,
        "ListTagsForResource",
        json!({"resourceArn": format!("{ARN_PREFIX}created/with-system-tag")}),
        "RepositoryNotFoundException",
        "after the refused create",
    )
    .await;
}

/// A tag set of `count` pairs, keyed so that key order is not insertion order.
fn filler_tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .rev()
            .map(|i| json!({"Key": format!("key-{i:03}"), "Value": format!("value-{i}")}))
            .collect(),
    )
}

#[tokio::test]
async fn a_write_that_would_pass_the_tag_limit_is_refused_whole() {
    let router = service();

    // The limit is on what the repository ENDS UP carrying, so the rows differ along how
    // the count is reached: in one call, across two calls, and by re-tagging keys that are
    // already there (which replaces rather than adds). Each bound is tested at the bound,
    // which must be accepted, and one past it, which must not be.
    let arn = given_repository(&router, "limit/one-call", &[], "one call").await;
    ok(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": filler_tags(MAX_TAGS)}),
        "the limit in one call",
    )
    .await;
    let at_limit = listed(&router, &arn, "the limit in one call").await;
    assert_eq!(
        at_limit.len(),
        MAX_TAGS,
        "case \"the limit in one call\": expected {MAX_TAGS} tags, got {}",
        at_limit.len()
    );

    // Re-tagging the keys it already carries keeps it at the limit rather than doubling.
    ok(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": filler_tags(MAX_TAGS)}),
        "re-tagging at the limit",
    )
    .await;
    let still = listed(&router, &arn, "re-tagging at the limit").await;
    assert_eq!(
        still, at_limit,
        "case \"re-tagging at the limit\": expected the same {MAX_TAGS} tags, got {still:?}"
    );

    // One key past the limit, added to a repository already holding the limit.
    let body = refused(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": tags(&[("one-too-many", "x")])}),
        "TooManyTagsException",
        "one key past the limit",
    )
    .await;
    let message = error_message(&body);
    assert_eq!(
        message, TOO_MANY_TAGS_MESSAGE,
        "case \"one key past the limit\": expected the limit refusal, got {message:?}"
    );
    let unchanged = listed(&router, &arn, "one key past the limit").await;
    assert_eq!(
        unchanged, at_limit,
        "case \"one key past the limit\": expected the tag set to be untouched, \
         got {unchanged:?}"
    );

    // Past the limit inside a single call, against an untagged repository.
    let fresh = given_repository(&router, "limit/single-request", &[], "a single request").await;
    let body = refused(
        &router,
        "TagResource",
        json!({"resourceArn": fresh, "tags": filler_tags(MAX_TAGS + 1)}),
        "TooManyTagsException",
        "the limit passed in one call",
    )
    .await;
    let message = error_message(&body);
    assert_eq!(
        message, TOO_MANY_TAGS_MESSAGE,
        "case \"the limit passed in one call\": expected the limit refusal, got {message:?}"
    );
    let none = listed(&router, &fresh, "the limit passed in one call").await;
    assert!(
        none.is_empty(),
        "case \"the limit passed in one call\": expected nothing written, got {none:?}"
    );

    // And the same count reached across two calls: the tags the repository was created
    // with count too.
    let split = given_repository(
        &router,
        "limit/across-calls",
        &[("created", "by-create")],
        "across calls",
    )
    .await;
    ok(
        &router,
        "TagResource",
        json!({"resourceArn": split, "tags": filler_tags(MAX_TAGS - 1)}),
        "up to the limit across calls",
    )
    .await;
    refused(
        &router,
        "TagResource",
        json!({"resourceArn": split, "tags": tags(&[("past", "the-limit")])}),
        "TooManyTagsException",
        "one key past the limit across calls",
    )
    .await;
    let across = listed(&router, &split, "one key past the limit across calls").await;
    assert_eq!(
        across.len(),
        MAX_TAGS,
        "case \"one key past the limit across calls\": expected {MAX_TAGS} tags, got {}",
        across.len()
    );
}

#[tokio::test]
async fn a_resource_arn_a_tagging_write_cannot_parse_is_an_invalid_parameter() {
    let router = service();
    given_repository(&router, "team/app", &[("env", "prod")], "setup").await;

    // Every way the six-field ARN form
    // `arn:<partition>:ecr-public:<region>:<account>:repository/<name>` can be missed. The
    // repository named by the later rows DOES exist, so only the ARN's form refuses them.
    for (case, arn) in [
        ("a bare repository name", "team/app".to_string()),
        (
            "a URI instead of an ARN",
            "public.ecr.aws/emulator/team/app".to_string(),
        ),
        ("too few fields", "arn:aws:ecr-public::000000000000".to_string()),
        ("no resource field", "arn:aws:ecr-public::000000000000:".to_string()),
        ("a resource type with no name", ARN_PREFIX.to_string()),
        (
            "a resource type that is not repository",
            "arn:aws:ecr-public::000000000000:registry/team/app".to_string(),
        ),
        (
            "the private ECR service",
            "arn:aws:ecr:us-east-1:000000000000:repository/team/app".to_string(),
        ),
        ("another service entirely", "arn:aws:s3:::team/app".to_string()),
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
        ("leading whitespace", format!(" {ARN_PREFIX}team/app")),
    ] {
        let body = refused(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(&[("env", "prod")])}),
            "InvalidParameterException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message, UNPARSEABLE_ARN_MESSAGE,
            "case {case:?}: expected the resourceArn clause, got {message:?}"
        );
    }

    // None of them tagged the repository that does exist.
    let actual = listed(&router, &format!("{ARN_PREFIX}team/app"), "after the refusals").await;
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after the refusals\": expected the created tag alone, got {actual:?}"
    );
}

#[tokio::test]
async fn a_well_formed_arn_naming_no_repository_of_this_registry_is_not_found() {
    let router = service();
    given_repository(&router, "team/app", &[], "setup").await;

    for (case, arn) in [
        (
            "a name that was never created",
            format!("{ARN_PREFIX}team/missing"),
        ),
        ("a prefix of a stored name", format!("{ARN_PREFIX}team")),
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
        let body = refused(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(&[("env", "prod")])}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert!(
            message.contains("does not exist in the registry"),
            "case {case:?}: expected the missing-repository refusal, got {message:?}"
        );
    }

    // The stored repository is tagged until it is deleted, and is not found after: the
    // tags of a deleted repository go with it ("when a resource is deleted, the tags
    // associated with that resource are also deleted"), so a repository created again
    // under the same name starts untagged.
    let arn = format!("{ARN_PREFIX}team/app");
    ok(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": tags(&[("env", "prod")])}),
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
        "TagResource",
        json!({"resourceArn": arn, "tags": tags(&[("env", "prod")])}),
        "RepositoryNotFoundException",
        "after the delete",
    )
    .await;
    given_repository(&router, "team/app", &[], "created again").await;
    let actual = listed(&router, &arn, "created again").await;
    assert!(
        actual.is_empty(),
        "case \"created again\": expected no tags to survive the delete, got {actual:?}"
    );
}

#[tokio::test]
async fn a_tag_that_carries_no_key_names_no_category() {
    let router = service();
    let arn = given_repository(&router, "keyless", &[("env", "prod")], "setup").await;

    // A tag is a key-value pair whose key "acts like a category" (`Tag$Key`), so a pair
    // sent without one is nothing this registry can store. `Tag$Key` is modelled
    // `@length(1..128)`, so the EMPTY key is refused by the framework before the handler
    // runs; the absent member reaches the handler.
    for (case, sent) in [
        ("a tag with no Key member", json!([{"Value": "orphan"}])),
        ("an empty Key", json!([{"Key": "", "Value": "orphan"}])),
    ] {
        let (status, body) = super::repository_image_lifecycle::call(
            &router,
            "TagResource",
            json!({"resourceArn": arn, "tags": sent}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a keyless tag to be refused, got {status} with {body}"
        );
    }

    let actual = listed(&router, &arn, "after the refusals").await;
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after the refusals\": expected the tag set to be untouched, got {actual:?}"
    );
}
