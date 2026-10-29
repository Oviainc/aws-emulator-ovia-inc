// SPDX-License-Identifier: CC0-1.0

//! `DeleteConnectionFunction`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are:
//!
//! * a delete REMOVES the ONE record every read of that connection function answered from —
//!   both of its stages, their code, configurations, statuses and version tokens, and the
//!   tags held on it — so afterwards the describe, the code read and the listing all report
//!   an entity that was not found, its ARN names no resource for a tag read, and the name is
//!   free for a later create to claim, while every other connection function stands;
//! * the operation addresses the resource by the `Id` its create minted, so every other
//!   identifier — the name it was created under among them — names no connection function to
//!   delete: HTTP 404 `EntityNotFound`, and nothing is removed;
//! * the removal only happens at the version its caller read: `IfMatch` is "The current
//!   version (ETag value) of the connection function you are deleting", which is the version
//!   a describe that names no stage hands back, so a header that carries no version token at
//!   all is missing or not valid (`InvalidIfMatchVersion`, HTTP 400), one that carries a
//!   well-formed token that is not the current one is a failed precondition
//!   (`PreconditionFailed`, HTTP 412), and a refused delete removes NOTHING;
//! * a connection function a stored distribution configuration still names is refused with
//!   `CannotDeleteEntityWhileInUse` (HTTP 409) and stays readable, and the same delete
//!   succeeds once the distribution no longer names it.
//!
//! The table rows differ along everything the delete generalises over — the name's length
//! (both published bounds of `FunctionName`) and the character classes its pattern admits,
//! the runtime (every value the modelled `FunctionRuntime` enum admits), the comment's length
//! and whether it is ASCII, the code's size (the modelled minimum, a real script and the
//! published quota bound) and whether it is text at all, whether the configuration associates
//! a key value store, whether the create carried tags, and whether the connection function
//! had been published to `LIVE` before the delete — because the record a delete removes
//! carries every one of them.

use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_code::get_connection_function_refusal;
use crate::connection_function_lifecycle::{
    create_connection_function, create_table, describe_connection_function, key_value_store_arn,
    member, Create,
};
use crate::connection_function_list::{list_body, list_connection_functions, summaries};
use crate::connection_function_publish::publish_connection_function;
use crate::distribution_lifecycle::{
    call, create_distribution, emulator, update_distribution, Answer, API_VERSION,
};
use crate::distribution_tags::list_tags_for_resource;
use crate::distributions_by_connection_function::config_xml;

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The wire error codes this surface refuses with.
const ENTITY_NOT_FOUND: &str = "EntityNotFound";
const NO_SUCH_RESOURCE: &str = "NoSuchResource";

/// The sentence CloudFront refuses a connection function it does not hold with — the whole
/// of it, carrying no identifier and no stage — spelled out here so a wrong constant in the
/// crate cannot make these assertions pass.
const CONNECTION_FUNCTION_NOT_FOUND_SENTENCE: &str =
    "The specified connection function does not exist.";

/// The sentence the model documents on `CannotDeleteEntityWhileInUse`
/// (`smithy_query(query="doc", shape="CannotDeleteEntityWhileInUse")`), spelled out here so a
/// wrong constant in the crate cannot make this assertion pass.
const ENTITY_IN_USE_MESSAGE: &str = "The entity cannot be deleted while it is in use.";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no connection function in a fresh emulator holds, so a precondition carrying it
/// is STALE rather than invalid.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

// ---------------------------------------------------------------------------
// Calling the delete
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/connection-function/{Id}`, carrying `if_match` as the `If-Match`
/// header the operation binds its required `IfMatch` member to.
///
/// `pub(crate)` so the suite that asserts the wording of this surface's refusals drives the
/// delete through this one call path rather than a second copy of it.
pub(crate) async fn delete_connection_function(
    router: &MultiServiceRouter,
    id: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/connection-function/{id}"),
        None,
        Some(if_match),
    )
    .await
}

/// What one create handed back: the identity it minted and the version token of the
/// `DEVELOPMENT` stage it stored.
#[derive(Debug, Clone)]
struct Created {
    id: String,
    arn: String,
    etag: String,
}

/// Create the connection function `create` asks for and answer what the create minted.
async fn create_one(router: &MultiServiceRouter, create: &Create) -> Created {
    let case = create.case;
    let answer = create_connection_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected HTTP 201 from the create, got {} with {}",
        answer.status, answer.raw
    );
    let root = answer.root();
    Created {
        id: member(root, "Id", case, &answer.raw),
        arn: member(root, "ConnectionFunctionArn", case, &answer.raw),
        etag: answer.header("etag").to_string(),
    }
}

/// The same create under another name, so a row runs against a connection function of its
/// own.
fn renamed(create: &Create, name: &str) -> Create {
    let mut renamed = create.clone();
    renamed.name = name.to_string();
    renamed
}

/// The version token the `DEVELOPMENT` stage of the connection function `id` names stands at
/// — the version a describe that names no stage hands a caller, and so the one a delete is
/// checked against.
async fn development_etag(router: &MultiServiceRouter, id: &str) -> String {
    let described = describe_connection_function(router, id, Some(STAGE_DEVELOPMENT)).await;
    assert_eq!(
        described.status, 200,
        "the DEVELOPMENT stage of {id} must be readable, got {} with {}",
        described.status, described.raw
    );
    described.header("etag").to_string()
}

/// The ids the listing of `stage` reports, in document order.
async fn listed_ids(router: &MultiServiceRouter, stage: &str) -> Vec<String> {
    let answer = list_connection_functions(router, &list_body(None, None, Some(stage))).await;
    assert_eq!(
        answer.status, 200,
        "the {stage} listing must answer HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    summaries(&answer)
        .iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Assert every read of the connection function `created` reports an entity that was not
/// found: the describe that names no stage, the describe of each stage, the describe by the
/// name it was created under, the code read, the tag read of its ARN, and both listings.
async fn assert_gone(router: &MultiServiceRouter, created: &Created, name: &str, case: &str) {
    for stage in [None, Some(STAGE_DEVELOPMENT), Some(STAGE_LIVE)] {
        let described = describe_connection_function(router, &created.id, stage).await;
        assert_eq!(
            described.status, 404,
            "case {case}: expected HTTP 404 describing {stage:?} of the deleted connection \
             function, got {} with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.error_code(),
            ENTITY_NOT_FOUND,
            "case {case}: expected {ENTITY_NOT_FOUND} describing {stage:?}, got {}",
            described.raw
        );
        let message = described.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, CONNECTION_FUNCTION_NOT_FOUND_SENTENCE,
            "case {case}: expected the fixed not-found sentence describing {stage:?}, got \
             {message:?}"
        );
    }

    // The name the create was given names nothing either: the record both namespaces
    // resolved to is gone.
    let by_name = describe_connection_function(router, name, None).await;
    assert_eq!(
        by_name.status, 404,
        "case {case}: expected HTTP 404 describing the deleted connection function by name, got \
         {} with {}",
        by_name.status, by_name.raw
    );

    let code = get_connection_function_refusal(router, &created.id, None).await;
    assert_eq!(
        code.status, 404,
        "case {case}: expected HTTP 404 reading the code of the deleted connection function, got \
         {} with {}",
        code.status, code.raw
    );

    // The tags were held on the record the delete removed, so the ARN names no resource for
    // a tag read any more.
    let tags = list_tags_for_resource(router, &created.arn).await;
    assert_eq!(
        tags.status, 404,
        "case {case}: expected HTTP 404 reading the tags of the deleted connection function, got \
         {} with {}",
        tags.status, tags.raw
    );
    assert_eq!(
        tags.error_code(),
        NO_SUCH_RESOURCE,
        "case {case}: expected {NO_SUCH_RESOURCE} from the tag read, got {}",
        tags.raw
    );

    for stage in [STAGE_DEVELOPMENT, STAGE_LIVE] {
        let listed = listed_ids(router, stage).await;
        assert!(
            !listed.contains(&created.id),
            "case {case}: expected the {stage} listing to leave the deleted connection function \
             out, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A delete at the current version removes the whole record: the answer carries HTTP 204 and
/// no document, every later read reports an entity that was not found, neither listing names
/// it, and the name is free for a later create — while a connection function the delete did
/// not name stands untouched. Every other row is published to `LIVE` first, so the delete is
/// asserted over a connection function with both stages as well as one with only
/// `DEVELOPMENT`.
#[tokio::test]
async fn a_delete_removes_the_whole_connection_function_record() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_delete_store").await;
    let table = create_table(&kvs_arn);

    // The connection function nothing ever deletes: the control every row below is checked
    // against.
    let survivor = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "connection_function_nothing_deletes",
    );
    let survived = create_one(&router, &survivor).await;

    for (index, row) in table.iter().enumerate() {
        let case = row.case;
        let create = renamed(row, &format!("delete_probe_{index}"));
        let created = create_one(&router, &create).await;

        // Every other row is published first, so the delete has to remove a connection
        // function with a LIVE stage as well as one that only ever had DEVELOPMENT.
        let published = index % 2 == 0;
        if published {
            let answer = publish_connection_function(&router, &created.id, &created.etag).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: the seed publish must answer HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let live = describe_connection_function(&router, &created.id, Some(STAGE_LIVE)).await;
            assert_eq!(
                live.status, 200,
                "case {case}: the published stage must be readable before the delete, got {} \
                 with {}",
                live.status, live.raw
            );
            let live_listed = listed_ids(&router, STAGE_LIVE).await;
            assert!(
                live_listed.contains(&created.id),
                "case {case}: expected the LIVE listing to name the published connection \
                 function before the delete, got {live_listed:?}"
            );
        }

        let listed = listed_ids(&router, STAGE_DEVELOPMENT).await;
        assert!(
            listed.contains(&created.id),
            "case {case}: expected the DEVELOPMENT listing to name the connection function \
             before the delete, got {listed:?}"
        );
        // The tags are readable while the record is there, whether the create carried any or
        // not, so the refusal after the delete is the record's removal rather than the tag
        // surface refusing this resource kind.
        let tags = list_tags_for_resource(&router, &created.arn).await;
        assert_eq!(
            tags.status, 200,
            "case {case}: expected the tags to be readable before the delete, got {} with {}",
            tags.status, tags.raw
        );

        let deleted = delete_connection_function(&router, &created.id, &created.etag).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 from the delete, got {} with {}",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected no document from the delete, got {}",
            deleted.raw
        );

        assert_gone(&router, &created, &create.name, case).await;

        // The connection function the delete did not name is untouched, at the version it
        // stood at.
        let survivor_now = development_etag(&router, &survived.id).await;
        assert_eq!(
            survivor_now, survived.etag,
            "case {case}: expected the untouched connection function to keep its version, got \
             {survivor_now:?}"
        );

        // "Gone means gone": the name is free, and the create that claims it mints an
        // identity of its own rather than resurrecting the deleted record.
        let again = create_one(&router, &create).await;
        assert_ne!(
            again.id, created.id,
            "case {case}: expected the re-created connection function to carry an id of its own"
        );
        assert_ne!(
            again.etag, created.etag,
            "case {case}: expected the re-created connection function to carry a version token \
             of its own"
        );
        // The re-created connection function is in DEVELOPMENT only: the LIVE stage the
        // delete removed did not come back with the name.
        let live = describe_connection_function(&router, &again.id, Some(STAGE_LIVE)).await;
        assert_eq!(
            live.status, 404,
            "case {case}: expected the re-created connection function to have no LIVE stage, got \
             {} with {}",
            live.status, live.raw
        );
    }
}

/// The `If-Match` header one precondition row sends.
enum Token {
    /// Built from the current version token of the connection function's `DEVELOPMENT` stage.
    Current(fn(&str) -> String),
    /// A value that stands on its own, whatever version the store holds.
    Literal(String),
}

/// One `If-Match` header, and what the service is expected to answer it with: `None` to
/// delete, or the status and error code the refusal carries.
struct Precondition {
    case: &'static str,
    token: Token,
    refusal: Option<(u16, &'static str)>,
}

/// The removal only happens at the version its caller read. A header carrying the current
/// version token — bare, quoted as an HTTP entity tag, or quoted as a weak one — deletes;
/// anything that is no version token at all is refused as missing or not valid; a well-formed
/// token that belongs to another version is a failed precondition. A refused delete removes
/// nothing: the connection function is still readable at the version it stood at.
#[tokio::test]
async fn a_delete_needs_the_current_version_of_the_connection_function() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_delete_precondition_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    // A version token this service really minted that belongs to another connection function,
    // so the stale case is a well-formed token rather than a made-up string.
    let foreign = create_one(&router, &renamed(&row, "delete_foreign_version_source")).await;

    let cases = [
        Precondition {
            case: "the current version token, bare",
            token: Token::Current(|current| current.to_string()),
            refusal: None,
        },
        Precondition {
            case: "the current version token, quoted as an entity tag",
            token: Token::Current(|current| format!("\"{current}\"")),
            refusal: None,
        },
        Precondition {
            case: "the current version token, quoted as a weak entity tag",
            token: Token::Current(|current| format!("W/\"{current}\"")),
            refusal: None,
        },
        Precondition {
            case: "a value that is no version token at all",
            token: Token::Literal("not-a-version".to_string()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "a token of the right shape in the wrong case",
            token: Token::Current(|current| current.to_lowercase()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "a token whose body is too short",
            token: Token::Literal("E123".to_string()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "a well-formed token no connection function holds",
            token: Token::Literal(UNHELD_VERSION_TOKEN.to_string()),
            refusal: Some((412, "PreconditionFailed")),
        },
        Precondition {
            case: "the version token of another connection function",
            token: Token::Literal(foreign.etag.clone()),
            refusal: Some((412, "PreconditionFailed")),
        },
    ];

    for (index, precondition) in cases.iter().enumerate() {
        let case = precondition.case;
        // One connection function per row, so what a row deletes cannot change what the next
        // one reads.
        let create = renamed(&row, &format!("delete_precondition_probe_{index}"));
        let created = create_one(&router, &create).await;
        let sent = match &precondition.token {
            Token::Current(render) => render(&created.etag),
            Token::Literal(literal) => literal.clone(),
        };

        let answer = delete_connection_function(&router, &created.id, &sent).await;

        match precondition.refusal {
            None => {
                assert_eq!(
                    answer.status, 204,
                    "case {case}: expected HTTP 204 deleting with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                assert_gone(&router, &created, &create.name, case).await;
            }
            Some((status, code)) => {
                assert_eq!(
                    answer.status, status,
                    "case {case}: expected HTTP {status} deleting with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case}: expected {code}, got {}",
                    answer.raw
                );
                // A refused delete removes nothing: the record still answers reads, at the
                // version it stood at and in the listing it was in.
                let still = development_etag(&router, &created.id).await;
                assert_eq!(
                    still, created.etag,
                    "case {case}: a refused delete must leave the connection function at its \
                     version, got {still:?}"
                );
                let listed = listed_ids(&router, STAGE_DEVELOPMENT).await;
                assert!(
                    listed.contains(&created.id),
                    "case {case}: a refused delete must leave the connection function in the \
                     listing, got {listed:?}"
                );
            }
        }
    }
}

/// The operation addresses the resource by "The connection function's ID" the create minted,
/// so every other identifier — the name it was created under among them — names no connection
/// function to delete: HTTP 404 `EntityNotFound`, whatever the `If-Match` carries, and nothing
/// is removed.
#[tokio::test]
async fn an_id_no_connection_function_answers_to_is_not_found() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_delete_unknown_id_store").await;
    let table = create_table(&kvs_arn);
    let create = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "delete_present_connection_function",
    );
    let created = create_one(&router, &create).await;

    let unknown = [
        ("the name the create was given", create.name.clone()),
        ("an id of the minted shape nothing holds", {
            // Flip the last character of a real id, so the value has the shape this service
            // mints and still names nothing.
            let mut other = created.id.clone();
            other.pop();
            format!("{other}0")
        }),
        ("a value of no particular shape", "missing".to_string()),
    ];

    for (case, identifier) in unknown {
        for (token_case, token) in [
            (
                "the version of the connection function that is there",
                created.etag.clone(),
            ),
            (
                "a well-formed version token nothing holds",
                UNHELD_VERSION_TOKEN.to_string(),
            ),
        ] {
            let answer = delete_connection_function(&router, &identifier, &token).await;
            assert_eq!(
                answer.status, 404,
                "case {case} / {token_case}: expected HTTP 404 deleting {identifier:?}, got {} \
                 with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                ENTITY_NOT_FOUND,
                "case {case} / {token_case}: expected {ENTITY_NOT_FOUND}, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, CONNECTION_FUNCTION_NOT_FOUND_SENTENCE,
                "case {case} / {token_case}: expected the fixed not-found sentence, got \
                 {message:?}"
            );
        }
    }

    // Nothing was removed: the connection function that is there still stands at its version.
    let still = development_etag(&router, &created.id).await;
    assert_eq!(
        still, created.etag,
        "a delete of another identifier must leave the stored connection function alone, got \
         {still:?}"
    );
}

/// A connection function a stored distribution configuration names is refused with
/// `CannotDeleteEntityWhileInUse` and stays readable, and the SAME delete succeeds once the
/// distribution no longer names it. A connection function no stored distribution names is
/// deletable throughout.
#[tokio::test]
async fn a_connection_function_a_distribution_names_cannot_be_deleted() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_delete_association_store").await;
    let table = create_table(&kvs_arn);

    for (index, row) in table.iter().enumerate() {
        let case = row.case;
        let create = renamed(row, &format!("delete_named_connection_function_{index}"));
        let created = create_one(&router, &create).await;

        // A distribution whose configuration names the connection function by the id the
        // create minted.
        let reference = format!("delete-association-{index}");
        let distribution =
            create_distribution(&router, &config_xml(&reference, Some(&created.id))).await;
        assert_eq!(
            distribution.status, 201,
            "case {case}: expected HTTP 201 creating the distribution, got {} with {}",
            distribution.status, distribution.raw
        );
        let distribution_id = member(distribution.root(), "Id", case, &distribution.raw);
        let distribution_etag = distribution.header("etag").to_string();

        let refused = delete_connection_function(&router, &created.id, &created.etag).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 deleting the named connection function, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "CannotDeleteEntityWhileInUse",
            "case {case}: expected CannotDeleteEntityWhileInUse, got {}",
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, ENTITY_IN_USE_MESSAGE,
            "case {case}: expected the modelled in-use sentence, got {message:?}"
        );
        // The refused delete removed nothing.
        let still = development_etag(&router, &created.id).await;
        assert_eq!(
            still, created.etag,
            "case {case}: the refused delete must leave the connection function at its version, \
             got {still:?}"
        );

        // Once the distribution no longer names it, the same delete goes through.
        let updated = update_distribution(
            &router,
            &distribution_id,
            &config_xml(&reference, None),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 removing the association, got {} with {}",
            updated.status, updated.raw
        );

        let deleted = delete_connection_function(&router, &created.id, &created.etag).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 deleting the connection function nothing names any \
             more, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &created, &create.name, case).await;
    }

    // A connection function no stored distribution names is deletable straight away, even
    // though distributions of this emulator's store name others.
    let control = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "delete_connection_function_nothing_names",
    );
    let created = create_one(&router, &control).await;
    let deleted = delete_connection_function(&router, &created.id, &created.etag).await;
    assert_eq!(
        deleted.status, 204,
        "case the connection function nothing names: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );
    assert_gone(
        &router,
        &created,
        &control.name,
        "the connection function nothing names",
    )
    .await;
}
