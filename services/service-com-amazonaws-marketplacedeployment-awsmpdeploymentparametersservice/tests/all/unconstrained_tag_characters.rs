// SPDX-License-Identifier: CC0-1.0

//! What a tagging member admits is decided by the map THAT member targets.
//!
//! The rule under test: a tag set arriving through a member whose target map the
//! model leaves unconstrained (`map<String,String>` — no `@length` and no
//! `@pattern` on either side) is held only to the tagging limits AWS applies to
//! every taggable resource — a key of 1..128 characters, a value of at most 256,
//! at most 50 tags on the resource, and no key in the namespace AWS reserves — so
//! any other UTF-8 key and value is stored as sent and listed back unchanged,
//! spaces, letters outside ASCII and an empty value included. A character class a
//! DIFFERENT member's target shape states constrains that member and no other.
//!
//! Which members that covers here (`smithy_query(query="operation",
//! operation="TagResource,ListTagsForResource,UntagResource,PutDeploymentParameter",
//! model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")`):
//! `TagResource$tags` and `ListTagsForResource$tags` target the unconstrained
//! `Tags`, and `UntagResource$tagKeys` an unconstrained `StringList`;
//! `PutDeploymentParameter`'s own `tags` target the constrained `TagsMap`, and the
//! last test here pins the contrast.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};

use crate::deployment_parameters::{
    emulator, error_code, field_name, list_tags, provision, put, tag, tag_map, tags_of, untag,
    CATALOG,
};

/// A tag map from owned pairs, so a row's key or value can be any string.
fn tags(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// One pair as a tag map.
fn one(key: &str, value: &str) -> BTreeMap<String, String> {
    tags(&[(key.to_string(), value.to_string())])
}

/// Create one deployment parameter carrying the baseline tag every row below
/// merges onto, and answer its ARN.
async fn tagged_parameter(
    router: &smithy_server_runtime::MultiServiceRouter,
    product_id: &str,
    agreement_id: &str,
    name: &str,
) -> String {
    let (status, created) = put(
        router,
        CATALOG,
        product_id,
        &json!({
            "agreementId": agreement_id,
            "deploymentParameter": { "name": name, "secretString": "s" },
            "tags": { "env": "dev" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup Put must succeed, got {status} {created:?}"
    );
    created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string()
}

/// Tag sets an unconstrained member admits, spanning what the rule generalises
/// over: where the unusual character sits (key, value, both), its kind (space,
/// letter outside ASCII, non-Latin script, the punctuation AWS's documented tag
/// character set names), the empty value, and each length bound reached with
/// MULTI-BYTE characters, which a bound counted in bytes would refuse.
///
/// The punctuation row's set is the one AWS documents for tags beside letters,
/// digits and spaces (`+ - = . _ : / @`); the length bounds are the service's own
/// `TAG_KEY_MAX_LEN` / `TAG_VALUE_MAX_LEN`.
fn accepted() -> Vec<(&'static str, BTreeMap<String, String>)> {
    vec![
        (
            "a key and a value carrying a space",
            one("Cost Center", "us east 1"),
        ),
        ("a value that is empty", one("owner", "")),
        (
            "a key and a value of letters outside ASCII",
            one("Propriétaire", "Café Münster"),
        ),
        (
            "a key and a value in a non-Latin script",
            one("所有者", "サーバー 1"),
        ),
        (
            "every punctuation character the documented tag set names",
            one("a/b+c=d.e_f:g@h-i", "a/b+c=d.e_f:g@h-i"),
        ),
        (
            "several pairs at once, each unusual in its own way",
            tags(&[
                ("Cost Center".to_string(), "".to_string()),
                ("Résumé".to_string(), "maître d'hôtel".to_string()),
                ("plain".to_string(), "plain".to_string()),
            ]),
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
    ]
}

#[tokio::test]
async fn an_unconstrained_tag_set_is_stored_and_listed_back() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-free", &["prod-free"]);
    let arn = tagged_parameter(&router, "prod-free", "agmt-free", "FreeTags").await;
    let baseline = tag_map(&[("env", "dev")]);

    for (case, written) in accepted() {
        let (status, answer) = tag(&router, &arn, &written).await;
        assert_eq!(
            status, 204,
            "case {case:?}: an unconstrained tag set must be accepted, got {status} {answer:?}"
        );

        let mut expected = baseline.clone();
        expected.extend(written.clone());
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: expected the stored set reported back, got {listed:?}"
        );

        // And the keys come off again: `UntagResource$tagKeys` is unconstrained too,
        // so what one operation stored the other can remove, leaving the baseline.
        let keys: Vec<String> = written.keys().cloned().collect();
        let (status, answer) = untag(&router, &arn, &keys).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the same keys must be removable, got {status} {answer:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            baseline,
            "case {case:?}: the removal must leave the baseline, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_tagging_limits_still_refuse_what_no_tag_may_carry() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-limits", &["prod-limits"]);
    let arn = tagged_parameter(&router, "prod-limits", "agmt-limits", "Limits").await;
    let baseline = tag_map(&[("env", "dev")]);

    // The limits an unconstrained member still keeps, each tried one character past
    // the bound — in multi-byte characters as well as ASCII, so a bound counted in
    // bytes would refuse the accepted rows above instead.
    let refused: Vec<(&str, BTreeMap<String, String>)> = vec![
        ("an empty key", one("", "v")),
        (
            "an ASCII key one character past the maximum",
            one(&"k".repeat(TAG_KEY_MAX_LEN + 1), "v"),
        ),
        (
            "a multi-byte key one character past the maximum",
            one(&"é".repeat(TAG_KEY_MAX_LEN + 1), "v"),
        ),
        (
            "an ASCII value one character past the maximum",
            one("k", &"v".repeat(TAG_VALUE_MAX_LEN + 1)),
        ),
        (
            "a multi-byte value one character past the maximum",
            one("k", &"é".repeat(TAG_VALUE_MAX_LEN + 1)),
        ),
        (
            "a key in the namespace AWS reserves",
            one("aws:cloudformation:stack-name", "mystack"),
        ),
    ];

    for (case, written) in refused {
        let (status, answer) = tag(&router, &arn, &written).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            field_name(&answer),
            "tags",
            "case {case:?}: the refusal must name the member that carried the tags, got {answer:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            baseline,
            "case {case:?}: a refused tag write must store nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_is_judged_by_the_map_its_member_targets() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-member", &["prod-member"]);
    let arn = tagged_parameter(&router, "prod-member", "agmt-member", "ByMember").await;

    // The same tag through the CONSTRAINED member is refused, because that member's
    // target map states a character class its keys and values must satisfy. The rows
    // differ along WHICH side carries the character the class excludes, because the
    // two are reported by different judgements: a map's values by the centralized
    // constraint walk, which names no `fieldName`, and its keys by this service's own
    // check, which blames the member that carried them.
    for (case, name, key, value, blamed) in [
        (
            "a key carrying a space",
            "ConstrainedKeySpace",
            "Cost Center",
            "useast1",
            Some("tags"),
        ),
        (
            "a key of letters outside ASCII",
            "ConstrainedKeyLetters",
            "Propriétaire",
            "moi",
            Some("tags"),
        ),
        (
            "a value carrying a space",
            "ConstrainedValueSpace",
            "CostCenter",
            "us east 1",
            None,
        ),
    ] {
        let (status, answer) = put(
            &router,
            CATALOG,
            "prod-member",
            &json!({
                "agreementId": "agmt-member",
                "deploymentParameter": { "name": name, "secretString": "s" },
                "tags": { key: value },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the constrained member must refuse it, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        if let Some(blamed) = blamed {
            assert_eq!(
                field_name(&answer),
                blamed,
                "case {case:?}: the refusal must be about the tags, got {answer:?}"
            );
        }

        // …while the unconstrained member stores it.
        let (status, answer) = tag(&router, &arn, &one(key, value)).await;
        assert_eq!(
            status, 204,
            "case {case:?}: the unconstrained member must accept it, got {status} {answer:?}"
        );
    }

    // A set the unconstrained member stored is reportable: a later
    // `PutDeploymentParameter` for the same identity answers with it.
    let (status, updated) = put(
        &router,
        CATALOG,
        "prod-member",
        &json!({
            "agreementId": "agmt-member",
            "deploymentParameter": { "name": "ByMember", "secretString": "s2" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the update must succeed, got {status} {updated:?}"
    );
    let mut expected = tag_map(&[("env", "dev")]);
    expected.extend(one("Cost Center", "useast1"));
    expected.extend(one("Propriétaire", "moi"));
    expected.extend(one("CostCenter", "us east 1"));
    assert_eq!(
        tags_of(&updated),
        expected,
        "the update must report every stored tag, got {updated:?}"
    );
}
