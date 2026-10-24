// SPDX-License-Identifier: CC0-1.0

//! Repository creation templates, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes. The
//! rule under test: a repository creation template is a record of the settings the
//! create was given, keyed by its namespace prefix (a trailing slash is assumed, so it
//! is not part of that key), and every reporting operation computes its answer from that
//! record — one template per prefix, a second create for the same prefix refused.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::{
    default_template_encryption, ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    CreationTemplate, MAX_REPOSITORY_CREATION_TEMPLATES,
};

use crate::wire::{call, emulator, emulator_with_engine, ok, REGISTRY_ID};

/// The repository creation scenarios the model enumerates, from
/// `smithy_query(query="shape", shape="RCTAppliedFor")`: the whole catalog, so a value
/// the implementation drops is visible.
const APPLIED_FOR: [&str; 3] = ["REPLICATION", "PULL_THROUGH_CACHE", "CREATE_ON_PUSH"];

/// A KMS key ARN matching `KmsKeyForRepositoryCreationTemplate`'s pattern
/// (`^$|arn:aws[a-z0-9-]*:kms:[a-z0-9-]+:[0-9]{12}:key\/[a-z0-9-]+$`).
const KMS_KEY_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/abc-123";

/// One row of the create table: the case name, the `CreateRepositoryCreationTemplate`
/// request, and the template members the registry must report for it (every member bar
/// the two timestamps, which are compared separately).
type TemplateCase = (&'static str, Value, Value);

/// The template as a client compares it: every member bar the two timestamps, which are
/// per-call instants and are asserted on separately.
pub fn without_timestamps(template: &Value) -> Value {
    let mut stripped = template.clone();
    let object = stripped
        .as_object_mut()
        .unwrap_or_else(|| panic!("a repositoryCreationTemplate must be an object: {template:?}"));
    object.remove("createdAt");
    object.remove("updatedAt");
    stripped
}

/// The `createdAt`/`updatedAt` pair, each as the epoch seconds the wire carries.
pub fn timestamps(case: &str, template: &Value) -> (f64, f64) {
    let read = |member: &str| {
        template[member].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {member} must be a timestamp, got {:?}", template[member])
        })
    };
    (read("createdAt"), read("updatedAt"))
}

/// The prefixes `DescribeRepositoryCreationTemplates` reported, in the order it listed
/// them.
fn listed_prefixes(body: &Value) -> Vec<String> {
    body["repositoryCreationTemplates"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("DescribeRepositoryCreationTemplates must return a list: {body:?}")
        })
        .iter()
        .map(|template| template["prefix"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The rows of the create table: they differ along what the rule generalises over — how
/// the prefix is spelled, which optional members are present, how many scenarios the
/// template applies for, which mutability setting and how many exclusion filters, which
/// encryption type, and how many tags.
fn create_cases() -> Vec<TemplateCase> {
    let longest_description = "d".repeat(256);
    vec![
        (
            "only the required members",
            json!({"prefix": "prod", "appliedFor": ["PULL_THROUGH_CACHE"]}),
            json!({
                "prefix": "prod",
                "appliedFor": ["PULL_THROUGH_CACHE"],
                "imageTagMutability": "MUTABLE",
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
        (
            "ROOT, the registry-wide prefix",
            json!({"prefix": "ROOT", "appliedFor": ["CREATE_ON_PUSH"]}),
            json!({
                "prefix": "ROOT",
                "appliedFor": ["CREATE_ON_PUSH"],
                "imageTagMutability": "MUTABLE",
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
        (
            "a trailing slash is the assumed one, not part of the prefix",
            json!({"prefix": "stage/", "appliedFor": ["REPLICATION"]}),
            json!({
                "prefix": "stage",
                "appliedFor": ["REPLICATION"],
                "imageTagMutability": "MUTABLE",
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
        (
            "every scenario the model enumerates, with a description and a role",
            json!({
                "prefix": "team/inner.a_b__c--d",
                "appliedFor": APPLIED_FOR,
                "description": longest_description.clone(),
                "customRoleArn": "arn:aws:iam::000000000000:role/ecr-template",
            }),
            json!({
                "prefix": "team/inner.a_b__c--d",
                "appliedFor": APPLIED_FOR,
                "description": longest_description,
                "customRoleArn": "arn:aws:iam::000000000000:role/ecr-template",
                "imageTagMutability": "MUTABLE",
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
        (
            "KMS encryption with a caller's key",
            json!({
                "prefix": "kms-encrypted",
                "appliedFor": ["REPLICATION"],
                "encryptionConfiguration": {"encryptionType": "KMS", "kmsKey": KMS_KEY_ARN},
            }),
            json!({
                "prefix": "kms-encrypted",
                "appliedFor": ["REPLICATION"],
                "encryptionConfiguration": {"encryptionType": "KMS", "kmsKey": KMS_KEY_ARN},
                "imageTagMutability": "MUTABLE",
            }),
        ),
        (
            "AES256 encryption, which takes no key",
            json!({
                "prefix": "s3-encrypted",
                "appliedFor": ["CREATE_ON_PUSH"],
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
            json!({
                "prefix": "s3-encrypted",
                "appliedFor": ["CREATE_ON_PUSH"],
                "encryptionConfiguration": {"encryptionType": "AES256"},
                "imageTagMutability": "MUTABLE",
            }),
        ),
        (
            "immutable tags, both policies and two tags",
            json!({
                "prefix": "immutable",
                "appliedFor": ["CREATE_ON_PUSH", "REPLICATION"],
                "imageTagMutability": "IMMUTABLE",
                "repositoryPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "lifecyclePolicy": "{\"rules\":[]}",
                "resourceTags": [
                    {"Key": "env", "Value": "prod"},
                    {"Key": "team", "Value": "platform"},
                ],
            }),
            json!({
                "prefix": "immutable",
                "appliedFor": ["CREATE_ON_PUSH", "REPLICATION"],
                "imageTagMutability": "IMMUTABLE",
                "repositoryPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "lifecyclePolicy": "{\"rules\":[]}",
                "resourceTags": [
                    {"Key": "env", "Value": "prod"},
                    {"Key": "team", "Value": "platform"},
                ],
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
        (
            "mutable except one excluded tag pattern",
            json!({
                "prefix": "partly-immutable",
                "appliedFor": ["CREATE_ON_PUSH"],
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "release-*"},
                ],
            }),
            json!({
                "prefix": "partly-immutable",
                "appliedFor": ["CREATE_ON_PUSH"],
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "release-*"},
                ],
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
        (
            "immutable except the five filters the model's maximum admits",
            json!({
                "prefix": "mostly-immutable",
                "appliedFor": ["PULL_THROUGH_CACHE"],
                "imageTagMutability": "IMMUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "dev-*"},
                    {"filterType": "WILDCARD", "filter": "test-*"},
                    {"filterType": "WILDCARD", "filter": "tmp"},
                    {"filterType": "WILDCARD", "filter": "a.b_c-*"},
                    {"filterType": "WILDCARD", "filter": "nightly"},
                ],
            }),
            json!({
                "prefix": "mostly-immutable",
                "appliedFor": ["PULL_THROUGH_CACHE"],
                "imageTagMutability": "IMMUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "dev-*"},
                    {"filterType": "WILDCARD", "filter": "test-*"},
                    {"filterType": "WILDCARD", "filter": "tmp"},
                    {"filterType": "WILDCARD", "filter": "a.b_c-*"},
                    {"filterType": "WILDCARD", "filter": "nightly"},
                ],
                "encryptionConfiguration": {"encryptionType": "AES256"},
            }),
        ),
    ]
}

/// `CreateRepositoryCreationTemplate` persists what it was given and reports that
/// record; `DescribeRepositoryCreationTemplates` reports the same record unchanged, and
/// reading it twice changes nothing.
#[tokio::test]
async fn a_created_template_is_described_from_what_was_stored() {
    let router = emulator();

    for (case, request, expected) in create_cases() {
        let created = ok(&router, case, "CreateRepositoryCreationTemplate", &request).await;

        assert_eq!(
            created["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the create must report this registry, got {:?}",
            created["registryId"]
        );
        let template = &created["repositoryCreationTemplate"];
        assert_eq!(
            without_timestamps(template),
            expected,
            "case {case:?}: expected the create to report {expected:?}, got {:?}",
            without_timestamps(template)
        );
        let (created_at, updated_at) = timestamps(case, template);
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: nothing has updated the template, so createdAt and updatedAt \
             must be the same instant, got {created_at} and {updated_at}"
        );

        // Read it back the way a client does — by the prefix it was created with, which
        // may carry the assumed trailing slash.
        let described = ok(
            &router,
            case,
            "DescribeRepositoryCreationTemplates",
            &json!({"prefixes": [request["prefix"].as_str().unwrap_or_default()]}),
        )
        .await;
        assert_eq!(
            described["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the describe must report this registry, got {:?}",
            described["registryId"]
        );
        let listed = described["repositoryCreationTemplates"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {described:?}"));
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: exactly the one template of that prefix must be reported, got {listed:?}"
        );
        assert_eq!(
            &listed[0], template,
            "case {case:?}: the describe must report the created record unchanged, got {:?}",
            listed[0]
        );

        // A read reports; it does not change what a later caller sees.
        let described_again = ok(
            &router,
            case,
            "DescribeRepositoryCreationTemplates",
            &json!({"prefixes": [request["prefix"].as_str().unwrap_or_default()]}),
        )
        .await;
        assert_eq!(
            described_again, described,
            "case {case:?}: a second read must answer exactly what the first did, got {described_again:?}"
        );
    }

    // Every template created above is still there, listed once, in prefix order.
    let all = ok(
        &router,
        "every template",
        "DescribeRepositoryCreationTemplates",
        &json!({}),
    )
    .await;
    let listed = listed_prefixes(&all);
    let expected: BTreeSet<String> = create_cases()
        .iter()
        .map(|(_, _, expected)| expected["prefix"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed.iter().cloned().collect::<BTreeSet<String>>(),
        expected,
        "describing every template must report each created prefix once, got {listed:?}"
    );
    let mut sorted = listed.clone();
    sorted.sort();
    assert_eq!(
        listed, sorted,
        "the listing must be ordered by prefix, got {listed:?}"
    );
}

/// One template per namespace prefix: a second create for a prefix that already names
/// one is refused with `TemplateAlreadyExistsException`, and the stored template is left
/// as the first create made it. The rows differ in how the two prefixes are spelled —
/// the assumed trailing slash makes `stage` and `stage/` the same namespace.
#[tokio::test]
async fn a_second_create_for_one_prefix_is_refused() {
    let cases: Vec<(&str, &str, &str)> = vec![
        ("the same spelling twice", "prod", "prod"),
        ("a trailing slash on the second create", "stage", "stage/"),
        ("a trailing slash on the first create", "beta/", "beta"),
        ("the registry-wide prefix", "ROOT", "ROOT"),
        ("a namespaced prefix", "team/app", "team/app"),
    ];

    for (case, first, second) in cases {
        let router = emulator();
        let created = ok(
            &router,
            case,
            "CreateRepositoryCreationTemplate",
            &json!({"prefix": first, "appliedFor": ["CREATE_ON_PUSH"]}),
        )
        .await;

        let answer = call(
            &router,
            "CreateRepositoryCreationTemplate",
            &json!({"prefix": second, "appliedFor": ["REPLICATION"], "description": "second"}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a duplicate prefix must be refused with HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "TemplateAlreadyExistsException",
            "case {case:?}: expected TemplateAlreadyExistsException, got {:?}",
            answer.body
        );

        // The refused create changed nothing: the first template is still the one stored.
        let described = ok(
            &router,
            case,
            "DescribeRepositoryCreationTemplates",
            &json!({}),
        )
        .await;
        assert_eq!(
            described["repositoryCreationTemplates"],
            json!([created["repositoryCreationTemplate"]]),
            "case {case:?}: the registry must still hold exactly the first template, got {:?}",
            described["repositoryCreationTemplates"]
        );
    }
}

/// A `kmsKey` belongs to the KMS encryption types: `AES256` encrypts with Amazon
/// S3-managed keys, so a key beside it cannot be honoured and the create is refused with
/// `InvalidParameterException` — nothing persisted. The rows differ in the encryption
/// type, from `EncryptionType`'s own value list (`AES256`, `KMS`, `KMS_DSSE`).
#[tokio::test]
async fn a_kms_key_is_refused_for_the_s3_managed_encryption_type() {
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "AES256 with a key",
            json!({"encryptionType": "AES256", "kmsKey": KMS_KEY_ARN}),
            Some("InvalidParameterException"),
        ),
        (
            "AES256 with no key",
            json!({"encryptionType": "AES256"}),
            None,
        ),
        (
            "KMS with a key",
            json!({"encryptionType": "KMS", "kmsKey": KMS_KEY_ARN}),
            None,
        ),
        (
            "KMS with no key, which is the account's default ECR key",
            json!({"encryptionType": "KMS"}),
            None,
        ),
        (
            "KMS_DSSE with a key",
            json!({"encryptionType": "KMS_DSSE", "kmsKey": KMS_KEY_ARN}),
            None,
        ),
    ];

    for (case, encryption, refusal) in cases {
        let router = emulator();
        let request = json!({
            "prefix": "encrypted",
            "appliedFor": ["CREATE_ON_PUSH"],
            "encryptionConfiguration": encryption,
        });
        let answer = call(&router, "CreateRepositoryCreationTemplate", &request).await;

        match refusal {
            Some(code) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case:?}: expected {code}, got {:?}",
                    answer.body
                );
                let described = ok(
                    &router,
                    case,
                    "DescribeRepositoryCreationTemplates",
                    &json!({}),
                )
                .await;
                assert_eq!(
                    described["repositoryCreationTemplates"],
                    json!([]),
                    "case {case:?}: a refused create must persist nothing, got {:?}",
                    described["repositoryCreationTemplates"]
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["repositoryCreationTemplate"]["encryptionConfiguration"],
                    request["encryptionConfiguration"],
                    "case {case:?}: the stored template must carry the encryption \
                     configuration it was given, got {:?}",
                    answer.body["repositoryCreationTemplate"]["encryptionConfiguration"]
                );
            }
        }
    }
}

/// `DescribeRepositoryCreationTemplates` pages over the stored templates in prefix
/// order: every template is reported exactly once across the pages, whatever page size
/// the caller asks for, and a page is never larger than it asked for. The rows differ in
/// `maxResults` — the model's own minimum, sizes that do and do not divide the store, and
/// a size larger than it.
#[tokio::test]
async fn paging_reports_every_template_once() {
    let router = emulator();
    let prefixes = ["alpha", "beta", "delta", "epsilon", "gamma", "omega", "zeta"];
    for prefix in prefixes {
        ok(
            &router,
            prefix,
            "CreateRepositoryCreationTemplate",
            &json!({"prefix": prefix, "appliedFor": ["REPLICATION"]}),
        )
        .await;
    }

    for page_size in [1_i64, 2, 3, 7, 1000] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["nextToken"] = json!(token);
            }
            let page = ok(
                &router,
                &case,
                "DescribeRepositoryCreationTemplates",
                &request,
            )
            .await;
            let reported = listed_prefixes(&page);
            assert!(
                reported.len() as i64 <= page_size,
                "case {case:?}: a page must hold at most maxResults templates, got {reported:?}"
            );
            seen.extend(reported);
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            prefixes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: paging must report every template once, in prefix order, got {seen:?}"
        );
    }

    // The `prefixes` filter reports exactly the named templates; one that names no
    // template is reported as nothing, not as a failure.
    let filter_cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("one prefix", json!(["gamma"]), vec!["gamma"]),
        (
            "three prefixes, out of order",
            json!(["zeta", "alpha", "delta"]),
            vec!["alpha", "delta", "zeta"],
        ),
        (
            "a prefix with the assumed trailing slash",
            json!(["omega/"]),
            vec!["omega"],
        ),
        ("a prefix with no template", json!(["unknown"]), vec![]),
        (
            "a known and an unknown prefix",
            json!(["beta", "unknown"]),
            vec!["beta"],
        ),
    ];
    for (case, filter, expected) in filter_cases {
        let page = ok(
            &router,
            case,
            "DescribeRepositoryCreationTemplates",
            &json!({"prefixes": filter}),
        )
        .await;
        assert_eq!(
            listed_prefixes(&page),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed_prefixes(&page)
        );
    }

    // A token this registry never issued is refused rather than read as a page.
    let answer = call(
        &router,
        "DescribeRepositoryCreationTemplates",
        &json!({"nextToken": "not-a-token"}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a malformed nextToken must be refused with HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidParameterException",
        "expected InvalidParameterException for a malformed nextToken, got {:?}",
        answer.body
    );
}

/// A template whose `resourceTags` break a shared tag rule is refused and persists
/// nothing. The rows cover each rule the shared checks carry, from the `Tag`
/// documentation they cite: a key in AWS's reserved namespace, an over-long key, an
/// over-long value, and more tags than a repository may hold.
#[tokio::test]
async fn a_template_whose_tags_break_a_tag_rule_is_refused() {
    let many: Vec<Value> = (0..51)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let cases: Vec<(&str, Value)> = vec![
        (
            "a key in the reserved aws: namespace",
            json!([{"Key": "aws:created-by", "Value": "me"}]),
        ),
        (
            "a key past its maximum character length",
            json!([{"Key": "k".repeat(129), "Value": "v"}]),
        ),
        (
            "a value past its maximum length",
            json!([{"Key": "k", "Value": "v".repeat(257)}]),
        ),
        ("more tags than a repository may hold", json!(many)),
    ];

    for (case, resource_tags) in cases {
        let router = emulator();
        let answer = call(
            &router,
            "CreateRepositoryCreationTemplate",
            &json!({
                "prefix": "tagged",
                "appliedFor": ["CREATE_ON_PUSH"],
                "resourceTags": resource_tags,
            }),
        )
        .await;
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
        let described = ok(
            &router,
            case,
            "DescribeRepositoryCreationTemplates",
            &json!({}),
        )
        .await;
        assert_eq!(
            described["repositoryCreationTemplates"],
            json!([]),
            "case {case:?}: a refused create must persist nothing, got {:?}",
            described["repositoryCreationTemplates"]
        );
    }
}

/// The store is bounded: once it holds the maximum number of templates, a create is
/// refused with the `LimitExceededException` the operation models rather than adding
/// another record. The store is filled through the state struct, which is the same
/// arrangement a thousand creates would leave and is the only refusal path no wire
/// sequence can reach cheaply.
#[tokio::test]
async fn a_create_past_the_store_cap_is_refused() {
    let (router, engine) = emulator_with_engine();
    {
        let state = engine.state::<ComAmazonawsEcrAmazonec2containerregistryV20150921State>();
        let mut store = state.lock();
        let now = SystemTime::now();
        for index in 0..MAX_REPOSITORY_CREATION_TEMPLATES {
            let prefix = format!("filler-{index}");
            store.creation_templates.insert(
                prefix.clone(),
                CreationTemplate {
                    prefix,
                    description: None,
                    encryption_config: default_template_encryption(),
                    resource_tags: Vec::new(),
                    image_tag_mutability: "MUTABLE".to_string(),
                    image_tag_mutability_exclusion_filters: Vec::new(),
                    repository_policy: None,
                    lifecycle_policy: None,
                    applied_for: vec!["REPLICATION".to_string()],
                    custom_role_arn: None,
                    created_at: now,
                    updated_at: now,
                },
            );
        }
    }

    let answer = call(
        &router,
        "CreateRepositoryCreationTemplate",
        &json!({"prefix": "one-too-many", "appliedFor": ["CREATE_ON_PUSH"]}),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a create past the store cap must be refused with HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "LimitExceededException",
        "expected LimitExceededException at the store cap, got {:?}",
        answer.body
    );

    // The refused create added nothing: the prefix it named still names no template.
    let described = ok(
        &router,
        "one too many",
        "DescribeRepositoryCreationTemplates",
        &json!({"prefixes": ["one-too-many"]}),
    )
    .await;
    assert_eq!(
        described["repositoryCreationTemplates"],
        json!([]),
        "a refused create must persist nothing, got {:?}",
        described["repositoryCreationTemplates"]
    );
}
