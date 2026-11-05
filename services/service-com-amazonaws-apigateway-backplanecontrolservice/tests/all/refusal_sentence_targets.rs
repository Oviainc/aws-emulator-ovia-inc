// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH record, condition and contract this control plane's 400 and
//! 404 refusals name, and for the sentences it names them in.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the per-resource
//! create and read helpers are this crate's existing harness.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one call:
//!
//! * a call addressed by a DOCUMENTATION VERSION label resolves that snapshot as one
//!   lookup, so an unresolvable target is reported as the documentation version — the
//!   record the URI's leaf names — and not as the API that would hold it (a call addressed
//!   by the API itself still reports the API identifier, which is where the rule stops);
//! * a stage's `copy` is the canary promotion, whose legal `(path, from)` pairs are a
//!   constant of the Stage patch grammar: a `copy` outside those pairs is refused with one
//!   sentence that quotes the submitted `path` and `from` and enumerates both legal sets,
//!   and each legal pair applies and reads back;
//! * every violation of the tag key/value contract earns ONE fixed sentence stating that
//!   contract, naming neither the offending key nor its length nor the rule it broke;
//! * the VALUE a patch carries for an ARN-typed member is read as an ARN before the
//!   op/path grammar is judged: a value that is no ARN earns the unreadable-ARN sentence,
//!   and a value that IS one gets past it and earns the grammar refusal — the path, the op
//!   and the paths the resource publishes for that op;
//! * a value this service cannot read as an ARN earns ONE fixed sentence, whichever member
//!   carried it and whatever the value was;
//! * a refusal that enumerates a resource's published paths enumerates the set belonging to
//!   the SUBMITTED op, and the paths it offers are the ones that op applies.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::account_settings::{get_account, role_arn};
use crate::authorizer_lifecycle::{created_authorizer, get_authorizer, token_request, user_pool_arn};
use crate::documentation_versions::{create_version, snapshot_request};
use crate::patch_updates::{op, patch};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};

/// The modelled error codes these cases assert.
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// The sentence a call addressed by a documentation version answers for a snapshot it
/// cannot resolve: entirely fixed text, naming neither the version nor the API.
const NO_SUCH_DOCUMENTATION_VERSION: &str = "Invalid Documentation version specified";

/// The paths a stage's `copy` may name, in the order the stage's own refusal enumerates
/// them.
const COPY_PATHS: [&str; 2] = ["/deploymentId", "/variables"];

/// The pointers a stage's `copy` may read FROM, in that same order: the canary's deployment
/// and the canary's stage variable overrides, the two halves of a canary promotion
/// (`PatchOperation$from`: "to promote a canary deployment, you copy the canary deployment
/// ID to the affiliated deployment ID … with "op":"copy",
/// "from":"/canarySettings/deploymentId" and "path":"/deploymentId"").
const COPY_FROM_PATHS: [&str; 2] = [
    "/canarySettings/deploymentId",
    "/canarySettings/stageVariableOverrides",
];

/// Assert `answer` is the modelled failure `(status, code)` carrying exactly `message`.
fn assert_refusal(answer: &Answer, expected: (u16, &str), message: &str, case: &str) {
    assert_failure(answer, expected.0, expected.1, case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the message {message:?}, got {:?}",
        answer.body["message"]
    );
}

/// The sentence a stage refuses a `copy` outside its copy grammar with.
fn invalid_copy(path: &str, from: &str) -> String {
    format!(
        "Invalid copy operation with path: {path} and from {from}. Valid copy:path are \
         [{}] and valid copy:from are [{}]",
        COPY_PATHS.join(", "),
        COPY_FROM_PATHS.join(", ")
    )
}

/// The one sentence every tag key/value violation earns. Entirely fixed text: the bounds
/// are the contract's, not one request's, and no rejected key, value or length appears.
fn invalid_tag() -> String {
    format!(
        "Invalid tag key/value. Valid characters are letters, numbers, spaces, and the \
         following: _ . : / = + - @. Tag key can be up to {MAX_TAG_KEY_CHARS} characters. \
         Tag value can be up to {MAX_TAG_VALUE_CHARS} characters."
    )
}

/// `TagResource` for `arn` with the one tag `key`/`value`.
async fn tag_resource(gateway: &Gateway, arn: &str, key: &str, value: &str) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({ "tags": { key: value } })),
        )
        .await
}

/// `UntagResource` for `arn`, removing the one `key`.
async fn untag_resource(gateway: &Gateway, arn: &str, key: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/tags/{}?tagKeys={}", encode_label(arn), encode_label(key)),
            None,
        )
        .await
}

/// `UpdateAccount` with one patch operation, whatever the service answers — the raw form of
/// the sibling `account_settings` suite's `patch_account`, which asserts the 200 a refused
/// row here does not get.
async fn patch_account_raw(gateway: &Gateway, kind: &str, path: &str, value: Option<&str>) -> Answer {
    patch(gateway, "/account", vec![op(kind, path, value)]).await
}

#[tokio::test]
async fn a_call_addressed_by_a_documentation_version_reports_that_version_as_missing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "documentation-version-leaf").await;
    let (other, _other_root) = create_api(&gateway, "documentation-version-leaf-other").await;
    let created = create_version(&gateway, &api, snapshot_request("1.0.0", Some("stored"))).await;
    assert_eq!(
        created.status, 201,
        "case setup: expected 201 from CreateDocumentationVersion, got {} with {:?}",
        created.status, created.body
    );

    // Rows differ along WHICH half of the pair does not resolve and in the form it takes:
    // the API alone, the version alone, both, a version stored under ANOTHER API, and a
    // version differing from the stored one only in case.
    let missing_api = "zzzzzzzzzz";
    let rows: Vec<(String, String, String)> = vec![
        (
            "an api that was never minted".to_string(),
            missing_api.to_string(),
            "1.0.0".to_string(),
        ),
        (
            "a version the stored api does not have".to_string(),
            api.clone(),
            "2.0.0".to_string(),
        ),
        (
            "neither the api nor the version".to_string(),
            missing_api.to_string(),
            "2.0.0".to_string(),
        ),
        (
            "a version stored under another api".to_string(),
            other.clone(),
            "1.0.0".to_string(),
        ),
        (
            "a version differing only in case".to_string(),
            api.clone(),
            "1.0.0-A".to_string(),
        ),
    ];

    // …and the rule holds for every operation the version label addresses: the read, the
    // update and the delete.
    for (case, target_api, version) in &rows {
        let uri = format!("/restapis/{target_api}/documentation/versions/{version}");
        let calls: Vec<(&str, Answer)> = vec![
            (
                "GetDocumentationVersion",
                gateway.call("GET", &uri, None).await,
            ),
            (
                "UpdateDocumentationVersion",
                patch(&gateway, &uri, vec![op("replace", "/description", Some("x"))]).await,
            ),
            (
                "DeleteDocumentationVersion",
                gateway.call("DELETE", &uri, None).await,
            ),
        ];
        for (operation, answer) in calls {
            assert_refusal(
                &answer,
                NOT_FOUND,
                NO_SUCH_DOCUMENTATION_VERSION,
                &format!("{operation} / {case}"),
            );
        }
    }

    // Where the rule STOPS: a call addressed by the API rather than by one of its snapshots
    // reports the API identifier, qualified by the account, as every other identifier
    // lookup of this service does.
    let listed = gateway
        .call(
            "GET",
            &format!("/restapis/{missing_api}/documentation/versions"),
            None,
        )
        .await;
    assert_refusal(
        &listed,
        NOT_FOUND,
        &crate::rest_api_identifier_refusals::no_such_api(missing_api),
        "GetDocumentationVersions / an api that was never minted",
    );

    // And the stored snapshot is still served, so the refusals above are about the
    // identifiers and not about the requests.
    let served = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/versions/1.0.0"),
            None,
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case the stored snapshot: expected 200, got {} with {:?}",
        served.status, served.body
    );
}

#[tokio::test]
async fn a_copy_outside_the_stages_copy_grammar_is_refused_with_both_legal_sets() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-copy-grammar").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let uri = format!("/restapis/{api}/stages/prod");

    // Rows differ along WHY the copy is outside the grammar: a legal path with an illegal
    // `from`, a legal path with the OTHER pair's legal `from`, a path outside the set, a
    // pointer reaching into a legal path, a structure of the stage, a method-setting path,
    // and a copy carrying no `from` at all.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("a legal path from a member of the stage", "/deploymentId", Some("/description")),
        (
            "a legal path from the other pair's source",
            "/deploymentId",
            Some("/canarySettings/stageVariableOverrides"),
        ),
        (
            "the other legal path from the first pair's source",
            "/variables",
            Some("/canarySettings/deploymentId"),
        ),
        (
            "a path outside the copy set",
            "/description",
            Some("/canarySettings/deploymentId"),
        ),
        (
            "a pointer reaching into a legal path",
            "/variables/color",
            Some("/canarySettings/stageVariableOverrides"),
        ),
        (
            "a nested structure of the stage",
            "/accessLogSettings",
            Some("/canarySettings/deploymentId"),
        ),
        (
            "a method-setting path",
            "/~1pets/GET/metrics/enabled",
            Some("/canarySettings/deploymentId"),
        ),
        ("a copy carrying no from at all", "/deploymentId", None),
    ];

    for (case, path, from) in &rows {
        let mut operation = json!({ "op": "copy", "path": path });
        if let (Some(target), Some(from)) = (operation.as_object_mut(), from) {
            target.insert("from".to_string(), Value::String((*from).to_string()));
        }
        let answer = patch(&gateway, &uri, vec![operation]).await;
        // A `copy` with no `from` quotes an empty source back, which is what the request
        // sent.
        let expected = invalid_copy(path, from.unwrap_or_default());
        assert_refusal(
            &answer,
            BAD_REQUEST,
            &expected,
            &format!("UpdateStage(copy {path} from {from:?}) / {case}"),
        );
    }

    // The other side of the rule: each legal pair the sentence enumerates applies, and the
    // stage reports it afterwards.
    let second = create_deployed_stage(&gateway, &api, "next").await;
    let staged = patch(
        &gateway,
        &uri,
        vec![
            op("replace", "/canarySettings/deploymentId", Some(&second)),
            op("replace", "/canarySettings/stageVariableOverrides/table", Some("pets")),
            op("replace", "/variables/table", Some("kittens")),
            op("replace", "/variables/kept", Some("as it was")),
        ],
    )
    .await;
    assert_eq!(
        staged.status, 200,
        "case the canary to promote: expected 200, got {} with {:?}",
        staged.status, staged.body
    );

    let promoted = patch(
        &gateway,
        &uri,
        vec![
            json!({ "op": "copy", "path": "/deploymentId", "from": COPY_FROM_PATHS[0] }),
            json!({ "op": "copy", "path": "/variables", "from": COPY_FROM_PATHS[1] }),
        ],
    )
    .await;
    assert_eq!(
        promoted.status, 200,
        "case the canary promotion: expected 200, got {} with {:?}",
        promoted.status, promoted.body
    );
    let read = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        read.status, 200,
        "case the canary promotion: expected 200 from GetStage, got {} with {:?}",
        read.status, read.body
    );
    for (member, expected) in [
        ("deploymentId", json!(second)),
        ("variables", json!({ "table": "pets", "kept": "as it was" })),
    ] {
        assert_eq!(
            read.body[member], expected,
            "case the canary promotion: expected GetStage to report {member} as {expected:?}, \
             got {:?}",
            read.body[member]
        );
    }
}

#[tokio::test]
async fn every_tag_contract_violation_earns_the_one_tag_contract_sentence() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "tag-contract-sentence").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // Rows differ along WHICH half of the contract is broken and by how much — a key one
    // character past its bound, a key far past it, a value one past its bound, a value far
    // past it, and both at once — because the sentence is the same for all of them and
    // carries none of those numbers.
    let past_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS * 3);
    let past_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS * 3);
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a key one character past its bound", past_key.as_str(), "v"),
        ("a key far past its bound", long_key.as_str(), "v"),
        ("a value one character past its bound", "k", past_value.as_str()),
        ("a value far past its bound", "k", long_value.as_str()),
        ("both past their bounds", past_key.as_str(), past_value.as_str()),
    ];
    // …and along the resource the tags were offered to, because the contract is the
    // service's rather than one resource's: the ARN travels in the request's own path label.
    let resources = [format!("/restapis/{api}"), format!("/restapis/{api}/stages/prod")];

    let expected = invalid_tag();
    for resource in &resources {
        let arn = apigateway_arn(resource);
        for (case, key, value) in &rows {
            let answer = tag_resource(&gateway, &arn, key, value).await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                &expected,
                &format!("TagResource({resource}) / {case}"),
            );
        }
        // A key past the bound is the same contract on the way back OUT.
        let answer = untag_resource(&gateway, &arn, past_key.as_str()).await;
        assert_refusal(
            &answer,
            BAD_REQUEST,
            &expected,
            &format!("UntagResource({resource}) / a key one character past its bound"),
        );
    }

    // The same contract where the tags ride on a CREATE, so a tag this service refuses on
    // one path cannot be stored through another.
    for (case, key, value) in &rows {
        let answer = gateway
            .call(
                "POST",
                "/restapis",
                Some(json!({ "name": "tagged-on-create", "tags": { (*key): value } })),
            )
            .await;
        assert_refusal(
            &answer,
            BAD_REQUEST,
            &expected,
            &format!("CreateRestApi(tags) / {case}"),
        );
    }

    // AT the two bounds the tag is stored and read back, so the refusals above are the
    // contract's bounds and not a blanket refusal of long tags.
    let at_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let at_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let arn = apigateway_arn(&format!("/restapis/{api}"));
    let accepted = tag_resource(&gateway, &arn, &at_key, &at_value).await;
    assert_eq!(
        accepted.status, 204,
        "case at both bounds: expected 204 from TagResource, got {} with {:?}",
        accepted.status, accepted.body
    );
    let tags = get_tags(&gateway, &arn).await;
    assert_eq!(
        tags.body["tags"][&at_key],
        json!(at_value),
        "case at both bounds: expected GetTags to read the stored tag back, got {:?}",
        tags.body
    );
}

#[tokio::test]
async fn a_patch_values_arn_is_read_before_the_op_path_grammar_is_judged() {
    let gateway = Gateway::new();

    // Rows differ along the OP that cannot name the path — the two a single-valued member
    // does not take — and along the VALUE the refused op carries: one the member would
    // take, one of another service, an empty one (which names no ARN to read) and none at
    // all. Each of these gets PAST the value check, so what it earns is that op's grammar
    // refusal, enumerating the paths the account publishes for the op that was submitted.
    let takeable = role_arn("takeable");
    let readable: Vec<(&str, Option<&str>)> = vec![
        ("a value the member would take", Some(takeable.as_str())),
        (
            "an ARN of another service",
            Some("arn:aws:logs:us-east-1:000000000000:log-group:/aws/apigateway/welcome"),
        ),
        ("an empty value", Some("")),
        ("no value at all", None),
    ];
    // …and the values that are no ARN at all, which the value check stops FIRST: the
    // unreadable-ARN sentence is what they earn, under the same ops.
    let unreadable: Vec<(&str, Option<&str>)> = vec![
        ("a value that is not an ARN at all", Some("not-an-arn")),
        ("a value of too few ARN fields", Some("arn:aws:iam")),
    ];
    for kind in ["add", "remove"] {
        let grammar = format!(
            "Invalid patch path  '/cloudwatchRoleArn' specified for op '{kind}'. \
             Must be one of: [/cloudwatchRoleArn]"
        );
        for (case, value) in &readable {
            let answer = patch_account_raw(&gateway, kind, "/cloudwatchRoleArn", *value).await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                &grammar,
                &format!("UpdateAccount({kind} /cloudwatchRoleArn) / {case}"),
            );
        }
        for (case, value) in &unreadable {
            let answer = patch_account_raw(&gateway, kind, "/cloudwatchRoleArn", *value).await;
            assert_refusal(
                &answer,
                BAD_REQUEST,
                "Invalid ARN specified in the request",
                &format!("UpdateAccount({kind} /cloudwatchRoleArn) / {case}"),
            );
        }
    }

    // The account is untouched by every refused row: not one of them was applied.
    let account = get_account(&gateway).await;
    assert_eq!(
        account["cloudwatchRoleArn"],
        Value::Null,
        "case after the refused rows: expected no cloudwatchRoleArn, got {account:?}"
    );

    // Where the op/path grammar DOES admit the path, the member's own, narrower rule is
    // applied to the value: the ARN has to name an IAM role.
    let refused = patch_account_raw(
        &gateway,
        "replace",
        "/cloudwatchRoleArn",
        Some("arn:aws:iam::000000000000:user/bob"),
    )
    .await;
    assert_refusal(
        &refused,
        BAD_REQUEST,
        "Invalid ARN specified in the request",
        "UpdateAccount(replace /cloudwatchRoleArn) / an ARN that names no role",
    );
    let accepted =
        patch_account_raw(&gateway, "replace", "/cloudwatchRoleArn", Some(takeable.as_str())).await;
    assert_eq!(
        accepted.status, 200,
        "case a replace carrying a value the member takes: expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.body["cloudwatchRoleArn"],
        json!(takeable),
        "case a replace carrying a value the member takes: expected the role back, got {:?}",
        accepted.body
    );
}

#[tokio::test]
async fn a_value_this_service_cannot_read_as_an_arn_earns_one_fixed_sentence() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "unreadable-arn-sentence").await;

    // The sentence is entirely fixed text: it carries neither the value that could not be
    // read nor the member that carried it.
    const EXPECTED: &str = "Invalid ARN specified in the request";

    // Rows differ along the FORM of the value: a plain string and an ARN of too few fields
    // are no ARN at all, so every member that takes an ARN of a STORED record refuses them;
    // an ARN of another service and an API Gateway ARN whose resource portion names nothing
    // taggable are well-formed ARNs that are not the ARN the TAG operations take, so those
    // three refuse them too.
    let no_arn_at_all = ["not-a-valid-arn", "arn:aws:apigateway"];
    let not_a_taggable_resource = [
        "arn:aws:s3:::a-bucket".to_string(),
        apigateway_arn("/nosuchkind/abcde12345"),
    ];

    for value in no_arn_at_all
        .iter()
        .map(|value| (*value).to_string())
        .chain(not_a_taggable_resource.iter().cloned())
    {
        // …and along the MEMBER it rides on: the `resourceArn` of each tag operation — a
        // write, a removal and a read.
        let value = value.as_str();
        let tagged = tag_resource(&gateway, value, "owner", "team").await;
        assert_refusal(
            &tagged,
            BAD_REQUEST,
            EXPECTED,
            &format!("TagResource({value:?})"),
        );
        let untagged = untag_resource(&gateway, value, "owner").await;
        assert_refusal(
            &untagged,
            BAD_REQUEST,
            EXPECTED,
            &format!("UntagResource({value:?})"),
        );
        let read = get_tags(&gateway, value).await;
        assert_refusal(&read, BAD_REQUEST, EXPECTED, &format!("GetTags({value:?})"));
    }

    // A `targetArns` entry that is no ARN has the MEMBER's own sentence instead: a VPC
    // link's target is the ARN of a load balancer rather than of a record this service
    // stores, and API Gateway states that the ARN is not well formed, quoting the value
    // back. See the `vpc_link_target_arns` rows of `refusal_contract_sentences`.
    for value in no_arn_at_all {
        let linked = crate::vpc_links::create_vpc_link(
            &gateway,
            json!({ "name": "unreadable-target", "targetArns": [value] }),
        )
        .await;
        assert_refusal(
            &linked,
            BAD_REQUEST,
            &format!("Invalid ARN. ARN is not well formed {value}"),
            &format!("CreateVpcLink(targetArns {value:?})"),
        );
    }

    // The same sentence for the one ARN a patch VALUE carries, so the refusal is the
    // service's and not one member's.
    let patched = patch_account_raw(
        &gateway,
        "replace",
        "/cloudwatchRoleArn",
        Some("not-a-valid-arn"),
    )
    .await;
    assert_refusal(
        &patched,
        BAD_REQUEST,
        EXPECTED,
        "UpdateAccount(replace /cloudwatchRoleArn)",
    );

    // A readable ARN of a stored resource is served, so the refusals above are about the
    // values and not about the calls.
    let served = get_tags(&gateway, &apigateway_arn(&format!("/restapis/{api}"))).await;
    assert_eq!(
        served.status, 200,
        "case a readable ARN: expected 200 from GetTags, got {} with {:?}",
        served.status, served.body
    );
}

#[tokio::test]
async fn an_authorizers_path_refusal_enumerates_the_paths_the_submitted_op_applies() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "authorizer-add-paths").await;
    let authorizer = created_authorizer(
        &gateway,
        &api,
        "CreateAuthorizer",
        json!({
            "name": "pool",
            "type": "COGNITO_USER_POOLS",
            "identitySource": "method.request.header.Authorization",
            "providerARNs": [user_pool_arn("us-east-1_first")],
        }),
    )
    .await;
    let authorizer = string_member(&authorizer, "id", "CreateAuthorizer");
    let uri = format!("/restapis/{api}/authorizers/{authorizer}");

    // The paths an authorizer publishes for an `add`: its multi-valued members, in the
    // order the refusal enumerates them.
    const ADD_PATHS: [&str; 2] = ["/providerARNs", "/identitySource"];

    // Rows differ along the FORM of the unapplicable path — a member no shape declares, the
    // member the service assigns, a pointer reaching into a single-valued member, a path
    // that is no JSON Pointer, an empty path — and each is refused with the set belonging to
    // the op that named it.
    let paths = [
        "/sim2realNoSuchField",
        "/id",
        "/name/first",
        "notAPointer",
        "",
    ];
    for path in paths {
        let answer = patch(&gateway, &uri, vec![op("add", path, Some("x"))]).await;
        let expected = format!(
            "Invalid patch path  '{path}' specified for op 'add'. Must be one of: [{}]",
            ADD_PATHS.join(", ")
        );
        assert_refusal(
            &answer,
            BAD_REQUEST,
            &expected,
            &format!("UpdateAuthorizer(add {path:?})"),
        );
        assert!(
            !ADD_PATHS.contains(&path),
            "case UpdateAuthorizer(add {path:?}): the enumerated list {ADD_PATHS:?} must not \
             hold the rejected path"
        );
    }

    // The other side of the rule: each path the `add` refusal offers takes an `add`, and the
    // authorizer reports it afterwards.
    let second = user_pool_arn("us-east-1_second");
    let added = patch(
        &gateway,
        &uri,
        vec![op("add", "/providerARNs", Some(&second))],
    )
    .await;
    assert_eq!(
        added.status, 200,
        "case UpdateAuthorizer(add /providerARNs): expected 200, got {} with {:?}",
        added.status, added.body
    );
    let read = get_authorizer(&gateway, &api, &authorizer).await;
    assert_eq!(
        read.status, 200,
        "case UpdateAuthorizer(add /providerARNs): expected 200 from GetAuthorizer, got {} \
         with {:?}",
        read.status, read.body
    );
    let pools = read.body["providerARNs"]
        .as_array()
        .map(|arns| arns.len())
        .unwrap_or_default();
    assert_eq!(
        pools, 2,
        "case UpdateAuthorizer(add /providerARNs): expected both pools, got {:?}",
        read.body["providerARNs"]
    );
    let source = "method.request.header.Auth";
    let sourced = patch(
        &gateway,
        &uri,
        vec![op("add", "/identitySource", Some(source))],
    )
    .await;
    assert_eq!(
        sourced.status, 200,
        "case UpdateAuthorizer(add /identitySource): expected 200, got {} with {:?}",
        sourced.status, sourced.body
    );
    let read = get_authorizer(&gateway, &api, &authorizer).await;
    assert_eq!(
        read.body["identitySource"],
        json!(source),
        "case UpdateAuthorizer(add /identitySource): expected GetAuthorizer to read the \
         identity source back, got {:?}",
        read.body
    );

    // A TOKEN authorizer's own refusal reads the same way, so the published set is the
    // resource's and not one record's.
    let token = created_authorizer(&gateway, &api, "CreateAuthorizer(token)", token_request("tok"))
        .await;
    let token = string_member(&token, "id", "CreateAuthorizer(token)");
    let answer = patch(
        &gateway,
        &format!("/restapis/{api}/authorizers/{token}"),
        vec![op("add", "/sim2realNoSuchField", Some("x"))],
    )
    .await;
    assert_refusal(
        &answer,
        BAD_REQUEST,
        &format!(
            "Invalid patch path  '/sim2realNoSuchField' specified for op 'add'. \
             Must be one of: [{}]",
            ADD_PATHS.join(", ")
        ),
        "UpdateAuthorizer(add /sim2realNoSuchField) on a TOKEN authorizer",
    );
}
