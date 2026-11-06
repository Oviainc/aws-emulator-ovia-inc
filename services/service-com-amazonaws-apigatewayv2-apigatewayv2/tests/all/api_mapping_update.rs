// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `UpdateApiMapping`.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees, not a handler called directly.
//!
//! The rules asserted here, each over a TABLE rather than over one request:
//!
//! * **`UpdateApiMapping` is a PATCH of the stored mapping**
//!   (`PATCH /v2/domainnames/{DomainName}/apimappings/{ApiMappingId}`): the record left
//!   behind is every member the request carries over the members it omits — `ApiId` is
//!   required so a patch always names the api, while an omitted `ApiMappingKey` or `Stage`
//!   keeps the value the mapping already routes on — under the identifier the mapping was
//!   minted with. What the response reports is what a later `GetApiMapping` reports.
//! * **A stored mapping always routes to a stage of an api the account holds**, so the
//!   PAIR the patch would leave behind is weighed against the store: an api no account
//!   resource carries is 404 `NotFoundException`, and a stage that names none of that
//!   api's stages — including the stage the mapping already routed on, when the patch
//!   moves it to an api that does not hold it — is 400 `BadRequestException`.
//! * **A mapping is patched only through the pair its path addresses**: a domain name the
//!   account does not hold and a mapping id the domain name does not hold are both 404,
//!   and the refusal for the mapping names the `ApiMappingId` the request addressed it by,
//!   so the rows that differ in that id differ in the message too.
//! * **An `ApiMappingKey` is a base path**: written without a leading separator, with
//!   exactly one separator between segments and each segment drawn from letters, digits
//!   and the documented punctuation; anything else is 400 `BadRequestException`.
//! * **One base path of one domain name resolves to exactly one api stage**: a patch onto
//!   a base path ANOTHER mapping of the same domain name occupies is 409
//!   `ConflictException`, while a patch that leaves the mapping on the base path it
//!   already occupies conflicts with nothing.
//! * **Every refusal persists nothing**: the addressed mapping, and the collection it
//!   belongs to, are exactly what they were before the refused call.
//!
//! The rows vary what the rules generalise over: which optional members the patch carries,
//! the form of the base path before and after (absent, empty, one segment, several
//! segments, the documented punctuation), whether the api and the stage move, whether the
//! patch repeats what is already stored, which resource of the request resolves to
//! nothing, and where a malformed key is malformed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    api_mapping_not_found_message, invalid_api_identifier_message, API_MAPPING_KEY_CONFLICT,
    API_MAPPING_KEY_ILLEGAL_CHARACTER, API_MAPPING_KEY_MALFORMED_PATH,
    API_MAPPING_KEY_PUNCTUATION, INVALID_DOMAIN_NAME_IDENTIFIER, INVALID_STAGE_IDENTIFIER,
};

use crate::api_lifecycle::{as_str, carries_nothing, error_code};
use crate::api_mapping_lifecycle::{
    call, create_api_mapping, get_api_mapping, seed_api, seed_domain_name, seeded, DOMAIN,
    OTHER_STAGE, STAGE,
};

/// Patch one api mapping, addressing it the way the model binds the operation.
pub(crate) async fn update_api_mapping(
    emulator: &MultiServiceRouter,
    domain: &str,
    api_mapping_id: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PATCH",
        &format!("/v2/domainnames/{domain}/apimappings/{api_mapping_id}"),
        Some(request.clone()),
    )
    .await
}

/// Build a create/patch body: `apiId` is always carried (the operation requires it), and
/// each optional member is carried only when the row names one. `Some("")` is the empty
/// base path — the root of the domain — written out, which is NOT the same request as one
/// that carries no key at all.
pub(crate) fn body(api_id: &str, key: Option<&str>, stage: Option<&str>) -> Value {
    let mut request = json!({ "apiId": api_id });
    if let Some(key) = key {
        request["apiMappingKey"] = json!(key);
    }
    if let Some(stage) = stage {
        request["stage"] = json!(stage);
    }
    request
}

/// Assert one mapping response carries exactly the record the rule says it should.
fn assert_record(case: &str, body: &Value, api_id: &str, key: Option<&str>, stage: &str) {
    assert_eq!(
        as_str(body, "apiId"),
        api_id,
        "case {case:?}: expected apiId {api_id:?}, got {body}"
    );
    assert_eq!(
        as_str(body, "stage"),
        stage,
        "case {case:?}: expected stage {stage:?}, got {body}"
    );
    match key {
        Some(key) => assert_eq!(
            as_str(body, "apiMappingKey"),
            key,
            "case {case:?}: expected apiMappingKey {key:?}, got {body}"
        ),
        None => assert!(
            carries_nothing(body, "apiMappingKey"),
            "case {case:?}: expected no apiMappingKey, got {body}"
        ),
    }
}

/// One row of the patch table: the mapping to create, then the patch applied to it.
///
/// `create_key`/`patch_key` are `None` when the request carries no `apiMappingKey` at all;
/// `patch_stage` is `None` when the patch carries no `Stage`. `patch_moves_api` points the
/// patch at the second seeded api instead of the one the mapping was created against.
struct PatchRow {
    case: &'static str,
    create_key: Option<&'static str>,
    create_stage: &'static str,
    patch_key: Option<&'static str>,
    patch_stage: Option<&'static str>,
    patch_moves_api: bool,
}

/// The rows span: which optional members a patch carries (neither, each, both), the form of
/// the base path before and after (absent, empty, one segment, several segments, the
/// documented punctuation class), whether the api moves, and a patch that only repeats what
/// is already stored.
fn patch_rows() -> Vec<PatchRow> {
    vec![
        PatchRow {
            case: "only the required apiId: every optional member keeps its stored value",
            create_key: Some("orders"),
            create_stage: STAGE,
            patch_key: None,
            patch_stage: None,
            patch_moves_api: false,
        },
        PatchRow {
            case: "the stage alone moves",
            create_key: Some("orders"),
            create_stage: STAGE,
            patch_key: None,
            patch_stage: Some(OTHER_STAGE),
            patch_moves_api: false,
        },
        PatchRow {
            case: "the base path alone moves",
            create_key: Some("orders"),
            create_stage: OTHER_STAGE,
            patch_key: Some("invoices"),
            patch_stage: None,
            patch_moves_api: false,
        },
        PatchRow {
            case: "the base path and the stage move at once",
            create_key: Some("orders"),
            create_stage: OTHER_STAGE,
            patch_key: Some("orders/v2/items"),
            patch_stage: Some(STAGE),
            patch_moves_api: false,
        },
        PatchRow {
            case: "a mapping created at the root of the domain takes a base path",
            create_key: None,
            create_stage: STAGE,
            patch_key: Some("orders"),
            patch_stage: None,
            patch_moves_api: false,
        },
        PatchRow {
            case: "a mapping on a base path moves to the root of the domain",
            create_key: Some("orders"),
            create_stage: STAGE,
            patch_key: Some(""),
            patch_stage: None,
            patch_moves_api: false,
        },
        PatchRow {
            // The punctuation class comes from the crate constant, not from a copy written
            // here; this row carries every character of it at once.
            case: "the base path moves onto every documented punctuation character",
            create_key: Some("v1.2"),
            create_stage: STAGE,
            patch_key: Some("a$-_.+!*'(),z"),
            patch_stage: None,
            patch_moves_api: false,
        },
        PatchRow {
            case: "the mapping moves to another api that holds the stage it routes on",
            create_key: Some("orders"),
            create_stage: STAGE,
            patch_key: None,
            patch_stage: None,
            patch_moves_api: true,
        },
        PatchRow {
            case: "another api, another base path and another stage at once",
            create_key: Some("orders"),
            create_stage: OTHER_STAGE,
            patch_key: Some("orders/v1"),
            patch_stage: Some(STAGE),
            patch_moves_api: true,
        },
        PatchRow {
            case: "the patch repeats every value already stored",
            create_key: Some("orders"),
            create_stage: STAGE,
            patch_key: Some("orders"),
            patch_stage: Some(STAGE),
            patch_moves_api: false,
        },
    ]
}

#[tokio::test]
async fn update_api_mapping_patches_the_stored_record_get_api_mapping_reports_back() {
    let (emulator, api_id) = seeded().await;
    // The api a row may move its mapping to. It holds only `STAGE`, so a row that moves
    // the api is also the case over "the stage the mapping keeps must exist on the api it
    // moves to".
    let other_api = seed_api(&emulator, "second-api", &[STAGE]).await;
    // Every character the punctuation class names must be a legal base-path character, so
    // the row that patches onto all of them is a row about the documented class rather than
    // about one string.
    for character in API_MAPPING_KEY_PUNCTUATION.chars() {
        assert!(
            "a$-_.+!*'(),z".contains(character),
            "the punctuation row must carry every documented character, missing \
             {character:?}"
        );
    }

    for (index, row) in patch_rows().into_iter().enumerate() {
        let case = row.case;
        // A domain name of its own per row, so what one row stores is never what another
        // row's base path collides with.
        let domain = format!("row{index}.example.com");
        seed_domain_name(&emulator, &domain).await;

        let (status, created) = create_api_mapping(
            &emulator,
            &domain,
            &body(&api_id, row.create_key, Some(row.create_stage)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateApiMapping, got {status} {created}"
        );
        let api_mapping_id = as_str(&created, "apiMappingId");

        // What the rule says the patch leaves behind: the members the request carries over
        // the members it omits.
        let patch_api = if row.patch_moves_api {
            other_api.clone()
        } else {
            api_id.clone()
        };
        let expected_key = if row.patch_key.is_some() {
            row.patch_key
        } else {
            row.create_key
        };
        let expected_stage = row.patch_stage.unwrap_or(row.create_stage);

        let (status, patched) = update_api_mapping(
            &emulator,
            &domain,
            &api_mapping_id,
            &body(&patch_api, row.patch_key, row.patch_stage),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateApiMapping, got {status} {patched}"
        );
        assert_record(case, &patched, &patch_api, expected_key, expected_stage);
        // A patch moves no mapping to a new identifier.
        assert_eq!(
            as_str(&patched, "apiMappingId"),
            api_mapping_id,
            "case {case:?}: expected the minted apiMappingId {api_mapping_id:?} to be kept, \
             got {patched}"
        );

        // The round trip: the patched record is what a later reader reports.
        let (status, read) = get_api_mapping(&emulator, &domain, &api_mapping_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetApiMapping, got {status} {read}"
        );
        assert_eq!(
            read, patched,
            "case {case:?}: expected GetApiMapping to report the patched record, got {read}"
        );
    }
}

#[tokio::test]
async fn a_patch_naming_a_resource_the_account_does_not_hold_is_refused_and_writes_nothing() {
    let (emulator, api_id) = seeded().await;
    let (status, created) = create_api_mapping(
        &emulator,
        DOMAIN,
        &body(&api_id, Some("orders"), Some(STAGE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the mapping: expected 200 from CreateApiMapping, got {status} {created}"
    );
    let api_mapping_id = as_str(&created, "apiMappingId");

    // The rows vary which resource of the request resolves to nothing, and the form the
    // absent identifier takes.
    let absent_domains = [
        ("a plain absent domain name", "absent.example.com"),
        ("a sibling of the held domain name", "other.example.com"),
        ("the held domain name in a different case", "Mapped.Example.Com"),
    ];
    for (case, domain) in absent_domains {
        let (status, refused) = update_api_mapping(
            &emulator,
            domain,
            &api_mapping_id,
            &body(&api_id, None, Some(OTHER_STAGE)),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateApiMapping on {domain:?}, got {status} \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            INVALID_DOMAIN_NAME_IDENTIFIER,
            "case {case:?}: expected the domain-name refusal message, got {refused}"
        );
    }

    let absent_mappings = [
        ("an api mapping id that was never minted", "nosuchmapping"),
        ("the minted id with every character changed", "0000000000"),
    ];
    for (case, mapping_id) in absent_mappings {
        let (status, refused) = update_api_mapping(
            &emulator,
            DOMAIN,
            mapping_id,
            &body(&api_id, None, Some(OTHER_STAGE)),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateApiMapping for mapping {mapping_id:?}, \
             got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            api_mapping_not_found_message(mapping_id),
            "case {case:?}: expected the api-mapping refusal message, got {refused}"
        );
    }

    let absent_apis = [
        ("an api id that was never minted", "nosuchapi0"),
        ("the empty api id", ""),
        ("the held api id with every character changed", "0000000000"),
    ];
    for (case, absent_api) in absent_apis {
        let (status, refused) = update_api_mapping(
            &emulator,
            DOMAIN,
            &api_mapping_id,
            &body(absent_api, None, None),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateApiMapping for api {absent_api:?}, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            invalid_api_identifier_message(absent_api),
            "case {case:?}: expected the account-qualified api refusal message, got {refused}"
        );
    }

    // Every refusal above left the mapping exactly as the create stored it.
    let (status, read) = get_api_mapping(&emulator, DOMAIN, &api_mapping_id).await;
    assert_eq!(
        status, 200,
        "expected the refused patches to have left the mapping readable, got {status} {read}"
    );
    assert_eq!(
        read, created,
        "expected the refused patches to have written nothing, got {read}"
    );
}

#[tokio::test]
async fn a_patch_whose_stage_names_no_stage_of_the_mapped_api_is_refused() {
    let (emulator, api_id) = seeded().await;
    // An api of its own, holding a stage neither of the others does, so a stage name that
    // exists on ANOTHER api is still no stage reference for the api a patch names.
    let neighbour = seed_api(&emulator, "neighbour-api", &["staging"]).await;

    let (status, created) = create_api_mapping(
        &emulator,
        DOMAIN,
        &body(&api_id, Some("orders"), Some(OTHER_STAGE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the mapping: expected 200 from CreateApiMapping, got {status} {created}"
    );
    let api_mapping_id = as_str(&created, "apiMappingId");

    // The rows vary the form of the unusable stage reference, and the last one carries no
    // `Stage` at all: moving the mapping to an api that does not hold the stage it already
    // routes on leaves the same unusable pair behind.
    let rows = [
        ("the empty stage", &api_id, Some("")),
        ("a stage name nothing created", &api_id, Some("nosuchstage")),
        ("a stage of another api", &api_id, Some("staging")),
        ("the held stage in a different case", &api_id, Some("PROD")),
        (
            "the default stage spelled without its sigil",
            &api_id,
            Some("default"),
        ),
        (
            "another api, which does not hold the stage the mapping keeps",
            &neighbour,
            None,
        ),
    ];
    for (case, api, stage) in rows {
        let (status, refused) =
            update_api_mapping(&emulator, DOMAIN, &api_mapping_id, &body(api, None, stage)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateApiMapping with stage {stage:?}, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            INVALID_STAGE_IDENTIFIER,
            "case {case:?}: expected the stage-reference refusal message, got {refused}"
        );
        let (status, read) = get_api_mapping(&emulator, DOMAIN, &api_mapping_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the mapping to survive the refusal, got {status} {read}"
        );
        assert_eq!(
            read, created,
            "case {case:?}: expected the refusal to have written nothing, got {read}"
        );
    }

    // The positive control for the same table: the neighbour's own stage IS a reference for
    // the neighbour, so the pair the patch leaves behind resolves.
    let (status, patched) = update_api_mapping(
        &emulator,
        DOMAIN,
        &api_mapping_id,
        &body(&neighbour, None, Some("staging")),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the neighbour's own stage to be a usable reference, got {status} {patched}"
    );
    assert_record(
        "the neighbour's own stage",
        &patched,
        &neighbour,
        Some("orders"),
        "staging",
    );
}

#[tokio::test]
async fn a_patch_whose_api_mapping_key_is_not_a_base_path_is_refused() {
    let (emulator, api_id) = seeded().await;
    let (status, created) = create_api_mapping(
        &emulator,
        DOMAIN,
        &body(&api_id, Some("orders"), Some(STAGE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the mapping: expected 200 from CreateApiMapping, got {status} {created}"
    );
    let api_mapping_id = as_str(&created, "apiMappingId");

    // A base path is written without a leading separator and with exactly one separator
    // between segments; the rows vary where the offending separator sits, and then which
    // character outside the documented class the key carries.
    let rows = [
        ("a leading separator", "/orders", API_MAPPING_KEY_MALFORMED_PATH),
        ("only a separator", "/", API_MAPPING_KEY_MALFORMED_PATH),
        (
            "consecutive separators",
            "orders//v1",
            API_MAPPING_KEY_MALFORMED_PATH,
        ),
        (
            "consecutive separators at the end",
            "orders//",
            API_MAPPING_KEY_MALFORMED_PATH,
        ),
        ("a space", "or ders", API_MAPPING_KEY_ILLEGAL_CHARACTER),
        ("a percent", "orders%20", API_MAPPING_KEY_ILLEGAL_CHARACTER),
        ("a colon", "orders:v1", API_MAPPING_KEY_ILLEGAL_CHARACTER),
        ("a tilde", "~orders", API_MAPPING_KEY_ILLEGAL_CHARACTER),
        (
            "a character outside ASCII",
            "ordérs",
            API_MAPPING_KEY_ILLEGAL_CHARACTER,
        ),
    ];
    for (case, key, message) in rows {
        let (status, refused) = update_api_mapping(
            &emulator,
            DOMAIN,
            &api_mapping_id,
            &body(&api_id, Some(key), None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateApiMapping with key {key:?}, got {status} \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            message,
            "case {case:?}: expected the base-path refusal message, got {refused}"
        );
        let (status, read) = get_api_mapping(&emulator, DOMAIN, &api_mapping_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the mapping to survive the refusal, got {status} {read}"
        );
        assert_eq!(
            read, created,
            "case {case:?}: expected the refusal to have written nothing, got {read}"
        );
    }
}

#[tokio::test]
async fn a_patch_onto_a_base_path_another_mapping_of_the_domain_name_occupies_conflicts() {
    let (emulator, api_id) = seeded().await;

    // Two mappings of one domain name: the base path the second holds is the one the first
    // is patched onto.
    let (status, root) =
        create_api_mapping(&emulator, DOMAIN, &body(&api_id, None, Some(STAGE))).await;
    assert_eq!(
        status, 200,
        "seeding the root mapping: expected 200, got {status} {root}"
    );
    let root_id = as_str(&root, "apiMappingId");
    let (status, taken) = create_api_mapping(
        &emulator,
        DOMAIN,
        &body(&api_id, Some("orders"), Some(STAGE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the occupied base path: expected 200, got {status} {taken}"
    );
    let taken_id = as_str(&taken, "apiMappingId");

    // The rows vary which mapping is patched onto which occupied base path, and — for the
    // root — which of the two spellings of the SAME base path the patch writes.
    let rows = [
        ("onto a base path one segment long", &root_id, Some("orders")),
        ("onto the root of the domain", &taken_id, Some("")),
    ];
    for (case, mapping_id, key) in rows {
        let (status, refused) =
            update_api_mapping(&emulator, DOMAIN, mapping_id, &body(&api_id, key, None)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 from UpdateApiMapping with key {key:?}, got {status} \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            API_MAPPING_KEY_CONFLICT,
            "case {case:?}: expected the base-path conflict message, got {refused}"
        );
    }

    // Both mappings survived every refusal, each on the base path it already held.
    for (label, expected) in [("the root mapping", &root), ("the occupied path", &taken)] {
        let (status, read) =
            get_api_mapping(&emulator, DOMAIN, &as_str(expected, "apiMappingId")).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the mapping to survive, got {status} {read}"
        );
        assert_eq!(
            &read, expected,
            "case {label:?}: expected the refusals to have written nothing, got {read}"
        );
    }

    // A mapping is never a second claim on its own base path: a patch that leaves it where
    // it already is — written either way round for the root — is accepted. The last row
    // carries no key at all, and the row before it stored the empty key verbatim, so what
    // it keeps is that empty base path.
    let rows = [
        (
            "the base path it already holds",
            &taken_id,
            Some("orders"),
            Some("orders"),
        ),
        (
            "the root it already holds, written out",
            &root_id,
            Some(""),
            Some(""),
        ),
        (
            "no key at all, so the root it already holds",
            &root_id,
            None,
            Some(""),
        ),
    ];
    for (case, mapping_id, key, expected_key) in rows {
        let (status, patched) = update_api_mapping(
            &emulator,
            DOMAIN,
            mapping_id,
            &body(&api_id, key, Some(OTHER_STAGE)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateApiMapping, got {status} {patched}"
        );
        assert_record(case, &patched, &api_id, expected_key, OTHER_STAGE);
    }

    // The base path identifies the mapping within ITS domain name, so the base path another
    // domain name occupies is free for this one.
    seed_domain_name(&emulator, "second.example.com").await;
    let (status, elsewhere) = create_api_mapping(
        &emulator,
        "second.example.com",
        &body(&api_id, Some("invoices"), Some(STAGE)),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the other domain name's mapping: expected 200, got {status} {elsewhere}"
    );
    let (status, patched) = update_api_mapping(
        &emulator,
        DOMAIN,
        &taken_id,
        &body(&api_id, Some("invoices"), None),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a base path occupied on another domain name to be free here, got {status} \
         {patched}"
    );
    assert_record(
        "a base path occupied on another domain name",
        &patched,
        &api_id,
        Some("invoices"),
        OTHER_STAGE,
    );
}
