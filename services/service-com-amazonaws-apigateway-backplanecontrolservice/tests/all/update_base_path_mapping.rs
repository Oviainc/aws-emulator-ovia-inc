// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateBasePathMapping`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the PATCH wrappers, the domain-name and
//! stage creates and the base-path-mapping create/read helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle`, `patch_updates`, `domain_name_collection`,
//! `stage_lifecycle` and `base_path_mapping`.
//!
//! The RULE under test: `UpdateBasePathMapping` applies every entry of `patchOperations`
//! to the stored `BasePathMapping` the `domainName` + `basePath` pair resolves to, in
//! request order, and reports the patched record — which a later `GetBasePathMapping`
//! reads back at the base path the record now answers to — while a patch the service
//! cannot apply is refused with a modelled error and leaves the stored record exactly as
//! it was, the base path staying unique across the domain name's mappings and the record
//! still pointing at an API, and a stage of that API, that exist.
//!
//! The cases assert that rule over SETS of inputs rather than one recorded call: a table of
//! members, ops, starting records and list lengths; the base-path spellings a mapping is
//! addressed and re-addressed by (named, `(none)`, empty, one at the published character
//! bound and one past it); the ops and paths this resource does not support; the
//! identifiers that resolve to no record; and the uniqueness, idempotence and
//! leave-the-siblings-alone invariants.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    BASE_PATH_MAPPING_ALREADY_EXISTS, INVALID_REST_API_IDENTIFIER, MAX_PATH_CHARS, NONE_BASE_PATH,
    NO_SUCH_BASE_PATH_MAPPING, NO_SUCH_DOMAIN_NAME, NO_SUCH_STAGE,
};

use crate::base_path_mapping::{
    assert_refusal, create_mapping, deployed_api, get_mapping, mapping_of,
};
use crate::domain_name_collection::created_domain;
use crate::patch_updates::{op, patch, replace};
use crate::rest_api_lifecycle::{assert_failure, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;

const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");
const CONFLICT: (u16, &str) = (409, "ConflictException");

/// The `UpdateBasePathMapping`/`GetBasePathMapping` URI for one mapping of one domain
/// name.
fn mapping_uri(domain_name: &str, base_path: &str) -> String {
    format!("/domainnames/{domain_name}/basepathmappings/{base_path}")
}

/// `UpdateBasePathMapping` on the mapping `domain_name` + `base_path` addresses.
async fn update(
    gateway: &Gateway,
    domain_name: &str,
    base_path: &str,
    operations: Vec<Value>,
) -> Answer {
    patch(gateway, &mapping_uri(domain_name, base_path), operations).await
}

/// Create the domain name `domain_name` and one mapping on it from `request`, asserting
/// the modelled 201.
async fn baseline(gateway: &Gateway, case: &str, domain_name: &str, request: Value) {
    created_domain(gateway, domain_name, json!({})).await;
    let created = create_mapping(gateway, domain_name, request).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected 201 from CreateBasePathMapping, got {} with {:?}",
        created.status, created.body
    );
}

/// The whole modelled record a read of `domain_name` + `base_path` reports, asserting the
/// modelled 200.
async fn read_mapping(
    gateway: &Gateway,
    case: &str,
    domain_name: &str,
    base_path: &str,
) -> (String, String, Option<String>) {
    let read = get_mapping(gateway, domain_name, base_path).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetBasePathMapping({base_path:?}), got {} with {:?}",
        read.status, read.body
    );
    mapping_of(&read.body, case)
}

#[tokio::test]
async fn a_patched_base_path_mapping_is_stored_and_read_back() {
    let gateway = Gateway::new();
    // Two APIs, each with two stages: a row needs somewhere ELSE to point the mapping,
    // and a stage of the API it points at.
    let (first_api, _) = deployed_api(&gateway, "update-mapping-first", "prod").await;
    create_deployed_stage(&gateway, &first_api, "beta").await;
    let (second_api, _) = deployed_api(&gateway, "update-mapping-second", "prod").await;
    create_deployed_stage(&gateway, &second_api, "live").await;

    let long_path = "p".repeat(MAX_PATH_CHARS);

    // Rows differ along what the rule generalises over: which member the path names, which
    // op the row uses, whether the record started with that member set, how many ops one
    // request carries, whether two ops target one member, the spelling and size of the base
    // path the record is re-addressed by, and whether the row moves the API, the stage, both
    // or neither.
    type Row = (
        String,
        Value,
        Vec<Value>,
        (String, String, Option<String>),
    );
    let rows: Vec<Row> = vec![
        (
            "replace the stage with another stage of the same API".to_string(),
            json!({ "basePath": "v1", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/stage", "beta")],
            ("v1".to_string(), first_api.clone(), Some("beta".to_string())),
        ),
        (
            "replace the API the mapping serves".to_string(),
            json!({ "basePath": "v2", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/restapiId", &second_api)],
            (
                "v2".to_string(),
                second_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "replace the API and the stage in one request".to_string(),
            json!({ "basePath": "v3", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/restapiId", &second_api), replace("/stage", "live")],
            (
                "v3".to_string(),
                second_api.clone(),
                Some("live".to_string()),
            ),
        ),
        (
            "rename the base path".to_string(),
            json!({ "basePath": "v4", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", "v4-renamed")],
            (
                "v4-renamed".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "rename the base path to the documented (none) spelling".to_string(),
            json!({ "basePath": "v5", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", NONE_BASE_PATH)],
            (
                NONE_BASE_PATH.to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "an empty base path value is that same (none) base path".to_string(),
            json!({ "basePath": "v6", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", "")],
            (
                NONE_BASE_PATH.to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "rename the (none) base path to a named one".to_string(),
            json!({ "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", "named-after-none")],
            (
                "named-after-none".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "a base path at the published character bound".to_string(),
            json!({ "basePath": "v7", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", &long_path)],
            (
                long_path.clone(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "a base path of digits, hyphens and underscores".to_string(),
            json!({ "basePath": "v8", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", "Orders-2_beta")],
            (
                "Orders-2_beta".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "remove the stage of a mapping that named one".to_string(),
            json!({ "basePath": "v9", "restApiId": first_api, "stage": "prod" }),
            vec![op("remove", "/stage", None)],
            ("v9".to_string(), first_api.clone(), None),
        ),
        (
            "an empty stage value names no stage either".to_string(),
            json!({ "basePath": "v10", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/stage", "")],
            ("v10".to_string(), first_api.clone(), None),
        ),
        (
            "add a stage the mapping named none of".to_string(),
            json!({ "basePath": "v11", "restApiId": first_api }),
            vec![op("add", "/stage", Some("beta"))],
            (
                "v11".to_string(),
                first_api.clone(),
                Some("beta".to_string()),
            ),
        ),
        (
            "the (none) stage leaves the stage to the caller of the base path".to_string(),
            json!({ "basePath": "v12", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/stage", NONE_BASE_PATH)],
            (
                "v12".to_string(),
                first_api.clone(),
                Some(NONE_BASE_PATH.to_string()),
            ),
        ),
        (
            "add on the base path sets it just as replace does".to_string(),
            json!({ "basePath": "v13", "restApiId": first_api, "stage": "prod" }),
            vec![op("add", "/basePath", Some("v13-added"))],
            (
                "v13-added".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "two ops on one member apply in request order".to_string(),
            json!({ "basePath": "v14", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/stage", "beta"), replace("/stage", "prod")],
            (
                "v14".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "a stage cleared and then set again".to_string(),
            json!({ "basePath": "v15", "restApiId": first_api, "stage": "prod" }),
            vec![op("remove", "/stage", None), replace("/stage", "beta")],
            (
                "v15".to_string(),
                first_api.clone(),
                Some("beta".to_string()),
            ),
        ),
        (
            "the base path renamed twice lands at the last one".to_string(),
            json!({ "basePath": "v16", "restApiId": first_api, "stage": "prod" }),
            vec![
                replace("/basePath", "v16-first"),
                replace("/basePath", "v16-second"),
            ],
            (
                "v16-second".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "every member in one request".to_string(),
            json!({ "basePath": "v17", "restApiId": first_api, "stage": "prod" }),
            vec![
                replace("/basePath", "v17-moved"),
                replace("/restapiId", &second_api),
                replace("/stage", "live"),
            ],
            (
                "v17-moved".to_string(),
                second_api.clone(),
                Some("live".to_string()),
            ),
        ),
        (
            "an empty list reports the record unchanged".to_string(),
            json!({ "basePath": "v18", "restApiId": first_api, "stage": "prod" }),
            vec![],
            (
                "v18".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
        (
            "a base path replaced by the one it already answers to".to_string(),
            json!({ "basePath": "v19", "restApiId": first_api, "stage": "prod" }),
            vec![replace("/basePath", "v19")],
            (
                "v19".to_string(),
                first_api.clone(),
                Some("prod".to_string()),
            ),
        ),
    ];

    for (index, (case, request, operations, expected)) in rows.iter().enumerate() {
        // One domain name per row: the base path is unique per domain name, and two rows
        // may re-address their mapping to the same spelling.
        let domain_name = format!("update-mapping-{index}.example.com");
        // The base path the row's mapping is ADDRESSED by is the one its create stored,
        // with an absent or empty one stored as the `(none)` spelling.
        let addressed_by = match request["basePath"].as_str() {
            Some(base_path) if !base_path.is_empty() => base_path.to_string(),
            _ => NONE_BASE_PATH.to_string(),
        };
        baseline(&gateway, case, &domain_name, request.clone()).await;

        let patched = update(&gateway, &domain_name, &addressed_by, operations.clone()).await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected the modelled 200, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            &mapping_of(&patched.body, case),
            expected,
            "case {case}: expected the update to report the patched record, got {:?}",
            patched.body
        );

        // The patched record is what is STORED: the read at the base path the record now
        // answers to reports exactly the body the update answered with.
        let read = get_mapping(&gateway, &domain_name, &expected.0).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetBasePathMapping({:?}), got {} with {:?}",
            expected.0, read.status, read.body
        );
        assert_eq!(
            read.body, patched.body,
            "case {case}: GetBasePathMapping must read back the patched record, got {:?} \
             after {:?}",
            read.body, patched.body
        );

        // A record re-addressed by another base path is GONE from the one it used to
        // answer to: the domain name carries one mapping, not a copy at each spelling.
        if expected.0 != addressed_by {
            let vacated = get_mapping(&gateway, &domain_name, &addressed_by).await;
            assert_refusal(
                &vacated,
                NOT_FOUND,
                NO_SUCH_BASE_PATH_MAPPING,
                &format!("{case}, at the base path it no longer answers to"),
            );
        }
    }
}

#[tokio::test]
async fn a_patch_this_operation_cannot_apply_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _) = deployed_api(&gateway, "refused-patch-api", "prod").await;
    let domain_name = "refused-patch.example.com";
    baseline(
        &gateway,
        "the mapping every row patches",
        domain_name,
        json!({ "basePath": "v1", "restApiId": api, "stage": "prod" }),
    )
    .await;
    // A second mapping on the same domain name, so a rename has something to collide with.
    let sibling = create_mapping(
        &gateway,
        domain_name,
        json!({ "basePath": "taken", "restApiId": api, "stage": "prod" }),
    )
    .await;
    assert_eq!(
        sibling.status, 201,
        "case the sibling mapping: expected 201, got {} with {:?}",
        sibling.status, sibling.body
    );

    // Rows differ along the KIND of refusal: an op this resource supports on no member, an
    // op missing its value, a path naming no member of the mapping, a path that is no JSON
    // Pointer, one reaching into a single-valued member, a remove and an empty value on a
    // member that always has one, a value one character past the published bound, a
    // reference that resolves to no record, a base path another mapping already answers to,
    // and a list whose LAST op is refused (so the earlier ops must not be kept either).
    /// One refusal row: the case, the ops it sends, the modelled `(status, code)` it must
    /// be refused with, and the message when the row pins one.
    type Refusal = (String, Vec<Value>, (u16, &'static str), Option<&'static str>);
    let cases: Vec<Refusal> = vec![
        (
            "move is not supported".to_string(),
            vec![op("move", "/stage", None)],
            BAD_REQUEST,
            None,
        ),
        (
            "copy is not supported".to_string(),
            vec![json!({ "op": "copy", "path": "/stage", "from": "/stage" })],
            BAD_REQUEST,
            None,
        ),
        (
            "test is not supported".to_string(),
            vec![op("test", "/stage", None)],
            BAD_REQUEST,
            None,
        ),
        (
            "a replace of the base path with no value".to_string(),
            vec![op("replace", "/basePath", None)],
            BAD_REQUEST,
            None,
        ),
        (
            "an add of the stage with no value".to_string(),
            vec![op("add", "/stage", None)],
            BAD_REQUEST,
            None,
        ),
        (
            "an entry with no path".to_string(),
            vec![json!({ "op": "replace" })],
            BAD_REQUEST,
            None,
        ),
        (
            "an entry with no op".to_string(),
            vec![json!({ "path": "/stage" })],
            BAD_REQUEST,
            None,
        ),
        (
            "a path naming no member of the mapping".to_string(),
            vec![replace("/nosuchmember", "x")],
            BAD_REQUEST,
            Some("Invalid patch path /nosuchmember"),
        ),
        (
            "a path that is not a JSON Pointer".to_string(),
            vec![replace("basePath", "x")],
            BAD_REQUEST,
            Some("Invalid patch path basePath"),
        ),
        (
            "an empty path".to_string(),
            vec![replace("", "x")],
            BAD_REQUEST,
            Some("Invalid patch path "),
        ),
        (
            "a path reaching into a single-valued member".to_string(),
            vec![replace("/stage/name", "x")],
            BAD_REQUEST,
            Some("Invalid patch path /stage/name"),
        ),
        (
            "remove of a base path the mapping always answers to".to_string(),
            vec![op("remove", "/basePath", None)],
            BAD_REQUEST,
            None,
        ),
        (
            "remove of the API the mapping always names".to_string(),
            vec![op("remove", "/restapiId", None)],
            BAD_REQUEST,
            None,
        ),
        (
            "an empty API identifier".to_string(),
            vec![replace("/restapiId", "")],
            BAD_REQUEST,
            None,
        ),
        (
            "a base path one character past the published bound".to_string(),
            vec![replace("/basePath", &"p".repeat(MAX_PATH_CHARS + 1))],
            BAD_REQUEST,
            None,
        ),
        (
            "an API identifier that resolves to no stored API".to_string(),
            vec![replace("/restapiId", "nonexist12")],
            BAD_REQUEST,
            Some(INVALID_REST_API_IDENTIFIER),
        ),
        (
            "an API identifier of the wrong shape".to_string(),
            vec![replace("/restapiId", "not an api")],
            BAD_REQUEST,
            Some(INVALID_REST_API_IDENTIFIER),
        ),
        (
            "a stage the named API has none of".to_string(),
            vec![replace("/stage", "absent")],
            BAD_REQUEST,
            Some(NO_SUCH_STAGE),
        ),
        (
            "a stage differing from the API's in case".to_string(),
            vec![replace("/stage", "PROD")],
            BAD_REQUEST,
            Some(NO_SUCH_STAGE),
        ),
        (
            "a base path another mapping of the domain name answers to".to_string(),
            vec![replace("/basePath", "taken")],
            CONFLICT,
            Some(BASE_PATH_MAPPING_ALREADY_EXISTS),
        ),
        (
            "a list whose last op is refused".to_string(),
            vec![replace("/stage", "beta"), replace("/nosuchmember", "x")],
            BAD_REQUEST,
            None,
        ),
        (
            "a list whose last op collides".to_string(),
            vec![replace("/stage", "prod"), replace("/basePath", "taken")],
            CONFLICT,
            None,
        ),
    ];

    for (case, operations, expected, message) in &cases {
        let answer = update(&gateway, domain_name, "v1", operations.clone()).await;
        match message {
            Some(message) => assert_refusal(&answer, *expected, message, case),
            None => assert_failure(&answer, expected.0, expected.1, case),
        }

        // Nothing was applied: both mappings are the ones their creates left behind, each
        // still at its own base path.
        assert_eq!(
            read_mapping(&gateway, case, domain_name, "v1").await,
            ("v1".to_string(), api.clone(), Some("prod".to_string())),
            "case {case}: the refused patch must leave the mapping as it was"
        );
        assert_eq!(
            read_mapping(&gateway, case, domain_name, "taken").await,
            ("taken".to_string(), api.clone(), Some("prod".to_string())),
            "case {case}: the refused patch must leave the sibling mapping as it was"
        );
    }
}

#[tokio::test]
async fn an_identifier_no_record_answers_to_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let (api, _) = deployed_api(&gateway, "absent-mapping-api", "prod").await;
    baseline(
        &gateway,
        "the stored mapping",
        "stored.example.com",
        json!({ "basePath": "v1", "restApiId": api, "stage": "prod" }),
    )
    .await;
    let operations = vec![replace("/stage", "never applied")];

    // A `domainName` no stored record answers to is the domain name's own missing-resource
    // refusal, whatever base path the request names.
    for (case, domain_name, base_path) in [
        ("an unknown domain name", "absent.example.com", "v1"),
        (
            "an unknown domain name, at the (none) base path",
            "absent.example.com",
            NONE_BASE_PATH,
        ),
        (
            "a domain name differing in one label",
            "stored.example.org",
            "v1",
        ),
        (
            "the domain name upper-cased",
            "STORED.EXAMPLE.COM",
            "v1",
        ),
    ] {
        let refused = update(&gateway, domain_name, base_path, operations.clone()).await;
        assert_refusal(&refused, NOT_FOUND, NO_SUCH_DOMAIN_NAME, case);
    }

    // A `basePath` the stored domain name carries no mapping for resolves to nothing,
    // whatever its spelling.
    for (case, base_path) in [
        ("an unmapped base path", "v2"),
        (
            "the (none) base path with nothing mapped at it",
            NONE_BASE_PATH,
        ),
        ("a base path differing in case", "V1"),
        ("a base path that extends a mapped one", "v1x"),
    ] {
        let refused = update(&gateway, "stored.example.com", base_path, operations.clone()).await;
        assert_refusal(&refused, NOT_FOUND, NO_SUCH_BASE_PATH_MAPPING, case);
    }

    // None of those refusals touched the stored record.
    assert_eq!(
        read_mapping(&gateway, "after the refusals", "stored.example.com", "v1").await,
        ("v1".to_string(), api.clone(), Some("prod".to_string())),
        "case after the refusals: expected the mapping the create stored"
    );
}

#[tokio::test]
async fn one_mapping_is_patched_and_its_siblings_are_left_alone() {
    let gateway = Gateway::new();
    let (first_api, _) = deployed_api(&gateway, "siblings-first", "prod").await;
    create_deployed_stage(&gateway, &first_api, "beta").await;
    let (second_api, _) = deployed_api(&gateway, "siblings-second", "live").await;

    let target_domain = "patched.example.com";
    let other_domain = "untouched.example.com";
    baseline(
        &gateway,
        "the mapping to patch",
        target_domain,
        json!({ "basePath": "v1", "restApiId": first_api, "stage": "prod" }),
    )
    .await;
    for (domain_name, base_path) in [(target_domain, "sibling"), (other_domain, "v1")] {
        if domain_name == other_domain {
            created_domain(&gateway, domain_name, json!({})).await;
        }
        let created = create_mapping(
            &gateway,
            domain_name,
            json!({ "basePath": base_path, "restApiId": second_api, "stage": "live" }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case the mapping {base_path:?} of {domain_name}: expected 201, got {} with {:?}",
            created.status, created.body
        );
    }

    // Applying one list repeatedly lands on the same record: the patch SETS members, it
    // does not accumulate — and the second round addresses the record by the base path the
    // first round moved it to.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("UpdateBasePathMapping(round {round})");
        let addressed_by = if round == 0 { "v1" } else { "settled" };
        let patched = update(
            &gateway,
            target_domain,
            addressed_by,
            vec![replace("/basePath", "settled"), replace("/stage", "beta")],
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected the modelled 200, got {} with {:?}",
            patched.status, patched.body
        );
        seen.push(patched.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case UpdateBasePathMapping repeated: the same list must land on the same record, \
         got {:?} then {:?}",
        seen[0], seen[2]
    );
    assert_eq!(
        mapping_of(&seen[2], "the settled mapping"),
        (
            "settled".to_string(),
            first_api.clone(),
            Some("beta".to_string())
        ),
        "case the settled mapping: expected the patched record, got {:?}",
        seen[2]
    );

    // The sibling on the same domain name and the same-named mapping of ANOTHER domain
    // name are the records their own creates left behind.
    for (case, domain_name, base_path) in [
        ("the sibling mapping", target_domain, "sibling"),
        ("another domain name's mapping", other_domain, "v1"),
    ] {
        assert_eq!(
            read_mapping(&gateway, case, domain_name, base_path).await,
            (
                base_path.to_string(),
                second_api.clone(),
                Some("live".to_string())
            ),
            "case {case}: expected the record its own create stored"
        );
    }
}
