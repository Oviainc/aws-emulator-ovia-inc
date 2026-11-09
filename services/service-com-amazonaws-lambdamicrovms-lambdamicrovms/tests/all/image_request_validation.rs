// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `CreateMicrovmImage` and `GetMicrovmImage` REFUSE, and
//! for the configuration the create reports back.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    base_image_not_found_message, default_log_group, image_name_conflict_message,
    image_not_found_message, invalid_arn_format_message, managed_base_image_arn,
    access_denied_message, CODE_ARTIFACT_URI_REQUIREMENT, DEFAULT_CPU_ARCHITECTURE,
    DEFAULT_MINIMUM_MEMORY_IN_MIB, MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, PAST_THE_SPAN, REGION,
};

/// `POST /2025-09-09/microvm-images`.
async fn create_image(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/2025-09-09/microvm-images", Some(body)).await
}

/// `GET /2025-09-09/microvm-images/{imageIdentifier}`.
async fn get_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

/// `DELETE /2025-09-09/microvm-images/{imageIdentifier}`.
async fn delete_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        None,
    )
    .await
}

/// RULE: `CreateMicrovmImage` resolves the code artifact's location before creating
/// anything, so a `codeArtifact$uri` that addresses no S3 object — "The URI of the code
/// artifact in Amazon S3" is `s3://<bucket>/<key>`, both parts non-empty — is refused
/// with HTTP 400 `ValidationException` naming the required form, and nothing is
/// created.
#[tokio::test]
async fn a_code_artifact_uri_that_addresses_no_s3_object_is_refused() {
    let router = emulator();

    // Rows differ in HOW the URI fails to address an object (scheme, bucket, key) and
    // in how deep a well-formed key is, because the rule is about the location the URI
    // addresses rather than about any one spelling of it.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a bucket and a key", "s3://artifacts/app.zip", true),
        ("a key several segments deep", "s3://artifacts/a/b/c.zip", true),
        ("a single-character bucket and key", "s3://b/k", true),
        ("another scheme", "https://artifacts.example/app.zip", false),
        ("no scheme at all", "artifacts/app.zip", false),
        ("a local file", "file:///tmp/app.zip", false),
        ("a bucket and no key", "s3://artifacts", false),
        ("a bucket and an empty key", "s3://artifacts/", false),
        ("no bucket", "s3:///app.zip", false),
        ("the scheme alone", "s3://", false),
    ];

    for (index, (case, uri, accepted)) in rows.into_iter().enumerate() {
        let name = format!("artifact-row-{index}");
        let (status, body) = create_image(
            &router,
            image_body(&name, json!({"codeArtifact": {"uri": uri}})),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: {uri} addresses an S3 object, so the create must \
                 succeed, got {status} {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: {uri} addresses no S3 object, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(CODE_ARTIFACT_URI_REQUIREMENT),
            "case {case:?}: the refusal must name the required form, got {:?}",
            body["message"]
        );
        // Nothing was created: the refusal came BEFORE the resource.
        let (status, after) = get_image(&router, &name).await;
        assert_ne!(
            status, 200,
            "case {case:?}: a refused create must leave no image behind, got {after}"
        );
    }
}

/// RULE: `CreateMicrovmImage` validates the request and resolves its references BEFORE
/// the account-level name-uniqueness check, so the refusal a caller gets names what is
/// actually wrong with its request: an unusable member or an unresolvable reference
/// first, and the taken name only once everything else is good. Every one of these
/// refusals is a 400 `ValidationException` of the member at fault.
#[tokio::test]
async fn input_and_reference_validation_precede_the_name_uniqueness_check() {
    let router = emulator();

    let taken = "already-taken";
    let (status, created) = create_image(&router, image_body(taken, json!({}))).await;
    assert_eq!(status, 201, "the first create must succeed, got {created}");

    // Rows differ in WHICH check the request also fails, and in whether the name it
    // asks for is taken: the taken name must never be what the caller is told about
    // while something else about the request is wrong.
    let unresolvable_base = expected_image_arn("no-such-base-image");
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a taken name and nothing else wrong",
            taken,
            json!({}),
            image_name_conflict_message(taken),
        ),
        (
            "a taken name and an unusable code artifact",
            taken,
            json!({"codeArtifact": {"uri": "not-an-s3-uri"}}),
            CODE_ARTIFACT_URI_REQUIREMENT.to_string(),
        ),
        (
            "a taken name and an unresolvable base image",
            taken,
            json!({"baseImageArn": unresolvable_base.clone()}),
            base_image_not_found_message(&unresolvable_base),
        ),
        (
            "a free name and an unusable code artifact",
            "free-name-bad-artifact",
            json!({"codeArtifact": {"uri": "not-an-s3-uri"}}),
            CODE_ARTIFACT_URI_REQUIREMENT.to_string(),
        ),
        (
            "a free name and an unresolvable base image",
            "free-name-bad-base",
            json!({"baseImageArn": unresolvable_base.clone()}),
            base_image_not_found_message(&unresolvable_base),
        ),
    ];

    for (case, name, extra, expected_message) in rows {
        let (status, body) = create_image(&router, image_body(name, extra)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected {expected_message:?}, got {:?}",
            body["message"]
        );
    }
}

/// RULE: `CreateMicrovmImage` resolves `baseImageArn` before creating anything and
/// refuses with HTTP 400 `ValidationException` any base-image reference it can
/// determine names nothing: an AWS-MANAGED ARN (account field `aws`) naming no image in
/// the catalogue this service publishes, or an ARN in THIS account's own
/// `microvm-image` namespace holding no such image. A reference into neither namespace
/// — another account's ARN, another resource type, something that is no ARN — names
/// nothing this emulator holds and is taken as given.
#[tokio::test]
async fn a_base_image_reference_that_resolves_to_nothing_is_refused() {
    let router = emulator();

    let base = "a-local-base-image";
    let (status, created) = create_image(&router, image_body(base, json!({}))).await;
    assert_eq!(status, 201, "create the base image: got {created}");
    let base_arn = expected_image_arn(base);

    // Every id the published catalogue carries is resolvable — the catalogue, not this
    // test, is the source of the set — and a managed ARN outside it is not.
    let mut rows: Vec<(&str, String, bool)> = MANAGED_BASE_IMAGE_IDS
        .iter()
        .map(|image_id| {
            (
                "a published managed base image",
                managed_base_image_arn(REGION, image_id),
                true,
            )
        })
        .collect();
    // Rows span the namespaces a `baseImageArn` can name: the AWS-managed catalogue and
    // this account's own images (both resolvable, so both refusable) and the ones this
    // emulator does not hold.
    rows.extend([
        (
            "a managed ARN the catalogue does not publish",
            managed_base_image_arn(REGION, "no-such-managed-image-0"),
            false,
        ),
        (
            // The catalogue publishes the same base-image ids in every region, so the
            // reference resolves by the id its ARN names.
            "a managed ARN in another region",
            managed_base_image_arn("eu-central-1", MANAGED_BASE_IMAGE_IDS[0]),
            true,
        ),
        ("this account's own image", base_arn.clone(), true),
        (
            "this account's namespace, no such image",
            expected_image_arn("never-created"),
            false,
        ),
        (
            "another account's image",
            format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:theirs"),
            true,
        ),
        (
            "another resource type in this account",
            format!("arn:aws:lambda:{REGION}:000000000000:managed-microvm-image:al2023"),
            true,
        ),
        (
            "a reference that is no ARN at all",
            "managed-base-image".to_string(),
            true,
        ),
    ]);

    for (index, (case, base_image_arn, accepted)) in rows.into_iter().enumerate() {
        let name = format!("base-row-{index}");
        let (status, body) = create_image(
            &router,
            image_body(&name, json!({"baseImageArn": base_image_arn.clone()})),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: {base_image_arn} resolves (or names a namespace this \
                 emulator does not hold), got {status} {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: {base_image_arn} resolves to no base image, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(base_image_not_found_message(&base_image_arn).as_str()),
            "case {case:?}: the refusal is built from the ARN the request carried, got {:?}",
            body["message"]
        );
    }

    // The rule reads the STATE, not the request: once the base image is gone, the same
    // reference that resolved a moment ago is refused.
    let (status, deleted) = delete_image(&router, base).await;
    assert_eq!(status, 200, "delete the base image: got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, body) = create_image(
        &router,
        image_body("after-the-base-went", json!({"baseImageArn": base_arn.clone()})),
    )
    .await;
    assert_eq!(
        status, 400,
        "a base image this account no longer holds must be refused, got {status} {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(base_image_not_found_message(&base_arn).as_str()),
        "expected the reference refusal, got {:?}",
        body["message"]
    );
}

/// RULE: `GetMicrovmImage` resolves its `imageIdentifier` — "The ARN or ID of the
/// MicroVmImage" — as an id this account holds and, failing that, as an ARN: a value
/// that is neither addresses nothing this API can name and is refused 400
/// `ValidationException`, a well-formed identifier this account holds no record of (now
/// or before) is the 403 `AccessDeniedException` the authorization verdict answers, and
/// only this account's OWN record being gone is the 404 `ResourceNotFoundException` a
/// poll may legitimately retry (the authorization boundary
/// `image_read_authorization` owns).
#[tokio::test]
async fn an_image_lookup_separates_a_malformed_identifier_from_an_absent_one() {
    let router = emulator();

    let held = "held-image";
    let (status, created) = create_image(&router, image_body(held, json!({}))).await;
    assert_eq!(status, 201, "create the image: got {created}");
    let held_arn = expected_image_arn(held);

    // An image this account creates and then deletes: its ARN stays an identifier the
    // account holds a record of, so the lookup reports the record's ABSENCE.
    let released = "released-image";
    let (status, created) = create_image(&router, image_body(released, json!({}))).await;
    assert_eq!(status, 201, "create the released image: got {created}");
    let released_arn = expected_image_arn(released);
    let (status, deleted) = delete_image(&router, &released_arn).await;
    assert_eq!(status, 200, "delete the released image: got {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    // Rows span the four outcomes, and within the malformed refusal every way an ARN
    // can be malformed that `service_common::arn` names: a missing field, an empty
    // partition, an empty resource, and no `arn:` prefix at all.
    let rows: Vec<(&str, String, u16)> = vec![
        ("an id this account holds", held.to_string(), 200),
        ("the ARN this account minted", held_arn.clone(), 200),
        (
            "the ARN of a record this account deleted",
            released_arn.clone(),
            404,
        ),
        (
            "a well-formed ARN naming no image",
            expected_image_arn("never-created"),
            404,
        ),
        (
            "a well-formed ARN of another partition",
            format!("arn:aws-us-gov:lambda:{REGION}:000000000000:microvm-image:{held}"),
            403,
        ),
        (
            "a well-formed ARN of another vendor",
            format!("arn:aws:s3:{REGION}:000000000000:microvm-image:{held}"),
            403,
        ),
        ("a bare id this account holds none of", "no-such-id".to_string(), 400),
        ("too few fields", "arn:aws:lambda".to_string(), 400),
        (
            "five fields, no resource",
            format!("arn:aws:lambda:{REGION}:000000000000"),
            400,
        ),
        (
            "an empty partition",
            format!("arn::lambda:{REGION}:000000000000:microvm-image:{held}"),
            400,
        ),
        (
            "an empty resource",
            format!("arn:aws:lambda:{REGION}:000000000000:"),
            400,
        ),
        ("nothing ARN-shaped at all", "not.an.arn".to_string(), 400),
    ];

    for (case, identifier, expected_status) in rows {
        let (status, body) = get_image(&router, &identifier).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {identifier} expected {expected_status}, got {status} {body}"
        );
        match expected_status {
            200 => assert_eq!(
                body["imageArn"].as_str(),
                Some(held_arn.as_str()),
                "case {case:?}: expected the stored image, got {body}"
            ),
            403 => {
                assert_eq!(
                    body["__type"].as_str(),
                    Some("AccessDeniedException"),
                    "case {case:?}: expected AccessDeniedException, got {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(access_denied_message().as_str()),
                    "case {case:?}: expected the modelled denial sentence, got {:?}",
                    body["message"]
                );
            }
            404 => {
                assert_eq!(
                    body["__type"].as_str(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(image_not_found_message(&identifier).as_str()),
                    "case {case:?}: expected the lookup-miss message, got {:?}",
                    body["message"]
                );
            }
            _ => {
                assert_eq!(
                    body["__type"].as_str(),
                    Some("ValidationException"),
                    "case {case:?}: expected ValidationException, got {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(invalid_arn_format_message(&identifier).as_str()),
                    "case {case:?}: expected the malformed-identifier message, got {:?}",
                    body["message"]
                );
            }
        }
    }
}

/// RULE: the `CreateMicrovmImage` response is the configuration the service ACCEPTED —
/// every member the request named comes back unchanged, and every member it left to the
/// service (`cpuConfigurations`, `resources`, `logging`) comes back carrying the value
/// the service supplied — and it is the PERSISTED record, so a `clientToken` replay
/// reports the same aggregate.
#[tokio::test]
async fn the_create_response_reports_the_configuration_the_service_accepted() {
    let router = emulator();

    // The values the service supplies for a request that names none of the three.
    let default_cpu = json!([{"architecture": DEFAULT_CPU_ARCHITECTURE}]);
    let default_resources = json!([{"minimumMemoryInMiB": DEFAULT_MINIMUM_MEMORY_IN_MIB}]);

    // Rows differ in which of the three the request names, because each is resolved
    // and stored independently of the others, and in which arm the `logging` union
    // takes.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("none of the three named", "supplied-all", json!({})),
        (
            "cpuConfigurations named",
            "supplied-cpu",
            json!({"cpuConfigurations": [{"architecture": "ARM_64"}]}),
        ),
        (
            "resources named",
            "supplied-resources",
            json!({"resources": [{"minimumMemoryInMiB": 512}]}),
        ),
        (
            "logging named, cloudWatch",
            "supplied-logging-cw",
            json!({"logging": {"cloudWatch": {"logGroup": "/aws/microvm/custom"}}}),
        ),
        (
            "logging named, disabled",
            "supplied-logging-off",
            json!({"logging": {"disabled": {}}}),
        ),
        (
            "all three named",
            "supplied-none",
            json!({
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 10240}],
                "logging": {"cloudWatch": {"logGroup": "/aws/microvm/all"}},
            }),
        ),
    ];

    for (case, name, extra) in rows {
        let request = image_body(name, extra.clone());
        let client_token = format!("token-for-{name}");
        let mut with_token = request.clone();
        with_token
            .as_object_mut()
            .expect("the body is an object")
            .insert("clientToken".to_string(), json!(client_token));

        let (status, created) = create_image(&router, with_token.clone()).await;
        assert_eq!(status, 201, "case {case:?}: got {status} {created}");

        // Each member the service may supply: what the request named, else the
        // service's own value.
        let expected_cpu = extra
            .get("cpuConfigurations")
            .cloned()
            .unwrap_or_else(|| default_cpu.clone());
        let expected_resources = extra
            .get("resources")
            .cloned()
            .unwrap_or_else(|| default_resources.clone());
        let expected_logging = extra.get("logging").cloned().unwrap_or_else(|| {
            json!({"cloudWatch": {"logGroup": default_log_group(name)}})
        });

        for (member, expected) in [
            ("cpuConfigurations", expected_cpu),
            ("resources", expected_resources),
            ("logging", expected_logging),
        ] {
            assert_eq!(
                created[member], expected,
                "case {case:?}: member {member} expected {expected}, got {:?}",
                created[member]
            );
        }

        // The response is the stored record: the idempotent replay of the same
        // `clientToken` reports the same aggregate rather than recomputing one.
        let (status, replayed) = create_image(&router, with_token).await;
        assert_eq!(
            status, 201,
            "case {case:?}: the replay must succeed, got {status} {replayed}"
        );
        assert_eq!(
            replayed, created,
            "case {case:?}: the replay must report the persisted aggregate"
        );
    }
}

/// RULE: an image's `tags` are projected as a MAP even when the image carries none —
/// `GetMicrovmImage` reports the whole stored record, and an image with no tags has an
/// empty tag set rather than an absent one.
#[tokio::test]
async fn an_image_reports_its_tags_as_a_map_even_when_it_carries_none() {
    let router = emulator();

    // Rows differ in how many tags the image carries, which is what the projection
    // generalises over.
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        ("no tags member at all", "untagged", json!({}), json!({})),
        (
            "an empty tags member",
            "empty-tagged",
            json!({"tags": {}}),
            json!({}),
        ),
        (
            "one tag",
            "one-tagged",
            json!({"tags": {"team": "platform"}}),
            json!({"team": "platform"}),
        ),
        (
            "several tags",
            "many-tagged",
            json!({"tags": {"team": "platform", "cost-centre": "1234"}}),
            json!({"team": "platform", "cost-centre": "1234"}),
        ),
    ];

    for (case, name, extra, expected) in rows {
        let (status, created) = create_image(&router, image_body(name, extra)).await;
        assert_eq!(status, 201, "case {case:?}: create got {status} {created}");

        let (status, image) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case:?}: get got {status} {image}");
        assert_eq!(
            image["tags"], expected,
            "case {case:?}: expected the stored tag set, got {:?}",
            image["tags"]
        );
    }
}

/// RULE: a `@pattern` violation is stated the way AWS states it — `1 validation error
/// detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must
/// satisfy regular expression pattern: <pattern>` where `<pattern>` is the member's own
/// constraint with the anchoring `^` and `$` of a whole-value pattern removed — with
/// HTTP 400 and the `ValidationException` code.
#[tokio::test]
async fn a_pattern_refusal_states_the_constraint_without_its_anchors() {
    let router = emulator();

    // Rows span the members of this request that carry a `@pattern`, with each
    // member's pattern as `smithy_query(query="operation",
    // operation="CreateMicrovmImage")` reports it and the expression the clause must
    // state. The unanchored rendering is the whole point: a caller that re-compiles
    // the echoed expression gets the constraint, not a doubly anchored one.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "the image name",
            "name",
            json!({"name": "not a valid name!"}),
            "not a valid name!",
            r"[a-zA-Z0-9-_]+",
        ),
        (
            "the base image ARN",
            "baseImageArn",
            json!({"baseImageArn": "has a space"}),
            "has a space",
            r"[^\s]+",
        ),
        (
            "the build role ARN",
            "buildRoleArn",
            json!({"buildRoleArn": "arn:aws:iam::not-twelve-digits:role/build"}),
            "arn:aws:iam::not-twelve-digits:role/build",
            r"arn:aws[a-z\-]*:iam::[0-9]{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+",
        ),
    ];
    // (`codeArtifact$uri` carries the same `^[^\s]+$` pattern, but it is a member of a
    // UNION and the framework's boundary validator does not walk into a union's
    // members, so no refusal to state is produced there; that gap is the framework's,
    // not this service's wording.)

    for (index, (case, path, extra, value, pattern)) in rows.into_iter().enumerate() {
        let name = format!("pattern-row-{index}");
        let (status, body) = create_image(&router, image_body(&name, extra)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a pattern violation is refused, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        let expected = format!(
            "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {pattern}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            body["message"]
        );
        let stated = body["message"]
            .as_str()
            .unwrap_or_default()
            .rsplit("regular expression pattern: ")
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stated.starts_with('^') && !stated.ends_with('$'),
            "case {case:?}: the stated pattern must carry no anchors, got {stated:?}"
        );
    }
}
