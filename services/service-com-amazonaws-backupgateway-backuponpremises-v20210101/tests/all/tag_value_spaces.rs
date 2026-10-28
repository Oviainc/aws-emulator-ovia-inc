// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one tag rule the modelled `Tag` shape states in prose and
//! none of its traits can express: a tag VALUE carries no space.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour
//! and the only way a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: a `Tag$Value` containing a space in any of its forms is refused
//! with the modelled `ValidationException` at HTTP 400 and nothing is written, on EVERY
//! operation of this service that accepts `Tags` — `TagResource`, `CreateGateway` and
//! `ImportHypervisorConfiguration` — while a value with no space is stored and read back
//! unchanged. The rule is the `Tag` shape's own sentence, "Spaces are not allowed in tag
//! values" (`smithy_query(query="shape",shape="Tag")`), which `TagValue`'s
//! `@pattern("^[^\x00]*$")` admits, so only the handler path can refuse it; a tag KEY is
//! the deliberate opposite, its pattern spelling the separators out as `\p{Z}`.
//!
//! The tables below span that class rather than one request: the space forms differ in
//! which space character they carry and where in the value it sits (between words,
//! leading, trailing, alone, at the length maximum), and the accepted values differ in
//! character class, script and length, including both of `TagValue`'s modelled length
//! bounds.

use std::collections::BTreeMap;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, create_body, gateway_count, hypervisor_count,
    issued_activation_key, pending_activation_count, Reply,
};
use crate::gateway_server_association::message_of;

/// A host no other row of this suite imports, so each import mints its own record.
fn host_of(nonce: usize) -> String {
    format!("10.1.{}.{}", nonce / 256, nonce % 256)
}

/// Put one tag on an existing resource through `TagResource` — the only way a client
/// writes a tag onto a resource it already created.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, key: &str, value: &str) -> Reply {
    call(
        router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": [{ "Key": key, "Value": value }] }),
    )
    .await
}

/// `CreateGateway` carrying one tag, on an activation this service really issued.
async fn create_gateway_with_tag(
    router: &MultiServiceRouter,
    display_name: &str,
    key: &str,
    value: &str,
) -> Reply {
    let activation_key = issued_activation_key(router).await;
    let mut body = create_body(display_name, &activation_key, &BTreeMap::new());
    body["Tags"] = json!([{ "Key": key, "Value": value }]);
    call(router, "CreateGateway", &body).await
}

/// `ImportHypervisorConfiguration` carrying one tag.
async fn import_hypervisor_with_tag(
    router: &MultiServiceRouter,
    name: &str,
    host: &str,
    key: &str,
    value: &str,
) -> Reply {
    call(
        router,
        "ImportHypervisorConfiguration",
        &json!({
            "Name": name,
            "Host": host,
            "Tags": [{ "Key": key, "Value": value }],
        }),
    )
    .await
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn listed_tags(router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let reply = call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "ListTagsForResource must answer 200 for {arn}, got {} {:?}",
        reply.status, reply.body
    );
    reply.tags()
}

/// Values that carry a space, the refusal side of the rule.
///
/// A space is not only `U+0020`: the rows span the Unicode space family — the separators
/// the modelled `TagKey` pattern spells out as `\p{Z}`, where a KEY is deliberately
/// allowed them (`smithy_query(query="shape",shape="TagKey")`), together with the tab and
/// line forms — and they differ in where the space sits. The last row puts one at
/// `TagValue`'s `@length(max: 256)` boundary, so a value the length constraint accepts is
/// still refused for the space it ends on.
fn spaced_values() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = [
        ("an ASCII space between words", "a value with spaces"),
        ("a leading ASCII space", " leading"),
        ("a trailing ASCII space", "trailing "),
        ("nothing but one ASCII space", " "),
        ("several consecutive ASCII spaces", "a   b"),
        ("a horizontal tab", "two\tfields"),
        ("a line feed", "first\nsecond"),
        ("a carriage return", "first\rsecond"),
        ("a no-break space (U+00A0)", "non\u{00a0}breaking"),
        ("an en space (U+2002)", "en\u{2002}space"),
        ("an ideographic space (U+3000)", "ideographic\u{3000}space"),
    ]
    .into_iter()
    .map(|(case, value)| (case.to_string(), value.to_string()))
    .collect();
    rows.push((
        "a space at the 256-character length maximum".to_string(),
        format!("{} ", "v".repeat(255)),
    ));
    rows
}

/// Values with no space, the acceptance side of the rule: the space rule must refuse
/// nothing else.
///
/// The character catalog is the `Tag` doc's own list of allowed characters — "UTF-8
/// letters, numbers, and the following characters: + - = . _ : /"
/// (`smithy_query(query="shape",shape="Tag")`) — and the lengths are both bounds of
/// `TagValue`'s `@length(min: 0, max: 256)`.
fn accepted_values() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = [
        ("every character the Tag doc lists beside letters and digits", "+-=._:/"),
        ("letters and digits", "prod42"),
        ("letters of a non-latin script", "значение"),
        ("the empty value, TagValue's modelled length minimum", ""),
        ("a multi-word value joined without spaces", "a-value-without-spaces"),
    ]
    .into_iter()
    .map(|(case, value)| (case.to_string(), value.to_string()))
    .collect();
    rows.push((
        "a 256-character value, TagValue's modelled length maximum".to_string(),
        "v".repeat(256),
    ));
    rows
}

/// Assert one refusal: 400, the modelled `ValidationException`, and a message that names
/// the member and the value it refused.
fn assert_refused(case: &str, operation: &str, value: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: {operation} must refuse the tag value {value:?} with 400 \
         ValidationException, got {} {:?}",
        reply.status,
        reply.body
    );
    let stated = message_of(reply);
    assert!(
        stated.contains("at 'Tags' failed to satisfy constraint: "),
        "case {case:?}: {operation} must state which member failed its constraint, got \
         {stated:?}"
    );
    assert!(
        stated.contains(value),
        "case {case:?}: {operation} must report the value {value:?} it refused, got \
         {stated:?}"
    );
}

/// A tag value carrying a space is refused on every operation that accepts `Tags`, and
/// the refusal writes nothing: no tag on the resource being tagged, no gateway, no
/// hypervisor, and no activation spent.
#[tokio::test]
async fn a_tag_value_carrying_a_space_is_refused_wherever_tags_are_accepted() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Live resources of both types this service tags, each already carrying a tag — so a
    // refusal below means "this value", and "nothing was written" means the tag set the
    // resource had is the tag set it still has.
    let gateway = create(&router, "gw-tag-spaces").await;
    let hypervisor = import_hypervisor_with_tag(
        &router,
        "hv-tag-spaces",
        &host_of(1),
        "env",
        "test",
    )
    .await
    .str_at("/HypervisorArn")
    .expect("the import must mint a HypervisorArn")
    .to_string();
    let accepted = tag_resource(&router, &gateway, "env", "test").await;
    assert_eq!(
        accepted.status, 200,
        "the seed tag must be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    let seeded: BTreeMap<String, String> = [("env".to_string(), "test".to_string())].into();

    let gateways_before = gateway_count(&engine);
    let hypervisors_before = hypervisor_count(&engine);

    for (index, (case, value)) in spaced_values().into_iter().enumerate() {
        for (resource, arn) in [("gateway", &gateway), ("hypervisor", &hypervisor)] {
            let refused = tag_resource(&router, arn, "env", &value).await;
            assert_refused(&case, &format!("TagResource on a {resource}"), &value, &refused);
            let listed = listed_tags(&router, arn).await;
            assert_eq!(
                listed, seeded,
                "case {case:?}: a refused TagResource must leave the {resource} carrying \
                 {seeded:?}, ListTagsForResource reported {listed:?}"
            );
        }

        // A create whose tags are refused spends nothing: the activation it named stays
        // pending for the appliance to retry with.
        let pending_before = pending_activation_count(&engine);
        let refused = create_gateway_with_tag(
            &router,
            &format!("gw-spaced-{index}"),
            "env",
            &value,
        )
        .await;
        assert_refused(&case, "CreateGateway", &value, &refused);
        assert_eq!(
            pending_activation_count(&engine),
            pending_before + 1,
            "case {case:?}: a refused CreateGateway must leave its activation pending"
        );

        let refused = import_hypervisor_with_tag(
            &router,
            &format!("hv-spaced-{index}"),
            &host_of(index + 2),
            "env",
            &value,
        )
        .await;
        assert_refused(&case, "ImportHypervisorConfiguration", &value, &refused);

        assert_eq!(
            (gateway_count(&engine), hypervisor_count(&engine)),
            (gateways_before, hypervisors_before),
            "case {case:?}: a refused request must create neither a gateway nor a \
             hypervisor, the store now holds {} and {}",
            gateway_count(&engine),
            hypervisor_count(&engine)
        );
    }
}

/// The acceptance side: a value with no space is stored and read back unchanged, through
/// each of the three operations that accept `Tags`.
#[tokio::test]
async fn a_tag_value_without_a_space_round_trips_through_the_store() {
    let engine = build_engine();
    let router = build_router(&engine);
    let gateway = create(&router, "gw-tag-accepted").await;

    for (index, (case, value)) in accepted_values().into_iter().enumerate() {
        let key = format!("k{index}");
        let expected: BTreeMap<String, String> = [(key.clone(), value.clone())].into();

        let tagged = tag_resource(&router, &gateway, &key, &value).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must accept the value {value:?}, got {} {:?}",
            tagged.status, tagged.body
        );
        let listed = listed_tags(&router, &gateway).await;
        assert_eq!(
            listed.get(&key),
            Some(&value),
            "case {case:?}: the tagged value must read back unchanged, \
             ListTagsForResource reported {listed:?}"
        );

        let created =
            create_gateway_with_tag(&router, &format!("gw-accepted-{index}"), &key, &value).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateGateway must accept the value {value:?}, got {} {:?}",
            created.status, created.body
        );
        let created_arn = created.gateway_arn().to_string();
        let listed = listed_tags(&router, &created_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the created gateway must carry {expected:?}, \
             ListTagsForResource reported {listed:?}"
        );

        let imported = import_hypervisor_with_tag(
            &router,
            &format!("hv-accepted-{index}"),
            &host_of(index + 100),
            &key,
            &value,
        )
        .await;
        assert_eq!(
            imported.status, 200,
            "case {case:?}: ImportHypervisorConfiguration must accept the value \
             {value:?}, got {} {:?}",
            imported.status, imported.body
        );
        let imported_arn = imported
            .str_at("/HypervisorArn")
            .expect("the import must mint a HypervisorArn")
            .to_string();
        let listed = listed_tags(&router, &imported_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the imported hypervisor must carry {expected:?}, \
             ListTagsForResource reported {listed:?}"
        );
    }
}

/// The two halves of a tag are not symmetric: a KEY may carry spaces — its modelled
/// pattern admits `\p{Z}` — while its VALUE may not, whatever the key looks like.
#[tokio::test]
async fn a_tag_key_may_carry_spaces_where_its_value_may_not() {
    let engine = build_engine();
    let router = build_router(&engine);
    let gateway = create(&router, "gw-spaced-keys").await;

    // (case, the key, a value with no space) — keys differing in where the space sits and
    // in script, all of them inside `TagKey`'s pattern.
    let rows: [(&str, &str, &str); 3] = [
        ("a space between two words of the key", "cost center", "ops"),
        ("a leading space in the key", " leading", "kept"),
        ("a space in a non-latin key", "Ключ 1", "значение"),
    ];

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (case, key, value) in rows {
        let tagged = tag_resource(&router, &gateway, key, value).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: a spaced tag KEY must be accepted, got {} {:?}",
            tagged.status, tagged.body
        );
        expected.insert(key.to_string(), value.to_string());

        // The same key with a spaced VALUE is refused, so the acceptance above is about
        // the key and not about spaces in general.
        let refused = tag_resource(&router, &gateway, key, "two words").await;
        assert_refused(case, "TagResource", "two words", &refused);
    }

    let listed = listed_tags(&router, &gateway).await;
    assert_eq!(
        listed, expected,
        "the spaced keys must all be stored with their values, ListTagsForResource \
         reported {listed:?}"
    );
}
