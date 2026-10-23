// SPDX-License-Identifier: CC0-1.0

//! `UpdateRepositoryCreationTemplate`, end to end over the wire.
//!
//! The rule under test: an update addresses an existing template by its namespace prefix
//! (the trailing slash ECR assumes is not part of that key) and replaces exactly the
//! members the request carries, leaving every omitted member as the store already held
//! it, moving only `updatedAt`; a prefix no template answers to is refused with
//! `TemplateNotFoundException`, and a refused update changes nothing. Every reporting
//! operation then answers from that one record.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::ComAmazonawsEcrAmazonec2containerregistryV20150921State;

use crate::repository_creation_templates::{timestamps, without_timestamps};
use crate::wire::{call, emulator, emulator_with_engine, ok, refused, REGISTRY_ID};

/// The repository creation scenarios the model enumerates, from
/// `smithy_query(query="shape", shape="RCTAppliedFor")`: the whole catalog, so a value an
/// update drops is visible.
const APPLIED_FOR: [&str; 3] = ["REPLICATION", "PULL_THROUGH_CACHE", "CREATE_ON_PUSH"];

/// The tag mutability settings the model enumerates, from
/// `smithy_query(query="shape", shape="ImageTagMutability")`.
const IMAGE_TAG_MUTABILITY: [&str; 4] = [
    "MUTABLE",
    "IMMUTABLE",
    "IMMUTABLE_WITH_EXCLUSION",
    "MUTABLE_WITH_EXCLUSION",
];

/// A KMS key ARN matching `KmsKeyForRepositoryCreationTemplate`'s pattern
/// (`^$|arn:aws[a-z0-9-]*:kms:[a-z0-9-]+:[0-9]{12}:key\/[a-z0-9-]+$`).
const KMS_KEY_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/abc-123";

/// The prefix the baseline template below is created under.
const BASELINE_PREFIX: &str = "team/app";

/// How far back a stored template's timestamps are moved before the update under test,
/// so `updatedAt` advancing is observable whatever the clock's resolution is.
const BACKDATE: Duration = Duration::from_secs(3600);

/// A template carrying a value for every member the update can change, so a member the
/// update omits is observably left alone rather than defaulted.
fn baseline_request() -> Value {
    json!({
        "prefix": BASELINE_PREFIX,
        "appliedFor": ["CREATE_ON_PUSH", "REPLICATION"],
        "description": "the baseline template",
        "customRoleArn": "arn:aws:iam::000000000000:role/ecr-template",
        "imageTagMutability": "IMMUTABLE_WITH_EXCLUSION",
        "imageTagMutabilityExclusionFilters": [
            {"filterType": "WILDCARD", "filter": "dev-*"},
            {"filterType": "WILDCARD", "filter": "nightly"},
        ],
        "repositoryPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
        "lifecyclePolicy": "{\"rules\":[]}",
        "resourceTags": [
            {"Key": "env", "Value": "prod"},
            {"Key": "team", "Value": "platform"},
        ],
        "encryptionConfiguration": {"encryptionType": "AES256"},
    })
}

/// Move the stored template's two timestamps back by [`BACKDATE`]. The store is reached
/// directly because no number of wire calls can age a record.
fn backdate(engine: &Arc<engine::InMemoryEngine>, prefix: &str) {
    let state = engine.state::<ComAmazonawsEcrAmazonec2containerregistryV20150921State>();
    let mut store = state.lock();
    let template = store
        .creation_templates
        .get_mut(prefix)
        .unwrap_or_else(|| panic!("the baseline template {prefix:?} must be stored"));
    template.created_at -= BACKDATE;
    template.updated_at -= BACKDATE;
}

/// The one template the registry holds for `prefix`, as `DescribeRepositoryCreationTemplates`
/// reports it.
async fn described(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    prefix: &str,
) -> Value {
    let body = ok(
        router,
        case,
        "DescribeRepositoryCreationTemplates",
        &json!({"prefixes": [prefix]}),
    )
    .await;
    let listed = body["repositoryCreationTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body:?}"))
        .clone();
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: exactly one template must answer to {prefix:?}, got {listed:?}"
    );
    listed[0].clone()
}

/// The record the rule says an update leaves behind: the stored template with exactly the
/// request's members replaced, timestamps aside.
fn expected_after(case: &str, stored: &Value, request: &Value) -> Value {
    let mut expected = without_timestamps(stored);
    let object = expected
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {case:?}: a template must be an object: {stored:?}"));
    let members = request
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: a request must be an object: {request:?}"));
    for (member, value) in members {
        // `prefix` is the key the update addresses, not a member it changes.
        if member == "prefix" {
            continue;
        }
        // An empty `resourceTags` list is reported as an absent member: nothing is
        // applied to a repository created from the template.
        if member == "resourceTags" && value.as_array().is_some_and(|list| list.is_empty()) {
            object.remove(member);
        } else {
            object.insert(member.clone(), value.clone());
        }
    }
    expected
}

/// The rows of the update table: they differ along what the rule generalises over — which
/// members are present, how the addressed prefix is spelled, how many scenarios and
/// filters and tags a list member carries, and which enumerated value a scalar takes.
fn update_cases() -> Vec<(&'static str, Value)> {
    let longest_description = "d".repeat(256);
    let mut cases: Vec<(&'static str, Value)> = vec![
        (
            "no member but the prefix: only updatedAt moves",
            json!({"prefix": BASELINE_PREFIX}),
        ),
        (
            "the description alone",
            json!({"prefix": BASELINE_PREFIX, "description": "updated"}),
        ),
        (
            "a description at the member's maximum length",
            json!({"prefix": BASELINE_PREFIX, "description": longest_description}),
        ),
        (
            "an empty description, which the member's minimum length admits",
            json!({"prefix": BASELINE_PREFIX, "description": ""}),
        ),
        (
            "the assumed trailing slash addresses the same template",
            json!({"prefix": format!("{BASELINE_PREFIX}/"), "description": "by slash"}),
        ),
        (
            "appliedFor narrowed to one scenario",
            json!({"prefix": BASELINE_PREFIX, "appliedFor": ["PULL_THROUGH_CACHE"]}),
        ),
        (
            "appliedFor widened to every scenario the model enumerates",
            json!({"prefix": BASELINE_PREFIX, "appliedFor": APPLIED_FOR}),
        ),
        (
            "appliedFor reordered, which the list preserves",
            json!({"prefix": BASELINE_PREFIX, "appliedFor": ["REPLICATION", "CREATE_ON_PUSH"]}),
        ),
        (
            "resourceTags replaced wholesale, not merged",
            json!({"prefix": BASELINE_PREFIX, "resourceTags": [{"Key": "owner", "Value": "sre"}]}),
        ),
        (
            "resourceTags cleared",
            json!({"prefix": BASELINE_PREFIX, "resourceTags": []}),
        ),
        (
            "one exclusion filter, the list's minimum",
            json!({
                "prefix": BASELINE_PREFIX,
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "release-*"},
                ],
            }),
        ),
        (
            "the five exclusion filters the list's maximum admits",
            json!({
                "prefix": BASELINE_PREFIX,
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "dev-*"},
                    {"filterType": "WILDCARD", "filter": "test-*"},
                    {"filterType": "WILDCARD", "filter": "tmp"},
                    {"filterType": "WILDCARD", "filter": "a.b_c-*"},
                    {"filterType": "WILDCARD", "filter": "weekly"},
                ],
            }),
        ),
        (
            "encryption moved to KMS with the caller's key",
            json!({
                "prefix": BASELINE_PREFIX,
                "encryptionConfiguration": {"encryptionType": "KMS", "kmsKey": KMS_KEY_ARN},
            }),
        ),
        (
            "encryption moved to KMS with the account's default key",
            json!({
                "prefix": BASELINE_PREFIX,
                "encryptionConfiguration": {"encryptionType": "KMS"},
            }),
        ),
        (
            "both policies",
            json!({
                "prefix": BASELINE_PREFIX,
                "repositoryPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"a\"}]}",
                "lifecyclePolicy": "{\"rules\":[{\"rulePriority\":1}]}",
            }),
        ),
        (
            "the custom role ECR assumes",
            json!({
                "prefix": BASELINE_PREFIX,
                "customRoleArn": "arn:aws:iam::000000000000:role/ecr-other",
            }),
        ),
        (
            "every member at once",
            json!({
                "prefix": BASELINE_PREFIX,
                "appliedFor": ["PULL_THROUGH_CACHE"],
                "description": "everything",
                "customRoleArn": "arn:aws:iam::000000000000:role/ecr-everything",
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "keep"},
                ],
                "repositoryPolicy": "{\"Version\":\"2012-10-17\"}",
                "lifecyclePolicy": "{\"rules\":[]}",
                "resourceTags": [{"Key": "env", "Value": "stage"}],
                "encryptionConfiguration": {"encryptionType": "KMS_DSSE"},
            }),
        ),
    ];
    // One row per tag mutability setting the model enumerates, including the baseline's
    // own — an update to the value already stored is still an update.
    for setting in IMAGE_TAG_MUTABILITY {
        cases.push((
            setting,
            json!({"prefix": BASELINE_PREFIX, "imageTagMutability": setting}),
        ));
    }
    cases
}

/// An update replaces the members it carries and leaves every other one as stored; the
/// answer is the stored record, and a later describe reports exactly the same thing.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries() {
    for (case, request) in update_cases() {
        let (router, engine) = emulator_with_engine();
        ok(
            &router,
            case,
            "CreateRepositoryCreationTemplate",
            &baseline_request(),
        )
        .await;
        backdate(&engine, BASELINE_PREFIX);
        let stored = described(&router, case, BASELINE_PREFIX).await;
        let (created_at, baseline_updated_at) = timestamps(case, &stored);

        let answer = ok(
            &router,
            case,
            "UpdateRepositoryCreationTemplate",
            &request,
        )
        .await;
        assert_eq!(
            answer["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the update must report this registry, got {:?}",
            answer["registryId"]
        );
        let updated = answer["repositoryCreationTemplate"].clone();
        let expected = expected_after(case, &stored, &request);
        assert_eq!(
            without_timestamps(&updated),
            expected,
            "case {case:?}: expected the update to report {expected:?}, got {:?}",
            without_timestamps(&updated)
        );

        let (updated_created_at, updated_at) = timestamps(case, &updated);
        assert_eq!(
            updated_created_at, created_at,
            "case {case:?}: an update must leave createdAt at the instant the template \
             was created ({created_at}), got {updated_created_at}"
        );
        assert!(
            updated_at > baseline_updated_at,
            "case {case:?}: an update must move updatedAt past {baseline_updated_at}, \
             got {updated_at}"
        );

        // The answer is the record the registry now holds, read back through the
        // describe — not something the update synthesised for its caller.
        let read_back = described(&router, case, BASELINE_PREFIX).await;
        assert_eq!(
            read_back, updated,
            "case {case:?}: the describe must report the updated record unchanged, got \
             {read_back:?}"
        );

        // Reading reports; it does not change what a later caller sees.
        let read_again = described(&router, case, BASELINE_PREFIX).await;
        assert_eq!(
            read_again, read_back,
            "case {case:?}: a second read must answer exactly what the first did, got \
             {read_again:?}"
        );
    }
}

/// Successive updates compose: each one replaces what it names on the record the previous
/// one left, so the registry ends up holding one template carrying the last value given
/// for every member.
#[tokio::test]
async fn successive_updates_compose_on_one_record() {
    let (router, engine) = emulator_with_engine();
    ok(
        &router,
        "composed",
        "CreateRepositoryCreationTemplate",
        &baseline_request(),
    )
    .await;
    backdate(&engine, BASELINE_PREFIX);
    let mut stored = described(&router, "composed", BASELINE_PREFIX).await;

    for (case, request) in update_cases() {
        let answer = ok(
            &router,
            case,
            "UpdateRepositoryCreationTemplate",
            &request,
        )
        .await;
        let expected = expected_after(case, &stored, &request);
        let reported = answer["repositoryCreationTemplate"].clone();
        assert_eq!(
            without_timestamps(&reported),
            expected,
            "case {case:?}: an update must build on the record the previous one left, \
             expected {expected:?}, got {:?}",
            without_timestamps(&reported)
        );
        stored = reported;
    }

    // One template per prefix throughout: the updates changed a record, never added one.
    let all = ok(
        &router,
        "composed",
        "DescribeRepositoryCreationTemplates",
        &json!({}),
    )
    .await;
    assert_eq!(
        all["repositoryCreationTemplates"],
        json!([stored]),
        "the registry must hold exactly the one updated template, got {:?}",
        all["repositoryCreationTemplates"]
    );
}

/// An update addresses a template that must already exist: a prefix no template answers
/// to is refused with `TemplateNotFoundException`, and nothing is created for it. The rows
/// differ in why the prefix names nothing and in how it is spelled.
#[tokio::test]
async fn an_update_to_an_unknown_prefix_is_refused() {
    let cases: Vec<(&str, &str, bool)> = vec![
        ("a prefix never created", "nothing-here", false),
        ("the registry-wide prefix, uncreated", "ROOT", false),
        (
            "a namespaced prefix never created",
            "other/namespace",
            false,
        ),
        (
            "an uncreated prefix with the assumed trailing slash",
            "nothing-here/",
            false,
        ),
        ("a prefix whose template was deleted", BASELINE_PREFIX, true),
    ];

    for (case, prefix, create_then_delete) in cases {
        let router = emulator();
        if create_then_delete {
            ok(
                &router,
                case,
                "CreateRepositoryCreationTemplate",
                &baseline_request(),
            )
            .await;
            ok(
                &router,
                case,
                "DeleteRepositoryCreationTemplate",
                &json!({"prefix": prefix}),
            )
            .await;
        }

        refused(
            &router,
            case,
            "UpdateRepositoryCreationTemplate",
            &json!({"prefix": prefix, "description": "updated"}),
            "TemplateNotFoundException",
        )
        .await;

        let listing = ok(
            &router,
            case,
            "DescribeRepositoryCreationTemplates",
            &json!({}),
        )
        .await;
        assert_eq!(
            listing["repositoryCreationTemplates"],
            json!([]),
            "case {case:?}: a refused update must create nothing, got {:?}",
            listing["repositoryCreationTemplates"]
        );
    }
}

/// A refused update leaves the stored template exactly as it was — every member and both
/// timestamps. The rows cover the two rules this operation checks itself: a `kmsKey`
/// belongs to the KMS encryption types, not to the S3-managed `AES256`; and `resourceTags`
/// are repository tags, so they obey the shared tag rules (no reserved key, no over-long
/// key or value, no more tags than a repository may hold).
#[tokio::test]
async fn a_refused_update_changes_nothing() {
    let many: Vec<Value> = (0..51)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let cases: Vec<(&str, Value)> = vec![
        (
            "a kmsKey beside the AES256 encryption type",
            json!({
                "prefix": BASELINE_PREFIX,
                "encryptionConfiguration": {"encryptionType": "AES256", "kmsKey": KMS_KEY_ARN},
            }),
        ),
        (
            "a tag key in the reserved aws: namespace",
            json!({
                "prefix": BASELINE_PREFIX,
                "resourceTags": [{"Key": "aws:created-by", "Value": "me"}],
            }),
        ),
        (
            "a tag key past its maximum character length",
            json!({
                "prefix": BASELINE_PREFIX,
                "resourceTags": [{"Key": "k".repeat(129), "Value": "v"}],
            }),
        ),
        (
            "a tag value past its maximum length",
            json!({
                "prefix": BASELINE_PREFIX,
                "resourceTags": [{"Key": "k", "Value": "v".repeat(257)}],
            }),
        ),
        (
            "more tags than a repository may hold",
            json!({"prefix": BASELINE_PREFIX, "resourceTags": many}),
        ),
    ];

    for (case, request) in cases {
        let (router, engine) = emulator_with_engine();
        ok(
            &router,
            case,
            "CreateRepositoryCreationTemplate",
            &baseline_request(),
        )
        .await;
        backdate(&engine, BASELINE_PREFIX);
        let stored = described(&router, case, BASELINE_PREFIX).await;

        let answer = call(&router, "UpdateRepositoryCreationTemplate", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );

        let read_back = described(&router, case, BASELINE_PREFIX).await;
        assert_eq!(
            read_back, stored,
            "case {case:?}: a refused update must leave the stored template untouched, \
             expected {stored:?}, got {read_back:?}"
        );
    }
}
