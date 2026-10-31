// SPDX-License-Identifier: CC0-1.0

//! What `PutDeploymentParameter` answers in `tags` is the resource's STORED set.
//!
//! The rule under test: the tags an operation reports for a deployment parameter
//! are the set the record holds when the call is served — entry for entry, neither
//! filtered nor replaced by the tags the request carried — so the two members the
//! model gives that one set report the same thing. Which members those are
//! (`smithy_query(query="shape",shape="Tags,TagsMap",
//! model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")`):
//! `PutDeploymentParameterResponse$tags` targets the constrained `TagsMap`, whose
//! key and value shapes carry a `@length` and a `@pattern`, while
//! `ListTagsForResourceResponse$tags` targets `Tags`, a `map<String,String>` the
//! model leaves unconstrained — one resource's tags, two different statements about
//! what a tag may hold. Those constraints bind what a REQUEST may carry through a
//! member that targets them (the `unconstrained_tag_characters` suite owns that
//! contrast); they are not a filter on what the record holds, so an entry written
//! through `TagResource` — whose member targets the unconstrained map — is reported
//! by the next `PutDeploymentParameter` for the same identity as stored.
//!
//! Every row drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};

use crate::deployment_parameters::{
    emulator, list_tags, provision, put, tag, tag_map, tags_of, untag, CATALOG,
};

/// The one target every row below writes against.
const PRODUCT: &str = "prod-stored-tags";
/// The agreement covering [`PRODUCT`], recorded out of band as a write's target is.
const AGREEMENT: &str = "agmt-stored-tags";
/// The tag key every row's create carries, through this operation's own member, so
/// each row starts from a set the constrained map can express.
const BASELINE_KEY: &str = "env";
/// The tags an UPDATE carries in every row: the member documents that tags "will
/// only be applied for create operations, and they'll be ignored if the resource
/// already exists", so this key must appear in no response — which is what makes
/// the reported set the record's own rather than an echo of the request.
const IGNORED_KEY: &str = "ignored-at-update";

/// A tag map from owned pairs, so a row's key or value can be any string.
fn tags(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One pair as a tag map.
fn one(key: &str, value: &str) -> BTreeMap<String, String> {
    tags(&[(key.to_string(), value.to_string())])
}

/// The tag sets each row writes through the unconstrained member, spanning what the
/// rule generalises over: where the entry sits relative to the constrained copy's
/// character class (outside it on the key, on the value, on both, or inside it
/// altogether), the kind of character that puts it outside (a space, a letter
/// outside ASCII, a non-Latin script), the empty value the constrained copy's
/// `TagValue` `length min 1` excludes, each length bound reached in multi-byte
/// characters, and the number of entries at once.
///
/// The characters are the ones AWS documents for tags on a taggable resource
/// (letters, digits, spaces and `+ - = . _ : / @`); the length bounds are the
/// service's own `TAG_KEY_MAX_LEN` / `TAG_VALUE_MAX_LEN`.
fn stored_sets() -> Vec<(&'static str, BTreeMap<String, String>)> {
    vec![
        (
            "a key outside the constrained class, carrying a space",
            one("Cost Center", "useast1"),
        ),
        (
            "a value outside the constrained class, carrying a space",
            one("CostCenter", "us east 1"),
        ),
        (
            "a value that is empty, which the constrained class excludes",
            one("owner", ""),
        ),
        (
            "a key and a value of letters outside ASCII",
            one("Propriétaire", "Café Münster"),
        ),
        (
            "a key and a value in a non-Latin script",
            one("所有者", "サーバー 1"),
        ),
        (
            "a key of multi-byte characters at the length maximum",
            one(&"é".repeat(TAG_KEY_MAX_LEN), "v"),
        ),
        (
            "a value of multi-byte characters at the length maximum",
            one("multibyte-value", &"é".repeat(TAG_VALUE_MAX_LEN)),
        ),
        ("a key that is a single space", one(" ", "space")),
        (
            "several entries, each outside the constrained class in its own way",
            tags(&[
                ("Cost Center".to_string(), String::new()),
                ("Résumé".to_string(), "maître d'hôtel".to_string()),
                ("plain".to_string(), "plain".to_string()),
            ]),
        ),
        (
            "a set entirely inside the constrained class, so the rule is not about unusual characters",
            one("team", "platform"),
        ),
    ]
}

/// Create the deployment parameter `name` carrying the baseline tag, and answer the
/// ARN its create minted alongside the tag set that create reported.
async fn created_parameter(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) -> (String, BTreeMap<String, String>) {
    let (status, created) = put(
        router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": name, "secretString": "s" },
            "tags": { BASELINE_KEY: "dev" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} {created:?}"
    );
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "case {case:?}: the create must mint a resourceArn, got {created:?}"
    );
    (arn, tags_of(&created))
}

/// Rewrite `name` in place, carrying a tag set an update ignores, and answer the
/// tag set the response reported.
async fn updated_parameter(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) -> BTreeMap<String, String> {
    let (status, updated) = put(
        router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": name, "secretString": "s2" },
            "tags": { IGNORED_KEY: "yes" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the update in place must succeed, got {status} {updated:?}"
    );
    assert!(
        !tags_of(&updated).contains_key(IGNORED_KEY),
        "case {case:?}: an update's tags are ignored, so {IGNORED_KEY:?} must not be reported, got {updated:?}"
    );
    tags_of(&updated)
}

#[tokio::test]
async fn a_put_reports_the_stored_tag_set_entry_for_entry() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let baseline = tag_map(&[(BASELINE_KEY, "dev")]);

    for (index, (case, written)) in stored_sets().into_iter().enumerate() {
        let name = format!("StoredTags{index}");
        let (arn, reported_by_create) = created_parameter(&router, case, &name).await;
        assert_eq!(
            reported_by_create, baseline,
            "case {case:?}: the create must report the set it stored, got {reported_by_create:?}"
        );

        // The write that can leave on the record what this operation's own member
        // could not have carried: `TagResource$tags` targets the unconstrained map.
        let (status, answer) = tag(&router, &arn, &written).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the unconstrained member must store the set, got {status} {answer:?}"
        );
        let mut stored = baseline.clone();
        stored.extend(written.clone());

        let reported_by_update = updated_parameter(&router, case, &name).await;
        assert_eq!(
            reported_by_update, stored,
            "case {case:?}: expected the stored set {stored:?}, got {reported_by_update:?}"
        );

        // …and the member the model leaves unconstrained reports that same set, so
        // the two reporting paths of one record cannot answer differently.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            reported_by_update,
            "case {case:?}: the two reporting members must agree, got {listed:?} against {reported_by_update:?}"
        );

        // A removal is reported the same way: what the record holds after it is what
        // the next call answers with, which a response built from the request or
        // frozen at the create could not do.
        let removed: Vec<String> = written
            .keys()
            .filter(|key| !baseline.contains_key(*key))
            .cloned()
            .collect();
        let (status, answer) = untag(&router, &arn, &removed).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the same keys must be removable, got {status} {answer:?}"
        );
        let reported_after_removal = updated_parameter(&router, case, &name).await;
        assert_eq!(
            reported_after_removal, baseline,
            "case {case:?}: the removal must leave the baseline, got {reported_after_removal:?}"
        );
    }
}
