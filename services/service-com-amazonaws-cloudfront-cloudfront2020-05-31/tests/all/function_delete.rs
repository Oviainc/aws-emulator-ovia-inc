// SPDX-License-Identifier: CC0-1.0

//! `DeleteFunction`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for the delete:
//!
//! * a delete REMOVES the one record every read of that function answered from — both of its
//!   stages, its code, its configuration, its version tokens and its tags — so afterwards
//!   the describe, the get and the listing all report a function that does not exist and the
//!   name is free for a later create to claim, while every other function stands;
//! * the removal only happens at the version its caller read: `IfMatch` is "The current
//!   version (ETag value) of the function that you are deleting, which you can get using
//!   DescribeFunction", so a header that does not carry THIS function's current version —
//!   one that is no version token at all, and one that is a well-formed but superseded or
//!   foreign token alike — is missing or not valid (`InvalidIfMatchVersion`, HTTP 400), and a
//!   refused delete removes NOTHING;
//! * a name no function holds is "The specified function does not exist."
//!   (`NoSuchFunctionExists`, HTTP 404), whatever the `If-Match` carries, and removes
//!   nothing — and a function that was deleted is refused with exactly that sentence, the
//!   same one a name that never existed gets, because the refusal is a property of the store
//!   rather than of the request;
//! * "You cannot delete a function if it's associated with a cache behavior. First, update
//!   your distributions to remove the function association from all cache behaviors, then
//!   delete the function": a function a stored cache behaviour attaches by ARN is refused
//!   with `FunctionInUse` (HTTP 409) and stays readable, and the same delete succeeds once
//!   the distribution no longer attaches it.
//!
//! The table rows differ along everything the delete generalises over — the name's length
//! (both published bounds of `FunctionName`) and the character classes its pattern admits,
//! the runtime (every value the modelled `FunctionRuntime` enum admits), the comment, the
//! code's size (the modelled minimum, a real script and the published quota bound) and
//! whether it is text at all, whether the configuration associates a key value store,
//! whether the create carried tags, and whether the function had been published to `LIVE`
//! before the delete — because the record a delete removes carries every one of them.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, update_distribution, Answer, Node, API_VERSION,
};
use crate::function_lifecycle::{
    create_function, create_key_value_store, create_table, describe_function, get_function, member,
    Create,
};
use crate::function_publish::{create_one, publish_function};
use crate::function_update::update_function;

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no function in a fresh emulator holds, so a precondition carrying it is STALE
/// rather than invalid.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

/// The sentence CloudFront answers a read of a function it does not hold with, in the form it
/// words a `NoSuch…` for a named resource (`smithy_query(query="doc",
/// shape="NoSuchFunctionExists")` states it in its shorter form), spelled out here so a wrong
/// constant in the crate cannot make this assertion pass.
const NO_SUCH_FUNCTION_MESSAGE: &str = "The specified function does not exist.";

/// The sentence CloudFront answers a delete of an attached function with, for the function
/// `name` that `distributions` distributions attach — the served template, spelled out here
/// so a wrong template in the crate cannot make this assertion pass.
fn function_in_use_message(name: &str, distributions: usize) -> String {
    format!("Cannot delete function {name}, it is in use by {distributions} distributions")
}

// ---------------------------------------------------------------------------
// Calling the delete
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/function/{Name}`, carrying `if_match` as the `If-Match` header the
/// operation binds its required `IfMatch` member to.
pub(crate) async fn delete_function(
    router: &MultiServiceRouter,
    name: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/function/{name}"),
        None,
        Some(if_match),
    )
    .await
}

/// `GET /2020-05-31/function` — the listing a deleted function must no longer appear in.
async fn list_function_names(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", &format!("/{API_VERSION}/function"), None, None).await;
    assert_eq!(
        answer.status, 200,
        "the function listing must answer HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("FunctionSummary")
                .iter()
                .map(|summary: &&Node| summary.text_at("Name").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The same create under another name, so a row runs against a function of its own.
fn renamed(create: &Create, name: &str) -> Create {
    let mut renamed = create.clone();
    renamed.name = name.to_string();
    renamed
}

/// The version token the `DEVELOPMENT` stage of `name` stands at — the version
/// `DescribeFunction` hands a caller that names no stage, and so the one a delete is checked
/// against.
async fn development_etag(router: &MultiServiceRouter, name: &str) -> String {
    let described = describe_function(router, name, Some(STAGE_DEVELOPMENT)).await;
    assert_eq!(
        described.status, 200,
        "the DEVELOPMENT stage of {name} must be readable, got {} with {}",
        described.status, described.raw
    );
    described.header("etag").to_string()
}

/// Assert every read of `name` reports a function that does not exist: the describe that
/// names no stage, the describe of each stage, and the get of the code.
async fn assert_gone(router: &MultiServiceRouter, name: &str, case: &str) {
    for stage in [None, Some(STAGE_DEVELOPMENT), Some(STAGE_LIVE)] {
        let described = describe_function(router, name, stage).await;
        assert_eq!(
            described.status, 404,
            "case {case}: expected HTTP 404 describing {stage:?} of the deleted function, got {} \
             with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.error_code(),
            "NoSuchFunctionExists",
            "case {case}: expected NoSuchFunctionExists describing {stage:?}, got {}",
            described.raw
        );
        let message = described.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_FUNCTION_MESSAGE,
            "case {case}: expected the fixed not-found sentence describing {stage:?}, got \
             {message:?}"
        );
    }

    let code = get_function(router, name, None).await;
    assert_eq!(
        code.status, 404,
        "case {case}: expected HTTP 404 getting the code of the deleted function, got {}",
        code.status
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A delete at the current version removes the whole record: the answer carries HTTP 204 and
/// no document, every later read of that name reports a function that does not exist, the
/// listing no longer names it, and the name is free for a later create — while a function the
/// delete did not name stands untouched. Half the rows are published to `LIVE` first, so the
/// delete is asserted over a function with both stages as well as one with only `DEVELOPMENT`.
#[tokio::test]
async fn a_delete_removes_the_whole_function_record() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_delete_store").await;
    let table = create_table(&kvs_arn);

    // The function nothing ever deletes: the control every row below is checked against.
    let survivor = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "function_nothing_deletes",
    );
    let survivor_etag = create_one(&router, &survivor).await;

    for (index, row) in table.iter().enumerate() {
        let case = row.case;
        let create = renamed(row, &format!("delete_probe_{index}"));
        let created_etag = create_one(&router, &create).await;

        // Every other row is published first, so the delete has to remove a function with a
        // LIVE stage as well as one that only ever had DEVELOPMENT.
        let published = index % 2 == 0;
        if published {
            let answer = publish_function(&router, &create.name, &created_etag).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: the seed publish must answer HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;
            assert_eq!(
                live.status, 200,
                "case {case}: the published stage must be readable before the delete, got {} \
                 with {}",
                live.status, live.raw
            );
        }

        let listed = list_function_names(&router).await;
        assert!(
            listed.contains(&create.name),
            "case {case}: expected the listing to name the function before the delete, got \
             {listed:?}"
        );

        let deleted = delete_function(&router, &create.name, &created_etag).await;
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

        assert_gone(&router, &create.name, case).await;
        let listed = list_function_names(&router).await;
        assert!(
            !listed.contains(&create.name),
            "case {case}: expected the listing to leave the deleted function out, got {listed:?}"
        );

        // The function the delete did not name is untouched, at the version it stood at.
        let survivor_now = development_etag(&router, &survivor.name).await;
        assert_eq!(
            survivor_now, survivor_etag,
            "case {case}: expected the untouched function to keep its version, got \
             {survivor_now:?}"
        );

        // "Gone means gone": the name is free, and the create that claims it mints a version
        // token of its own rather than resurrecting the deleted record.
        let again = create_function(&router, &create.xml()).await;
        assert_eq!(
            again.status, 201,
            "case {case}: expected the name to be free for a later create, got {} with {}",
            again.status, again.raw
        );
        let again_etag = again.header("etag").to_string();
        assert_ne!(
            again_etag, created_etag,
            "case {case}: expected the re-created function to carry a version token of its own"
        );
        // The re-created function is in DEVELOPMENT only: the LIVE stage the delete removed
        // did not come back with the name.
        let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;
        assert_eq!(
            live.status, 404,
            "case {case}: expected the re-created function to have no LIVE stage, got {} with {}",
            live.status, live.raw
        );
    }
}

/// The `If-Match` header one precondition row sends.
enum Token {
    /// Built from the current version token of the function's `DEVELOPMENT` stage.
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
/// token that belongs to another version is refused too. A refused delete removes
/// nothing: the function is still readable at the version it stood at, serving the code it
/// stored.
#[tokio::test]
async fn a_delete_needs_the_current_version_of_the_function() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_delete_precondition_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    // A version token this service really minted that belongs to another function, so the
    // stale case is a well-formed token rather than a made-up string.
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
            case: "a well-formed token no function holds",
            token: Token::Literal(UNHELD_VERSION_TOKEN.to_string()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
        Precondition {
            case: "the version token of another function",
            token: Token::Literal(foreign.clone()),
            refusal: Some((400, "InvalidIfMatchVersion")),
        },
    ];

    for (index, precondition) in cases.iter().enumerate() {
        let case = precondition.case;
        // One function per row, so what a row deletes cannot change what the next one reads.
        let create = renamed(&row, &format!("delete_precondition_probe_{index}"));
        let created_etag = create_one(&router, &create).await;
        let sent = match &precondition.token {
            Token::Current(render) => render(&created_etag),
            Token::Literal(literal) => literal.clone(),
        };

        let answer = delete_function(&router, &create.name, &sent).await;

        match precondition.refusal {
            None => {
                assert_eq!(
                    answer.status, 204,
                    "case {case}: expected HTTP 204 deleting with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                assert_gone(&router, &create.name, case).await;
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
                // version it stood at and with the code it stored.
                let still = development_etag(&router, &create.name).await;
                assert_eq!(
                    still, created_etag,
                    "case {case}: a refused delete must leave the function at its version, got \
                     {still:?}"
                );
                let code = get_function(&router, &create.name, None).await;
                assert_eq!(
                    code.status, 200,
                    "case {case}: a refused delete must leave the code readable, got {}",
                    code.status
                );
                assert_eq!(
                    code.body, create.code,
                    "case {case}: expected the {} bytes the create stored, got {} bytes",
                    create.code.len(),
                    code.body.len()
                );
            }
        }
    }
}

/// The version an update SUPERSEDED is a well-formed token that is no longer the current one,
/// so a delete holding it is refused and removes nothing — and the delete that
/// carries the version the update handed back removes the function.
#[tokio::test]
async fn the_version_an_update_superseded_no_longer_deletes() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_delete_superseded_store").await;

    for (index, row) in create_table(&kvs_arn).into_iter().enumerate() {
        let case = row.case;
        let create = renamed(&row, &format!("delete_superseded_probe_{index}"));
        let stale = create_one(&router, &create).await;

        // The update replaces what the create stored and mints a fresh version token, so the
        // token the create handed back is out of date from here on.
        let updated = update_function(&router, &create.name, &create, Some(&stale)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: the seed update must answer HTTP 200, got {} with {}",
            updated.status, updated.raw
        );
        let current = development_etag(&router, &create.name).await;
        assert_ne!(
            current, stale,
            "case {case}: expected the update to mint a version token of its own"
        );

        let refused = delete_function(&router, &create.name, &stale).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400 deleting the superseded version, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidIfMatchVersion",
            "case {case}: expected InvalidIfMatchVersion, got {}",
            refused.raw
        );
        let still = describe_function(&router, &create.name, None).await;
        assert_eq!(
            still.status, 200,
            "case {case}: the refused delete must leave the function readable, got {} with {}",
            still.status, still.raw
        );

        let deleted = delete_function(&router, &create.name, &current).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 deleting at the current version, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &create.name, case).await;
    }
}

/// The operation addresses the function by "The name of the function that you are deleting",
/// so a name no function holds is "The function does not exist" (HTTP 404) and removes
/// nothing — whatever version the header carries.
#[tokio::test]
async fn a_name_no_function_holds_is_not_found() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_delete_unknown_name_store").await;
    let table = create_table(&kvs_arn);
    let create = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "delete_present_function",
    );
    let created_etag = create_one(&router, &create).await;

    let unknown = [
        ("another admissible name", "delete_absent_function".to_string()),
        (
            "the name of the present function with a character added",
            format!("{}x", create.name),
        ),
        ("the longest admissible name", "z".repeat(64)),
    ];

    for (case, name) in unknown {
        for (token_case, token) in [
            ("the version of the function that is there", &created_etag),
            (
                "a well-formed version token nothing holds",
                &UNHELD_VERSION_TOKEN.to_string(),
            ),
        ] {
            let answer = delete_function(&router, &name, token).await;
            assert_eq!(
                answer.status, 404,
                "case {case} / {token_case}: expected HTTP 404 deleting {name:?}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchFunctionExists",
                "case {case} / {token_case}: expected NoSuchFunctionExists, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, NO_SUCH_FUNCTION_MESSAGE,
                "case {case} / {token_case}: expected the fixed not-found sentence, got \
                 {message:?}"
            );
        }
    }

    // Nothing was removed: the function that is there still stands at its version.
    let still = development_etag(&router, &create.name).await;
    assert_eq!(
        still, created_etag,
        "a delete of another name must leave the present function alone, got {still:?}"
    );
}

/// Where a distribution attaches the function under test. A behaviour carries its functions
/// in a `FunctionAssociations` list, and a distribution's behaviours are its DEFAULT cache
/// behaviour together with its ordered ones, so both placements are exercised.
#[derive(Debug, Clone, Copy)]
enum Attachment {
    Default,
    Ordered,
    /// No behaviour of the distribution attaches the function at all — the shape the
    /// configuration takes once the caller has removed the association.
    None,
}

/// The `FunctionAssociations` wrapper attaching `arn` to a cache behaviour. `EventType`
/// `viewer-request` is one of the values the modelled `EventType` enum admits
/// (`smithy_query(query="shape", shape="EventType")`).
fn associations_xml(arn: &str) -> String {
    format!(
        "<FunctionAssociations><Quantity>1</Quantity><Items><FunctionAssociation>\
         <FunctionARN>{arn}</FunctionARN><EventType>viewer-request</EventType>\
         </FunctionAssociation></Items></FunctionAssociations>"
    )
}

/// The `DistributionConfig` document for a distribution that attaches `arn` at `attachment`.
fn distribution_xml(reference: &str, arn: &str, attachment: Attachment) -> String {
    let ordered = |associations: String| {
        format!(
            "<Quantity>2</Quantity><Items>\
             <CacheBehavior><PathPattern>/static/*</PathPattern>\
             <TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></CacheBehavior>\
             <CacheBehavior><PathPattern>/api/*</PathPattern>\
             <TargetOriginId>primary</TargetOriginId>\
             <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{associations}</CacheBehavior>\
             </Items>"
        )
    };
    let (default_attachment, behaviors) = match attachment {
        Attachment::Default => (associations_xml(arn), "<Quantity>0</Quantity>".to_string()),
        // A behaviour that attaches nothing comes first, so the walk has to reach past it.
        Attachment::Ordered => (String::new(), ordered(associations_xml(arn))),
        // The same two behaviours, with the association taken off the one that carried it.
        Attachment::None => (String::new(), ordered(String::new())),
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{default_attachment}\
         </DefaultCacheBehavior>\
         <CacheBehaviors>{behaviors}</CacheBehaviors>\
         <Comment>{reference}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// A function a stored cache behaviour attaches by ARN — the default behaviour's or an
/// ordered one's — is refused with `FunctionInUse` and stays readable, and the SAME delete
/// succeeds once the distribution no longer attaches it. A function nothing attaches deletes
/// straight away.
#[tokio::test]
async fn a_function_a_cache_behaviour_attaches_cannot_be_deleted() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_delete_association_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    for (index, attachment) in [Attachment::Default, Attachment::Ordered]
        .into_iter()
        .enumerate()
    {
        let case = format!("{attachment:?}");
        let create = renamed(&row, &format!("delete_attached_function_{index}"));
        let created_etag = create_one(&router, &create).await;
        let arn = member(
            describe_function(&router, &create.name, None).await.root(),
            "FunctionMetadata/FunctionARN",
            &case,
            "the describe of the created function",
        );

        // A distribution whose cache behaviour attaches the function by that ARN.
        let reference = format!("delete-association-{index}");
        let distribution = create_distribution(
            &router,
            &distribution_xml(&reference, &arn, attachment),
        )
        .await;
        assert_eq!(
            distribution.status, 201,
            "case {case}: expected HTTP 201 creating the distribution, got {} with {}",
            distribution.status, distribution.raw
        );
        let distribution_id = member(distribution.root(), "Id", &case, &distribution.raw);
        let distribution_etag = distribution.header("etag").to_string();

        let refused = delete_function(&router, &create.name, &created_etag).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 deleting the attached function, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "FunctionInUse",
            "case {case}: expected FunctionInUse, got {}",
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message,
            function_in_use_message(&create.name, 1),
            "case {case}: expected the served FunctionInUse sentence, got {message:?}"
        );
        // The refused delete removed nothing.
        let still = development_etag(&router, &create.name).await;
        assert_eq!(
            still, created_etag,
            "case {case}: the refused delete must leave the function at its version, got \
             {still:?}"
        );

        // "First, update your distributions to remove the function association from all cache
        // behaviors, then delete the function."
        let updated = update_distribution(
            &router,
            &distribution_id,
            &distribution_xml(&reference, &arn, Attachment::None),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 removing the association, got {} with {}",
            updated.status, updated.raw
        );

        let deleted = delete_function(&router, &create.name, &created_etag).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 deleting the detached function, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &create.name, &case).await;
    }

    // A function no behaviour of any stored distribution attaches is deletable throughout.
    let control = renamed(&row, "delete_function_nothing_attaches");
    let control_etag = create_one(&router, &control).await;
    let deleted = delete_function(&router, &control.name, &control_etag).await;
    assert_eq!(
        deleted.status, 204,
        "case the function nothing attaches: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );
    assert_gone(&router, &control.name, "the function nothing attaches").await;
}
