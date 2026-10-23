// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetTags`: the tag collection of a resource, read by ARN.
//!
//! The RULE under test: `GetTags` reports the tag collection the resource its
//! `resourceArn` names has PERSISTED — whichever taggable kind that resource is, however
//! many tags it carries and whatever form their keys and values take — and reports it
//! from that record, so reading it changes nothing a later caller sees. A `resourceArn`
//! this service cannot read as the ARN of one of its taggable resources is a bad request
//! (400 `BadRequestException`); a well-formed such ARN that no stored record answers to
//! is a missing resource (404 `NotFoundException`).
//!
//! Which resources are taggable comes from the model — a resource is taggable exactly
//! when the shape declared for it carries a `tags` member
//! (`smithy_query(query="shape", shape="RestApi,Stage,ApiKey,UsagePlan,DomainName,\
//! ClientCertificate,VpcLink,DomainNameAccessAssociation")`) — and an ARN's resource
//! portion is the URI of the operation that reads that resource
//! (`smithy_query(query="operation", operation="GetRestApi,GetStage,GetApiKey")`:
//! `/restapis/{restApiId}`, `/restapis/{restApiId}/stages/{stageName}`,
//! `/apikeys/{apiKey}`). Those three are the kinds this emulator stores records for, so
//! the table spans all three; the kinds whose creates are still scaffolds can hold no
//! record and are covered among the 404 rows.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`] and [`string_member`] are this crate's one
//! wire-test harness, reused rather than standing a second emulator up another way.

use serde_json::{json, Map, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};

/// The region every ARN in this suite names: the one the harness' requests carry in
/// their credential scope.
const REGION: &str = "us-east-1";

/// An API Gateway ARN for the resource portion `path`. API Gateway's own ARNs carry an
/// EMPTY account segment — `arn:aws:apigateway:{region}::/restapis/{id}` — which is the
/// form an SDK puts in a tag call's `resourceArn`.
fn apigateway_arn(path: &str) -> String {
    format!("arn:aws:apigateway:{REGION}::{path}")
}

/// Percent-encode one URI path segment the way an AWS SDK encodes a non-greedy
/// `@httpLabel`: encoding every non-alphanumeric byte is a superset of what a label needs
/// and covers the `:` and `/` of an ARN, so the whole ARN travels as `%3A`/`%2F` inside
/// ONE path segment of `/tags/{resourceArn}`. The encoding itself is the
/// `percent-encoding` crate's.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `GetTags` for `arn`.
async fn get_tags(gateway: &Gateway, arn: &str) -> Answer {
    gateway
        .call("GET", &format!("/tags/{}", encode_label(arn)), None)
        .await
}

/// `GetTags` for `arn` with the two members the operation documents as
/// "(Not currently supported)".
async fn get_tags_paged(gateway: &Gateway, arn: &str, limit: i32, position: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!(
                "/tags/{}?limit={limit}&position={}",
                encode_label(arn),
                encode_label(position)
            ),
            None,
        )
        .await
}

/// The tag keys and values this suite tags resources with, differing in the form the
/// `tags` member's own documentation allows: "The key-value map of strings. The valid
/// character set is [a-zA-Z+-=._:/]. The tag key can be up to 128 characters and must
/// not start with `aws:`. The tag value can be up to 256 characters."
/// (`smithy_query(query="doc", shape="CreateRestApiRequest", member="tags")`). The last
/// rows sit on the two published length bounds; the first is the plain recorded form.
///
/// The non-ASCII row is in the table on the corpus' own evidence, not on the doc's
/// character-set sentence: the recorded units `apigateway.tag-resource-unicode-key-value-
/// round-trip`, `apigateway.tag-value-unicode-round-trip` and
/// `apigateway.tags-unicode-value-round-trip` (listed on the sibling `TagResource` stub
/// issue) are AWS round-tripping exactly such keys and values, so what this suite asserts
/// of them — that a stored tag is reported back unchanged — is what the service does.
/// Which keys a tag write REFUSES is `TagResource`'s question, not this read's.
fn tag_forms() -> Vec<(String, String)> {
    vec![
        ("Name".to_string(), "value".to_string()),
        ("cost:center".to_string(), "team/platform".to_string()),
        ("a+b-c=d._e".to_string(), "+-=._:/".to_string()),
        ("empty-value".to_string(), String::new()),
        ("ünïcødé".to_string(), "ünïcødé-值".to_string()),
        (
            "k".repeat(MAX_TAG_KEY_CHARS),
            "v".repeat(MAX_TAG_VALUE_CHARS),
        ),
    ]
}

/// The first `count` [`tag_forms`] as a `tags` member.
fn tag_map(count: usize) -> Value {
    Value::Object(
        tag_forms()
            .into_iter()
            .take(count)
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// One row of the table: the kind's case name, the ARN that names the created resource,
/// and the URI that resource's OWN read is served at.
struct TaggedResource {
    case: String,
    arn: String,
    own_read: String,
}

/// Create one resource of each taggable kind this emulator stores, each carrying `tags`,
/// and answer the row for each: a `RestApi` (`CreateRestApi`), a `Stage` of it
/// (`CreateDeployment` then `CreateStage`) and an `ApiKey` (`CreateApiKey`). Every
/// record is created OVER THE WIRE, so what `GetTags` later reports is what the service
/// persisted.
async fn tagged_resources_of_every_kind(
    gateway: &Gateway,
    label: &str,
    tags: &Value,
) -> Vec<TaggedResource> {
    let created = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": format!("tagged-{label}"), "tags": tags })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {label}: expected 201 from CreateRestApi, got {} with {:?}",
        created.status, created.body
    );
    let api = string_member(&created.body, "id", label);

    let deployment = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "description": "the deployment the stage points at" })),
        )
        .await;
    assert_eq!(
        deployment.status, 201,
        "case {label}: expected 201 from CreateDeployment, got {} with {:?}",
        deployment.status, deployment.body
    );
    let deployment_id = string_member(&deployment.body, "id", label);
    let stage_name = "prod";
    let stage = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({
                "stageName": stage_name,
                "deploymentId": deployment_id,
                "tags": tags,
            })),
        )
        .await;
    assert_eq!(
        stage.status, 201,
        "case {label}: expected 201 from CreateStage, got {} with {:?}",
        stage.status, stage.body
    );

    let key = gateway
        .call(
            "POST",
            "/apikeys",
            Some(json!({ "name": format!("tagged-key-{label}"), "tags": tags })),
        )
        .await;
    assert_eq!(
        key.status, 201,
        "case {label}: expected 201 from CreateApiKey, got {} with {:?}",
        key.status, key.body
    );
    let key_id = string_member(&key.body, "id", label);

    vec![
        TaggedResource {
            case: format!("RestApi / {label}"),
            arn: apigateway_arn(&format!("/restapis/{api}")),
            own_read: format!("/restapis/{api}"),
        },
        TaggedResource {
            case: format!("Stage / {label}"),
            arn: apigateway_arn(&format!("/restapis/{api}/stages/{stage_name}")),
            own_read: format!("/restapis/{api}/stages/{stage_name}"),
        },
        TaggedResource {
            case: format!("ApiKey / {label}"),
            arn: apigateway_arn(&format!("/apikeys/{key_id}")),
            own_read: format!("/apikeys/{key_id}"),
        },
    ]
}

#[tokio::test]
async fn get_tags_reports_the_collection_the_named_resource_persisted() {
    let gateway = Gateway::new();
    // Rows differ along what the rule generalises over: the KIND of resource the ARN
    // names, how MANY tags that resource carries (none, one, several, every form) and
    // the FORM of the keys and values. The one-tag row of the `RestApi` kind is the
    // recorded case.
    for count in [0usize, 1, 3, tag_forms().len()] {
        let tags = tag_map(count);
        let label = format!("{count}-tags");
        for resource in tagged_resources_of_every_kind(&gateway, &label, &tags).await {
            let case = &resource.case;
            // Read TWICE: the collection is reported from the record every time, so a
            // read never changes what the next caller sees.
            for round in 0..2 {
                let answer = get_tags(&gateway, &resource.arn).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case} (round {round}): expected 200 from GetTags, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["tags"], tags,
                    "case {case} (round {round}): expected {tags:?}, got {:?}",
                    answer.body["tags"]
                );
            }

            // The SAME record the resource's own read reports, so there is one tag
            // collection per resource and not two. A kind that omits an empty `tags`
            // member from its own body reports nothing to compare for the no-tags row,
            // which `GetTags` still answers as the empty collection above.
            let own = gateway.call("GET", &resource.own_read, None).await;
            assert_eq!(
                own.status, 200,
                "case {case}: expected 200 from the resource's own read, got {} with {:?}",
                own.status, own.body
            );
            if !own.body["tags"].is_null() {
                assert_eq!(
                    own.body["tags"], tags,
                    "case {case}: expected the resource's own read to report {tags:?}, got {:?}",
                    own.body["tags"]
                );
            }
        }
    }
}

#[tokio::test]
async fn get_tags_reports_the_whole_collection_whatever_limit_and_position_ask_for() {
    // `limit` and `position` are each documented "(Not currently supported)" on this
    // operation's request shape, so neither cuts a page out of the collection: every
    // row asks for a page and gets the whole map.
    let gateway = Gateway::new();
    let tags = tag_map(tag_forms().len());
    let resources = tagged_resources_of_every_kind(&gateway, "paged", &tags).await;
    let pages: &[(i32, &str)] = &[(1, "Name"), (2, ""), (500, "cost:center")];
    for resource in &resources {
        for (limit, position) in pages {
            let case = format!("{} / limit={limit} position={position:?}", resource.case);
            let answer = get_tags_paged(&gateway, &resource.arn, *limit, position).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 from GetTags, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["tags"], tags,
                "case {case}: expected the whole collection {tags:?}, got {:?}",
                answer.body["tags"]
            );
        }
    }
}

#[tokio::test]
async fn get_tags_refuses_a_resource_arn_it_cannot_read_as_a_taggable_resource() {
    let gateway = Gateway::new();
    // Every row is a value that is NOT the ARN of a taggable resource of this service:
    // not an ARN at all, an ARN of another service, an ARN missing a segment, and API
    // Gateway ARNs whose resource portion names something this service does not tag (an
    // API's resource, a deployment, a usage plan key, an unknown collection, a
    // collection with no identifier). None of them can name a record, so each is a bad
    // request rather than a missing resource.
    let cases: Vec<(&str, String)> = vec![
        ("not an arn", "petstore".to_string()),
        (
            "an arn of another service",
            "arn:aws:s3:::a-bucket".to_string(),
        ),
        (
            "one segment short",
            format!("arn:aws:apigateway:{REGION}:/restapis/abcde12345"),
        ),
        (
            "no arn prefix",
            format!("aws:apigateway:{REGION}::/restapis/abcde12345"),
        ),
        (
            "no partition",
            format!("arn::apigateway:{REGION}::/restapis/abcde12345"),
        ),
        ("no resource portion", apigateway_arn("")),
        (
            "a resource portion that is not a path",
            apigateway_arn("restapis/abcde12345"),
        ),
        ("a collection with no identifier", apigateway_arn("/restapis")),
        (
            "an untaggable resource of an api",
            apigateway_arn("/restapis/abcde12345/resources/fghij67890"),
        ),
        (
            "an untaggable deployment of an api",
            apigateway_arn("/restapis/abcde12345/deployments/fghij67890"),
        ),
        (
            "an untaggable usage plan key",
            apigateway_arn("/usageplans/abcde12345/keys/fghij67890"),
        ),
        (
            "an unknown collection",
            apigateway_arn("/widgets/abcde12345"),
        ),
    ];
    for (case, arn) in &cases {
        let answer = get_tags(&gateway, arn).await;
        assert_failure(&answer, 400, "BadRequestException", case);
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(arn.as_str()),
            "case {case}: expected the refusal to name the rejected {arn:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn get_tags_answers_not_found_for_a_taggable_arn_no_record_answers_to() {
    let gateway = Gateway::new();
    let tags = tag_map(2);
    let existing = tagged_resources_of_every_kind(&gateway, "not-found", &tags).await;
    let api = existing[0]
        .own_read
        .trim_start_matches("/restapis/")
        .to_string();

    // Every row is a well-formed ARN of a taggable API Gateway resource that no stored
    // record answers to: an unknown identifier of a kind this emulator stores, a stage
    // name no stage of an existing API has, and the kinds whose creates are still
    // scaffolds, which can therefore hold no record at all.
    let cases: Vec<(&str, String)> = vec![
        ("an unknown rest api", apigateway_arn("/restapis/zzzzz99999")),
        (
            "an unknown stage of an existing api",
            apigateway_arn(&format!("/restapis/{api}/stages/staging")),
        ),
        (
            "a stage of an unknown api",
            apigateway_arn("/restapis/zzzzz99999/stages/prod"),
        ),
        ("an unknown api key", apigateway_arn("/apikeys/zzzzz99999")),
        (
            "a usage plan, a kind with no records",
            apigateway_arn("/usageplans/zzzzz99999"),
        ),
        (
            "a domain name, a kind with no records",
            apigateway_arn("/domainnames/api.example.com"),
        ),
        (
            "a client certificate, a kind with no records",
            apigateway_arn("/clientcertificates/zzzzz99999"),
        ),
        (
            "a vpc link, a kind with no records",
            apigateway_arn("/vpclinks/zzzzz99999"),
        ),
        (
            "a domain name access association, a kind with no records",
            apigateway_arn("/domainnameaccessassociations/zzzzz99999"),
        ),
    ];
    for (case, arn) in &cases {
        let answer = get_tags(&gateway, arn).await;
        assert_failure(&answer, 404, "NotFoundException", case);
    }

    // The refusals left every stored collection exactly as it was.
    for resource in &existing {
        let answer = get_tags(&gateway, &resource.arn).await;
        assert_eq!(
            answer.body["tags"], tags,
            "case {}: expected {tags:?} after the refusals, got {:?}",
            resource.case, answer.body["tags"]
        );
    }
}
