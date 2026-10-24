// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Firewall Manager's resource tagging (`TagResource`,
//! `ListTagsForResource`) and for `ListAdminsManagingAccount`.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router — reusing the request harness in
//! [`crate::policy_lifecycle`], so the assertions are about the WIRE response.
//!
//! The rules asserted here, each over a TABLE rather than one recorded request:
//!
//! * a `ResourceArn` that is not an ARN at all is refused with 400
//!   `ValidationException` before any state is read or written;
//! * a well-formed ARN naming no stored Firewall Manager resource is the
//!   modelled 400 `ResourceNotFoundException`, and a refused tag write leaves no
//!   tag state behind;
//! * a tag write merges into the resource's persisted tag set by key, and
//!   `ListTagsForResource` reports exactly that set;
//! * `ListAdminsManagingAccount` enumerates the administrator accounts managing
//!   the caller, and is `ResourceNotFoundException` — not an empty page — when
//!   none does.

use serde_json::{json, Value};

use crate::policy_lifecycle::{call, emulator, error_code, ACCOUNT, REGION};

/// A minimal policy document: every required `Policy` member and nothing else.
/// What is varied in these tests is the TAGS and the ARN, not the policy.
///
/// This suite owns the tagging harness for this test target (`minimal_policy`,
/// [`tag_list`], [`owned`], [`tag_pairs`] and [`list_tags`]), and they are
/// `pub(crate)` so the sibling `tagging_across_resource_kinds` suite builds and
/// reads a `TagList` through the SAME helpers rather than a second copy of them.
pub(crate) fn minimal_policy(name: &str) -> Value {
    json!({
        "PolicyName": name,
        "SecurityServicePolicyData": {
            "Type": "WAFV2",
            "ManagedServiceData": "{\"type\":\"WAFV2\"}"
        },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    })
}

/// Create a policy carrying `tags` and return its ARN.
async fn put_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    tags: &Value,
) -> String {
    let (status, body) = call(
        router,
        "PutPolicy",
        &json!({ "Policy": minimal_policy(name), "TagList": tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected PutPolicy to answer 200, got {status} with {body:?}"
    );
    body["PolicyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: PutPolicy reported no PolicyArn: {body:?}"))
        .to_string()
}

/// A `TagList` wire value built from key/value pairs.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// `pairs` as the owned, sorted set the assertions compare against.
pub(crate) fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// A wire `TagList` as the same sorted set: tags are a key:value MAP, so the
/// comparison is order-insensitive.
pub(crate) fn tag_pairs(value: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = match value.as_array() {
        Some(items) => items
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        None => Vec::new(),
    };
    pairs.sort();
    pairs
}

/// `ListTagsForResource` for `arn`, as `(status, error code or "", tag set)`.
pub(crate) async fn list_tags(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> (u16, String, Vec<(String, String)>) {
    let (status, body) = call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    (status, code, tag_pairs(&body["TagList"]))
}

/// One row of the tag-write table: what the resource already holds, what the
/// write adds, and the set it must hold afterwards.
struct TagCase {
    name: String,
    initial: Value,
    added: Value,
    expected: Vec<(String, String)>,
}

/// The tag-write class the merge rule is asserted over. The rows differ in tag
/// COUNT (none, one, several), in whether a written key is already held, in key
/// and value FORM across the modelled `TagKey` / `TagValue` pattern
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from `smithy_query(shape="Tag")`) and at
/// those members' published length bounds (`TagKey` 1..128, `TagValue` 0..256).
fn tag_cases() -> Vec<TagCase> {
    // The published maxima of TagKey and TagValue, exactly.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        TagCase {
            name: "untagged resource, one tag added".to_string(),
            initial: tag_list(&[]),
            added: tag_list(&[("k", "v")]),
            expected: owned(&[("k", "v")]),
        },
        TagCase {
            name: "nothing added keeps what is held".to_string(),
            initial: tag_list(&[("stage", "test")]),
            added: tag_list(&[]),
            expected: owned(&[("stage", "test")]),
        },
        TagCase {
            name: "a new key is added beside the held ones".to_string(),
            initial: tag_list(&[("stage", "test"), ("team", "net")]),
            added: tag_list(&[("owner", "ops")]),
            expected: owned(&[("stage", "test"), ("team", "net"), ("owner", "ops")]),
        },
        TagCase {
            name: "a held key takes the new value".to_string(),
            initial: tag_list(&[("stage", "test"), ("team", "net")]),
            added: tag_list(&[("stage", "prod")]),
            expected: owned(&[("stage", "prod"), ("team", "net")]),
        },
        TagCase {
            name: "keys spanning the modelled pattern".to_string(),
            initial: tag_list(&[("dot.key", "1")]),
            added: tag_list(&[
                ("colon:key", "2"),
                ("slash/key", "3"),
                ("plus+key", "4"),
                ("equals=key", "5"),
                ("at@key", "6"),
                ("dash-key", "7"),
                ("under_key", "8"),
                ("Ünïcødé", "9"),
                ("digits123", "10"),
            ]),
            expected: owned(&[
                ("dot.key", "1"),
                ("colon:key", "2"),
                ("slash/key", "3"),
                ("plus+key", "4"),
                ("equals=key", "5"),
                ("at@key", "6"),
                ("dash-key", "7"),
                ("under_key", "8"),
                ("Ünïcødé", "9"),
                ("digits123", "10"),
            ]),
        },
        TagCase {
            // `TagValue`'s minimum length is 0: an empty value is a value.
            name: "an empty tag value is stored as sent".to_string(),
            initial: tag_list(&[("held", "x")]),
            added: tag_list(&[("empty", "")]),
            expected: owned(&[("held", "x"), ("empty", "")]),
        },
        TagCase {
            name: "the longest modelled key and value".to_string(),
            initial: tag_list(&[]),
            added: tag_list(&[(longest_key.as_str(), longest_value.as_str())]),
            expected: owned(&[(longest_key.as_str(), longest_value.as_str())]),
        },
        TagCase {
            // "Tag keys are case-sensitive" (`com.amazonaws.fms#Tag$Key`), so
            // these are two keys, not one overwritten.
            name: "keys differing only in case are distinct".to_string(),
            initial: tag_list(&[("Env", "a")]),
            added: tag_list(&[("env", "b")]),
            expected: owned(&[("Env", "a"), ("env", "b")]),
        },
    ]
}

/// `TagResource` merges the written tags into the resource's persisted set by
/// key, and `ListTagsForResource` reports exactly that set — for every row of
/// the tag table, and identically on a repeated read.
#[tokio::test]
async fn tags_written_by_tag_resource_are_read_back_by_list_tags_for_resource() {
    for case in tag_cases() {
        let router = emulator();
        let name = &case.name;
        let arn = put_policy(&router, "tagged.policy", &case.initial).await;

        // The create's own TagList is what the resource starts from.
        let (status, code, before) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {name:?}: expected ListTagsForResource to answer 200, got {status} / {code:?}"
        );
        assert_eq!(
            before,
            tag_pairs(&case.initial),
            "case {name:?}: expected the created policy to hold the TagList it was created \
             with, got {before:?}"
        );

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceArn": arn, "TagList": case.added }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected TagResource to answer 200, got {status} with {body:?}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {name:?}: TagResource models no output members, so the body must be empty, \
             got {body:?}"
        );

        let (status, code, after) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {name:?}: expected ListTagsForResource to answer 200 after the write, got \
             {status} / {code:?}"
        );
        assert_eq!(
            after, case.expected,
            "case {name:?}: expected the merged tag set {:?}, got {after:?}",
            case.expected
        );

        // A read changes nothing a later caller sees.
        let (_, _, again) = list_tags(&router, &arn).await;
        assert_eq!(
            again, after,
            "case {name:?}: a repeated ListTagsForResource must answer identically, got \
             {again:?} after {after:?}"
        );

        // The tags travel with the resource: `GetPolicy` still reads back the
        // same policy, so tagging did not disturb it.
        let policy_id = arn
            .rsplit('/')
            .next()
            .unwrap_or_else(|| panic!("case {name:?}: the policy ARN has no resource id: {arn:?}"));
        let (status, _) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected GetPolicy to still answer 200 after tagging, got {status}"
        );
    }
}

/// Strings that are NOT Amazon Resource Names. Each one satisfies the modelled
/// `ResourceArn` pattern, so it reaches the handler and the service-level
/// ARN-format check is what refuses it.
fn malformed_arns() -> Vec<(&'static str, String)> {
    vec![
        ("no arn prefix at all", "this-is-not-an-arn-at-all".to_string()),
        ("the bare word arn", "arn".to_string()),
        ("prefix only", "arn:".to_string()),
        ("three fields", "arn:aws:fms".to_string()),
        (
            "five fields",
            format!("arn:aws:fms:{REGION}:{ACCOUNT}"),
        ),
        (
            "leading colon instead of the arn prefix",
            format!(":aws:fms:{REGION}:{ACCOUNT}:policy/abc"),
        ),
        (
            "empty partition",
            format!("arn::fms:{REGION}:{ACCOUNT}:policy/abc"),
        ),
        (
            "empty service",
            format!("arn:aws::{REGION}:{ACCOUNT}:policy/abc"),
        ),
        (
            "empty resource",
            format!("arn:aws:fms:{REGION}:{ACCOUNT}:"),
        ),
    ]
}

/// Well-formed ARNs that name no stored Firewall Manager resource.
fn unknown_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "an fms policy ARN naming no policy",
            format!("arn:aws:fms:{REGION}:{ACCOUNT}:policy/0f13c2a4-1111-4a26-9d0e-6b4f8d2a7c31"),
        ),
        (
            "an fms applications-list ARN",
            format!("arn:aws:fms:{REGION}:{ACCOUNT}:applications-list/abc"),
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket".to_string(),
        ),
        (
            "another partition",
            format!("arn:aws-cn:fms:cn-north-1:{ACCOUNT}:policy/abc"),
        ),
        (
            "another account",
            format!("arn:aws:fms:{REGION}:999988887777:policy/abc"),
        ),
    ]
}

/// A `ResourceArn` that is not an ARN at all is refused with 400
/// `ValidationException` — on the read and on the write alike — and the write
/// leaves the tags of the resources that DO exist untouched.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_refused_as_a_validation_error() {
    let router = emulator();
    let held = tag_list(&[("stage", "test")]);
    let arn = put_policy(&router, "untouched.policy", &held).await;

    for (case, malformed) in malformed_arns() {
        let (status, body) = call(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceArn": malformed }),
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected ListTagsForResource({malformed:?}) to answer 400 \
             ValidationException, got {status} / {code:?} with {body:?}"
        );

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceArn": malformed, "TagList": tag_list(&[("k", "v")]) }),
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected TagResource({malformed:?}) to answer 400 \
             ValidationException, got {status} / {code:?} with {body:?}"
        );

        let (_, _, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            tags,
            tag_pairs(&held),
            "case {case:?}: a refused TagResource must leave no tag state behind, but the \
             stored policy now holds {tags:?}"
        );
    }
}

/// A well-formed ARN naming nothing is the modelled `ResourceNotFoundException`
/// on both operations, and a tag write against it persists nothing — so the
/// read after it is still not-found.
#[tokio::test]
async fn a_well_formed_arn_naming_no_resource_is_not_found_and_stores_nothing() {
    // Asserted both on a fresh server and beside a stored policy, so "not
    // found" cannot be an artefact of an empty store.
    for with_a_stored_policy in [false, true] {
        let router = emulator();
        if with_a_stored_policy {
            put_policy(&router, "stored.policy", &tag_list(&[("stage", "test")])).await;
        }

        for (case, unknown) in unknown_arns() {
            let (status, code, _) = list_tags(&router, &unknown).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} (stored policy: {with_a_stored_policy}): expected \
                 ListTagsForResource({unknown:?}) to answer 400 ResourceNotFoundException, got \
                 {status} / {code:?}"
            );

            let (status, body) = call(
                &router,
                "TagResource",
                &json!({ "ResourceArn": unknown, "TagList": tag_list(&[("k", "v")]) }),
            )
            .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} (stored policy: {with_a_stored_policy}): expected \
                 TagResource({unknown:?}) to answer 400 ResourceNotFoundException, got {status} \
                 / {code:?} with {body:?}"
            );

            // Nothing was created for the ARN by the refused write.
            let (status, code, _) = list_tags(&router, &unknown).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} (stored policy: {with_a_stored_policy}): a refused TagResource \
                 must create no resource, but ListTagsForResource({unknown:?}) now answers \
                 {status} / {code:?}"
            );
        }
    }
}

/// A `TagList` that names one key twice has no single outcome, so it is refused
/// with the modelled `InvalidInputException` and nothing is written.
#[tokio::test]
async fn a_tag_list_repeating_a_key_is_refused() {
    let rows: [(&str, Value); 3] = [
        ("the same key twice", tag_list(&[("k", "1"), ("k", "2")])),
        (
            "a repeat among other keys",
            tag_list(&[("a", "1"), ("b", "2"), ("a", "3")]),
        ),
        (
            "the same key and value twice",
            tag_list(&[("same", "v"), ("same", "v")]),
        ),
    ];

    for (case, duplicate) in rows {
        let router = emulator();
        let held = tag_list(&[("stage", "test")]);
        let arn = put_policy(&router, "duplicate.policy", &held).await;

        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceArn": arn, "TagList": duplicate }),
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case:?}: expected 400 InvalidInputException, got {status} / {code:?} with \
             {body:?}"
        );

        let (_, _, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            tags,
            tag_pairs(&held),
            "case {case:?}: a refused TagResource must write nothing, but the policy now holds \
             {tags:?}"
        );
    }
}

/// The tag set a resource can hold is bounded by the modelled `TagList` length
/// (0..200): the bound itself is accepted, and a write that would cross it is
/// refused with a 400 client error rather than stored.
#[tokio::test]
async fn the_tag_set_is_bounded_by_the_modelled_tag_list_length() {
    // `com.amazonaws.fms#TagList` is len 0..200.
    const MAX_TAGS: usize = 200;

    let router = emulator();
    let arn = put_policy(&router, "bounded.policy", &tag_list(&[])).await;

    let keys: Vec<String> = (0..MAX_TAGS).map(|index| format!("key{index}")).collect();
    let at_bound: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "ResourceArn": arn, "TagList": tag_list(&at_bound) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case at-bound: expected a {MAX_TAGS}-tag write to answer 200, got {status} with {body:?}"
    );
    let (_, _, tags) = list_tags(&router, &arn).await;
    assert_eq!(
        tags.len(),
        MAX_TAGS,
        "case at-bound: expected the resource to hold {MAX_TAGS} tags, got {}",
        tags.len()
    );

    // One key past the bound, both ways round: added to a full resource, and
    // sent as one over-long list.
    let one_more = tag_list(&[("key-past-the-bound", "v")]);
    let mut over_bound: Vec<(&str, &str)> = at_bound.clone();
    over_bound.push(("key-past-the-bound", "v"));
    let rows: [(&str, Value); 2] = [
        ("one tag past a full resource", one_more),
        ("a list one longer than the bound", tag_list(&over_bound)),
    ];
    for (case, over) in rows {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceArn": arn, "TagList": over }),
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            status, 400,
            "case {case:?}: expected a write past the {MAX_TAGS}-tag bound to be refused with \
             400, got {status} / {code:?} with {body:?}"
        );
        assert!(
            code == "LimitExceededException" || code == "ValidationException",
            "case {case:?}: expected the refusal to carry a modelled client error code, got \
             {code:?} with {body:?}"
        );
        let (_, _, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            tags.len(),
            MAX_TAGS,
            "case {case:?}: a refused write must not grow the tag set past {MAX_TAGS}, got {}",
            tags.len()
        );
    }
}

/// `ListAdminsManagingAccount` is `ResourceNotFoundException` while no
/// administrator manages the caller, and reports the associated administrator
/// once there is one — on every page size, and identically on a repeated read.
#[tokio::test]
async fn list_admins_managing_account_reports_the_administrator_or_is_not_found() {
    // Account ids differing in length and digits: the model constrains
    // `AWSAccountId` only to `^[0-9]+$`, length 1..1024.
    let accounts = ["123456789012", "000000000000", "7"];

    for account in accounts {
        let router = emulator();

        let (status, body) = call(&router, "ListAdminsManagingAccount", &json!({})).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {account:?}: with no administrator associated, expected 400 \
             ResourceNotFoundException rather than an empty page, got {status} / {code:?} with \
             {body:?}"
        );

        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": account }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account:?}: expected AssociateAdminAccount to answer 200, got {status} with \
             {body:?}"
        );

        // No `MaxResults`, the smallest and the largest the model allows
        // (`PaginationMaxResults` range 1..100) — and one past each bound, which
        // the boundary validator refuses.
        for max_results in [None, Some(1), Some(50), Some(100)] {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["MaxResults"] = json!(max);
            }
            let (status, body) = call(&router, "ListAdminsManagingAccount", &request).await;
            assert_eq!(
                status, 200,
                "case {account:?} / MaxResults {max_results:?}: expected 200, got {status} with \
                 {body:?}"
            );
            let admins: Vec<String> = body["AdminAccounts"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                admins,
                vec![account.to_string()],
                "case {account:?} / MaxResults {max_results:?}: expected the associated \
                 administrator to be the one managing the caller, got {admins:?}"
            );
            assert!(
                body.get("NextToken").is_none_or(Value::is_null),
                "case {account:?} / MaxResults {max_results:?}: the last page must carry no \
                 NextToken, got {:?}",
                body.get("NextToken")
            );
        }

        for past_bound in [0, 101] {
            let (status, body) = call(
                &router,
                "ListAdminsManagingAccount",
                &json!({ "MaxResults": past_bound }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {account:?} / MaxResults {past_bound}: a MaxResults outside the modelled \
                 1..100 range must be refused, got {status} with {body:?}"
            );
        }

        // A token this service minted for this page class resumes the
        // enumeration; anything else names no boundary and is the modelled
        // `InvalidInputException`.
        let own_token = hex::encode(account);
        let (status, body) = call(
            &router,
            "ListAdminsManagingAccount",
            &json!({ "NextToken": own_token }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account:?}: a token naming this account's own page boundary must resume the \
             enumeration, got {status} with {body:?}"
        );
        assert_eq!(
            body["AdminAccounts"],
            json!([account]),
            "case {account:?}: expected the resumed page to start at the boundary account, got \
             {:?}",
            body["AdminAccounts"]
        );

        let foreign = [
            ("not hex at all", "zzzz".to_string()),
            ("hex of a non-account string", hex::encode("abc")),
            (
                "a token minted for a policy page",
                hex::encode("0f13c2a4-1111-4a26-9d0e-6b4f8d2a7c31"),
            ),
        ];
        for (case, token) in foreign {
            let (status, body) = call(
                &router,
                "ListAdminsManagingAccount",
                &json!({ "NextToken": token }),
            )
            .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidInputException"),
                "case {account:?} / {case:?}: expected 400 InvalidInputException for a token \
                 this operation did not mint, got {status} / {code:?} with {body:?}"
            );
        }

        // A read changes nothing: the association is still reported, and
        // `GetAdminAccount` agrees with it.
        let (status, body) = call(&router, "GetAdminAccount", &json!({})).await;
        assert_eq!(
            (status, body["AdminAccount"].as_str().unwrap_or_default()),
            (200, account),
            "case {account:?}: GetAdminAccount must report the same administrator, got {status} \
             with {body:?}"
        );
    }
}
