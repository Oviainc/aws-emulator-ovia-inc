// SPDX-License-Identifier: CC0-1.0

//! The `Tags` a CREATE carries, read back through `ListTagsForResource` over the
//! `awsJson1_1` wire.
//!
//! `resource_tags.rs` owns the tagging slice as the tagging operations drive it. This
//! suite pins the other way a resource acquires tags: every create of a taggable
//! resource takes an optional `Tags` member, and the rule is that the set the create
//! persisted is the set the read reports — one entry per distinct `Tag$Key`, in key
//! order, with no `NextToken` while the whole set fits one page — for every kind of
//! resource the model's `TaggableResourceArn` admits, including the instance, whose tags
//! only a create can attach at enablement time.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call, empty_account_router, organization_router};

/// The provider a customer managed application is created under: the `custom` entry of
/// the published catalog (`state::BUILT_IN_APPLICATION_PROVIDERS`), the one
/// `CreateApplication` serves.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `TrustedTokenIssuerType` enum admits
/// (`smithy_query(query="shape",shape="TrustedTokenIssuerType")` → `["OIDC_JWT"]`).
const ISSUER_TYPE: &str = "OIDC_JWT";

/// The only value the model's `JwksRetrievalOption` enum admits
/// (`smithy_query(query="shape",shape="OidcJwtConfiguration")`).
const JWKS_RETRIEVAL_OPTION: &str = "OPEN_ID_DISCOVERY";

/// One tag set a create carries: a row of the table.
struct TagSet {
    label: &'static str,
    /// `None` is a create that carries no `Tags` member at all.
    tags: Option<Vec<(String, String)>>,
}

impl TagSet {
    fn new(label: &'static str, tags: &[(&str, &str)]) -> Self {
        Self {
            label,
            tags: Some(
                tags.iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            ),
        }
    }

    fn untagged(label: &'static str) -> Self {
        Self { label, tags: None }
    }

    fn from_pairs(label: &'static str, tags: Vec<(String, String)>) -> Self {
        Self {
            label,
            tags: Some(tags),
        }
    }

    /// The `Tags` member of a create request for this row, when it carries one.
    fn request_tags(&self) -> Option<Value> {
        self.tags.as_ref().map(|tags| {
            Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"Key": key, "Value": value}))
                    .collect(),
            )
        })
    }

    /// What the read must report: this row's tags in TAG-KEY order, which is the order
    /// the enumeration's page key gives them.
    fn expected(&self) -> Value {
        let mut tags = self.tags.clone().unwrap_or_default();
        tags.sort_by(|left, right| left.0.cmp(&right.0));
        Value::Array(
            tags.iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        )
    }
}

/// The rows the rule is asserted over.
///
/// They differ along what it generalises over: the NUMBER of tags (none at all, one,
/// several, and the 75 `TagList` admits at its upper bound), the ORDER the create sent
/// them in (a tag set carries none), the LENGTH of a key at its `TagKey` upper bound
/// (`length 1..128`) and of a value at both `TagValue` bounds (`length 0..256`, so an
/// empty value is admitted), and the CHARACTER CLASSES both patterns admit
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`: letters of any script, separators, digits and
/// `_ . : / = + - @`). The constraints are from
/// `smithy_query(query="shape",shape="Tag,TagList")`.
fn tag_sets() -> Vec<TagSet> {
    vec![
        TagSet::untagged("a create that carries no Tags member at all"),
        TagSet::new("a single tag", &[("Owner", "platform")]),
        TagSet::new(
            "three tags sent in descending key order",
            &[("zone", "eu"), ("owner", "platform"), ("Env", "staging")],
        ),
        TagSet::new(
            "a key at its 128-character bound and an empty value",
            &[(&"k".repeat(128), ""), ("filled", "value")],
        ),
        TagSet::new(
            "keys and values spanning the character classes the patterns admit",
            &[
                ("cost centre", "42"),
                ("path/to:thing", "a=b+c-d@e.f"),
                ("Caf\u{e9}_\u{41e}\u{442}\u{434}\u{435}\u{43b}", "na\u{ef}ve"),
                ("padded", &"v".repeat(256)),
            ],
        ),
        TagSet::from_pairs(
            "75 tags, the most TagList admits",
            (0..75)
                .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
                .collect(),
        ),
    ]
}

/// The 200 body of a `ListTagsForResource` for `resource_arn`.
async fn tags_of(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "ListTagsForResource",
        &json!({"ResourceArn": resource_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource must answer 200 for {resource_arn}, got \
         {status} {body}"
    );
    body
}

/// Create a permission set carrying `tags` under `instance_arn`; answer its ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
    tags: Option<Value>,
) -> String {
    let mut body = json!({"InstanceArn": instance_arn, "Name": name});
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let (status, response) = call(router, "CreatePermissionSet", &body).await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?} must answer 200, got {status} {response}"
    );
    response["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet minted no ARN: {response}"))
        .to_string()
}

/// Create an application carrying `tags` under `instance_arn`; answer its ARN.
async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
    tags: Option<Value>,
) -> String {
    let mut body = json!({
        "InstanceArn": instance_arn,
        "ApplicationProviderArn": PROVIDER_CUSTOM,
        "Name": name,
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let (status, response) = call(router, "CreateApplication", &body).await;
    assert_eq!(
        status, 200,
        "CreateApplication {name:?} must answer 200, got {status} {response}"
    );
    response["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication minted no ARN: {response}"))
        .to_string()
}

/// Create a trusted token issuer carrying `tags` under `instance_arn`; answer its ARN.
async fn create_trusted_token_issuer(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
    tags: Option<Value>,
) -> String {
    let mut body = json!({
        "InstanceArn": instance_arn,
        "Name": name,
        "TrustedTokenIssuerType": ISSUER_TYPE,
        "TrustedTokenIssuerConfiguration": {
            "OidcJwtConfiguration": {
                "IssuerUrl": "https://issuer.example.com/oidc",
                "ClaimAttributePath": "email",
                "IdentityStoreAttributePath": "emails.primary.value",
                "JwksRetrievalOption": JWKS_RETRIEVAL_OPTION,
            }
        },
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let (status, response) = call(router, "CreateTrustedTokenIssuer", &body).await;
    assert_eq!(
        status, 200,
        "CreateTrustedTokenIssuer {name:?} must answer 200, got {status} {response}"
    );
    response["TrustedTokenIssuerArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateTrustedTokenIssuer minted no ARN: {response}"))
        .to_string()
}

/// Create the account's instance carrying `tags`; answer its ARN.
async fn create_instance(router: &MultiServiceRouter, tags: Option<Value>) -> String {
    let mut body = json!({"Name": "tagged-instance"});
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    let (status, response) = call(router, "CreateInstance", &body).await;
    assert_eq!(
        status, 200,
        "CreateInstance must answer 200, got {status} {response}"
    );
    response["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInstance minted no ARN: {response}"))
        .to_string()
}

/// The tags a create of a permission set, an application or a trusted token issuer
/// persisted are exactly the tags the read reports, for every shape of tag set — and a
/// repeat of the read answers identically, because the read is a report.
#[tokio::test]
async fn a_created_resource_reports_the_tag_set_its_create_persisted() {
    let (router, instance_arn) = organization_router();

    for (index, set) in tag_sets().into_iter().enumerate() {
        let permission_set = create_permission_set(
            &router,
            &instance_arn,
            &format!("ps-row-{index}"),
            set.request_tags(),
        )
        .await;
        let application = create_application(
            &router,
            &instance_arn,
            &format!("application row {index}"),
            set.request_tags(),
        )
        .await;
        let issuer = create_trusted_token_issuer(
            &router,
            &instance_arn,
            &format!("issuer-row-{index}"),
            set.request_tags(),
        )
        .await;

        for (kind, resource_arn) in [
            ("permission set", &permission_set),
            ("application", &application),
            ("trusted token issuer", &issuer),
        ] {
            let case = format!("{} of a {kind}", set.label);
            let body = tags_of(&router, &case, resource_arn).await;
            assert_eq!(
                body["Tags"],
                set.expected(),
                "case {case:?}: expected the persisted tags in key order {}, got {body}",
                set.expected()
            );
            assert!(
                body["NextToken"].is_null(),
                "case {case:?}: a tag set that fits one page carries no NextToken, got {body}"
            );

            let again = tags_of(&router, &case, resource_arn).await;
            assert_eq!(
                again, body,
                "case {case:?}: a repeated read must change nothing a later caller sees, \
                 got {again}"
            );

            // A resource reports its OWN tags: the three resources of this row carry the
            // same set, and each answers for itself rather than for the one created last.
            let scoped = call(
                &router,
                "ListTagsForResource",
                &json!({"InstanceArn": instance_arn, "ResourceArn": resource_arn}),
            )
            .await;
            assert_eq!(
                scoped.0, 200,
                "case {case:?}: the read scoped to the holding instance must answer 200, \
                 got {} {}",
                scoped.0, scoped.1
            );
            assert_eq!(
                scoped.1, body,
                "case {case:?}: naming the holding instance must answer the same tags, got {}",
                scoped.1
            );
        }
    }
}

/// The same rule for the INSTANCE kind: an instance acquires its tags at enablement, so
/// `CreateInstance` is the only write that can attach them before any tagging call.
#[tokio::test]
async fn a_created_instance_reports_the_tag_set_its_create_persisted() {
    for set in tag_sets() {
        // An account holds ONE instance, so each row gets its own account: the shared
        // harness's "this account holds none" setup.
        let router = empty_account_router();
        let instance_arn = create_instance(&router, set.request_tags()).await;
        let case = format!("{} of an instance", set.label);
        let body = tags_of(&router, &case, &instance_arn).await;
        assert_eq!(
            body["Tags"],
            set.expected(),
            "case {case:?}: expected the persisted tags in key order {}, got {body}",
            set.expected()
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: a tag set that fits one page carries no NextToken, got {body}"
        );
    }
}
