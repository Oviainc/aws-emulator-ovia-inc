// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) `WebACL` surface a client can still reach, end to end
//! over the wire: `GetChangeToken` and `GetWebACL`.
//!
//! AWS WAF Classic is past its end of support, so no call mints a web ACL any more:
//! `CreateWebACL` refuses every request (`web_acl_end_of_support.rs` pins that rule) and
//! the web ACLs a client reads are the ones that already existed. Those records are put
//! in the store with the `harness::store_web_acl` fixture — through the service's own
//! state types, so what the reader answers over the wire is a record indistinguishable
//! from one this service minted itself.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::harness::{expected_arn, store_web_acl};
use crate::wire::{
    call_in_region, change_token, error_message, expect_error, is_resource_id, ok_call, router,
    router_on, DEFAULT_TEST_REGION,
};

/// One row of the read-back table: a label, the `Name`, the `MetricName`, the
/// `DefaultAction.Type` of the stored record, and the tags it carries.
type StoredCase<'a> = (&'a str, &'a str, &'a str, &'a str, &'a [(&'a str, &'a str)]);

/// `GetWebACL` reports the stored record, field for field.
///
/// The table spans the actions a stored `DefaultAction` may carry, `Name` and
/// `MetricName` lengths and forms, records with and without tags, and two records
/// sharing one `Name` — `ResourceName` carries no uniqueness constraint, so those are
/// two distinct resources under two ids.
#[tokio::test]
async fn get_web_acl_reads_back_the_stored_web_acl() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    let longest_name = "n".repeat(128);
    let longest_metric_name = "M".repeat(128);
    // (label, Name, MetricName, DefaultAction.Type, Tags)
    let cases: &[StoredCase<'_>] = &[
        ("allow by default", "bootstrap-webacl", "bootstrapWebACL", "ALLOW", &[]),
        ("block by default", "edge-block", "edgeBlock", "BLOCK", &[]),
        ("one character of each (the minimum length)", "x", "m", "ALLOW", &[]),
        (
            "128 characters of each (the maximum length)",
            longest_name.as_str(),
            longest_metric_name.as_str(),
            "BLOCK",
            &[],
        ),
        (
            "a name with mixed case, digits and separators",
            "Prod_WebACL-2",
            "ProdWebACL2",
            "ALLOW",
            &[],
        ),
        ("an interior space in the name", "corp office acl", "corpOffice", "BLOCK", &[]),
        (
            "a tagged record",
            "tagged-acl",
            "taggedAcl",
            "ALLOW",
            &[("team", "edge"), ("cost", "")],
        ),
        ("a name shared with the next row", "shared-name", "sharedOne", "ALLOW", &[]),
        ("the same name again", "shared-name", "sharedTwo", "BLOCK", &[]),
    ];

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for (label, name, metric_name, default_action, tags) in cases {
        let web_acl_id = store_web_acl(
            &engine,
            DEFAULT_TEST_REGION,
            name,
            metric_name,
            default_action,
            tags,
        );
        assert!(
            is_resource_id(&web_acl_id),
            "case {label}: expected a minted WebACLId, got {web_acl_id:?}"
        );
        assert!(
            seen_ids.insert(web_acl_id.clone()),
            "case {label}: WebACLId {web_acl_id} was already handed out"
        );

        let web_acl = ok_call(&router, "GetWebACL", &json!({"WebACLId": &web_acl_id})).await
            ["WebACL"]
            .clone();
        assert_eq!(
            web_acl["WebACLId"].as_str(),
            Some(web_acl_id.as_str()),
            "case {label}: expected WebACLId {web_acl_id:?}, got {web_acl}"
        );
        assert_eq!(
            web_acl["Name"].as_str(),
            Some(*name),
            "case {label}: expected Name {name:?}, got {}",
            web_acl["Name"]
        );
        assert_eq!(
            web_acl["MetricName"].as_str(),
            Some(*metric_name),
            "case {label}: expected MetricName {metric_name:?}, got {}",
            web_acl["MetricName"]
        );
        assert_eq!(
            web_acl["DefaultAction"]["Type"].as_str(),
            Some(*default_action),
            "case {label}: expected DefaultAction.Type {default_action:?}, got {}",
            web_acl["DefaultAction"]
        );
        assert_eq!(
            web_acl["Rules"].as_array().map(Vec::len),
            Some(0),
            "case {label}: a web ACL activating no rule reports an empty Rules, got {}",
            web_acl["Rules"]
        );
        assert_eq!(
            web_acl["WebACLArn"].as_str(),
            Some(expected_arn(DEFAULT_TEST_REGION, &web_acl_id).as_str()),
            "case {label}: expected WebACLArn {:?}, got {}",
            expected_arn(DEFAULT_TEST_REGION, &web_acl_id),
            web_acl["WebACLArn"]
        );
        // The modelled `WebACL` shape carries no `Tags` member, so a tagged record
        // reports the same projection as an untagged one.
        assert!(
            web_acl.get("Tags").is_none(),
            "case {label}: the modelled WebACL carries no Tags member, got {web_acl}"
        );
    }
}

/// `WebACLArn` is a property of the RECORD: a reader reports the ARN the web ACL carries
/// — naming the region it was created in — rather than one rebuilt for the region the
/// read was signed for, and reading it twice reports the same thing, since a read writes
/// nothing.
#[tokio::test]
async fn a_read_reports_the_stored_arn_whatever_region_it_is_signed_for() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    // Regions differing in shape and length, each read back from a region that is not
    // the record's own.
    for (label, record_region, reader_region) in [
        ("the default region", "us-east-1", "us-west-2"),
        ("another commercial region", "eu-west-3", "us-east-1"),
        ("a longer region name", "ap-southeast-4", "eu-central-1"),
    ] {
        let web_acl_id = store_web_acl(
            &engine,
            record_region,
            &format!("regional-{record_region}"),
            "regionalAcl",
            "ALLOW",
            &[],
        );
        let expected = expected_arn(record_region, &web_acl_id);

        let (status, read) = call_in_region(
            &router,
            reader_region,
            "GetWebACL",
            &json!({"WebACLId": &web_acl_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: GetWebACL expected 200, got {status}: {read}"
        );
        assert_eq!(
            read["WebACL"]["WebACLArn"].as_str(),
            Some(expected.as_str()),
            "case {label}: a read from {reader_region} must report the stored ARN \
             {expected:?}, got {}",
            read["WebACL"]["WebACLArn"]
        );

        let (_, again) = call_in_region(
            &router,
            reader_region,
            "GetWebACL",
            &json!({"WebACLId": &web_acl_id}),
        )
        .await;
        assert_eq!(
            again, read,
            "case {label}: a read must not change what a later caller sees: {read} then \
             {again}"
        );
    }
}

/// `GetWebACL` refuses an id that names no stored WebACL — "the referenced object
/// doesn't exist".
#[tokio::test]
async fn get_web_acl_refuses_an_id_that_names_no_web_acl() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    store_web_acl(
        &engine,
        DEFAULT_TEST_REGION,
        "reader-probe",
        "readerProbe",
        "BLOCK",
        &[],
    );
    // An id minted for a DIFFERENT resource type is not a WebACLId either.
    let ip_set_token = change_token(&router).await;
    let ip_set = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "not-a-web-acl", "ChangeToken": ip_set_token}),
    )
    .await;
    let ip_set_id = ip_set["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {ip_set}"))
        .to_string();

    for (label, id) in [
        ("a UUID this service never minted", "11111111-2222-4333-8444-555555555555"),
        ("an id of another resource type", ip_set_id.as_str()),
        ("a value that is not an id at all", "no-such-web-acl"),
    ] {
        expect_error(
            &router,
            "GetWebACL",
            &json!({"WebACLId": id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
    }
}

/// The message of each refusal is ONE fixed sentence for that exception class: the same
/// string for every request in the class, carrying nothing of the request.
///
/// Source: AWS WAF Classic's recorded behaviour — the identical sentence for a
/// never-created id and for a deleted one, with no request-derived content.
#[tokio::test]
async fn each_refusal_class_answers_one_fixed_request_independent_sentence() {
    let router = router();

    // Two absent ids, differing in form and length: one sentence, naming neither.
    let absent = [
        "11111111-2222-4333-8444-555555555555",
        "an-id-that-was-never-minted",
    ];
    let mut not_found: Vec<String> = Vec::new();
    for id in absent {
        let body = expect_error(
            &router,
            "GetWebACL",
            &json!({"WebACLId": id}),
            "WAFNonexistentItemException",
            id,
        )
        .await;
        let text = error_message(&body);
        assert!(
            !text.contains(id),
            "case {id}: the not-found message must not echo the requested id, got {text:?}"
        );
        not_found.push(text);
    }
    assert_eq!(
        not_found[0], not_found[1],
        "every WAFNonexistentItemException must carry the same sentence, got {not_found:?}"
    );

    // Two stale tokens — one a mutating call has spent, one never minted: one sentence,
    // naming neither token.
    let consumed = change_token(&router).await;
    ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "fixed-message-probe", "ChangeToken": consumed}),
    )
    .await;
    let stale = [consumed.as_str(), "11111111-2222-4333-8444-555555555555"];
    let mut stale_messages: Vec<String> = Vec::new();
    for token in stale {
        let body = expect_error(
            &router,
            "CreateIPSet",
            &json!({"Name": "fixed-message-probe", "ChangeToken": token}),
            "WAFStaleDataException",
            token,
        )
        .await;
        let text = error_message(&body);
        assert!(
            !text.contains(token),
            "case {token}: the stale-token message must not echo the token, got {text:?}"
        );
        stale_messages.push(text);
    }
    assert_eq!(
        stale_messages[0], stale_messages[1],
        "every WAFStaleDataException must carry the same sentence, got {stale_messages:?}"
    );

    // Two disallowed metric names: one sentence, naming neither name.
    let mut disallowed: Vec<String> = Vec::new();
    for metric_name in ["All", "edge acl"] {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "CreateWebACL",
            &json!({
                "Name": "fixed-message-probe",
                "MetricName": metric_name,
                "DefaultAction": {"Type": "ALLOW"},
                "ChangeToken": token,
            }),
            "WAFDisallowedNameException",
            metric_name,
        )
        .await;
        let text = error_message(&body);
        assert!(
            !text.contains(metric_name),
            "case {metric_name}: the disallowed-name message must not echo the name, \
             got {text:?}"
        );
        disallowed.push(text);
    }
    assert_eq!(
        disallowed[0], disallowed[1],
        "every WAFDisallowedNameException must carry the same sentence, got {disallowed:?}"
    );
}

/// A `MetricName` the model's own `@length` bound refuses never reaches the handler: the
/// boundary answers it, so the refusal is the modelled constraint's, not this service's.
#[tokio::test]
async fn a_metric_name_past_the_modelled_length_is_refused_at_the_boundary() {
    let router = router();

    let token = change_token(&router).await;
    let (status, body) = call_in_region(
        &router,
        DEFAULT_TEST_REGION,
        "CreateWebACL",
        &json!({
            "Name": "metric-probe",
            "MetricName": "m".repeat(129),
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case 129 characters (one past the maximum length): expected 400, got {status}: {body}"
    );
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    assert!(
        code.contains("ValidationException") || code.contains("WAFDisallowedNameException"),
        "case 129 characters (one past the maximum length): expected the request to be \
         refused as invalid input, got {code}"
    );
}
