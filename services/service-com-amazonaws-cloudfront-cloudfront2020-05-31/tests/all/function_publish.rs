// SPDX-License-Identifier: CC0-1.0

//! `PublishFunction`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for a publish:
//!
//! * a publish COPIES the `DEVELOPMENT` stage of the function its `Name` names into `LIVE`
//!   ("Publishes a CloudFront function by copying the function code from the DEVELOPMENT
//!   stage to LIVE"): the stage it writes carries the configuration and the code that stage
//!   holds, keeps the function's created time, is last modified at the publish and gets a
//!   FRESH version token of its own, so a later describe and get of `LIVE` answer from that
//!   record — while the stage the publish READ keeps its own status and version token;
//! * the copy only happens at the version its caller read: `IfMatch` is "The current version
//!   (ETag value) of the function that you are publishing", so a header that carries no
//!   version token at all is missing or not valid (`InvalidIfMatchVersion`, HTTP 400), one
//!   that carries a version token that is not the current one — the version an update has
//!   since superseded among them — is a failed precondition (`PreconditionFailed`, HTTP
//!   412), and a refused publish promotes NOTHING: the function still has no `LIVE` stage
//!   and its `DEVELOPMENT` stage stands at the version it stood at;
//! * a name no function holds is "The function does not exist" (`NoSuchFunctionExists`, HTTP
//!   404), whatever the `If-Match` carries, and publishes nothing;
//! * the status the written stage reports is computed from the stored distributions: a
//!   function a cache behaviour attaches by ARN is published as associated, and one nothing
//!   attaches as unassociated.
//!
//! The table rows differ along everything the copy generalises over — the name's length
//! (both published bounds of `FunctionName`) and character classes, the runtime (every value
//! the modelled `FunctionRuntime` enum admits), the comment's length and whether it is
//! ASCII, the code's size (the modelled minimum, a real script and the published quota
//! bound) and whether it is text at all, and whether the configuration associates a key
//! value store — because every one of them is part of what a publish has to carry across.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, is_version_token, Answer, API_VERSION,
};
// The function surface's request builder, callers, create table and member reader: one
// `pub(crate)` copy, in the suite that first needed them, rather than a copy per suite.
use crate::function_lifecycle::{
    create_function, create_key_value_store, create_table, describe_function, get_function, member,
    Create,
};
use crate::function_update::update_function;

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The status a stage that has never been published reports.
const STATUS_UNPUBLISHED: &str = "UNPUBLISHED";

/// The status a published stage reports while no distribution attaches the function, and the
/// one it reports while a distribution does.
const STATUS_UNASSOCIATED: &str = "UNASSOCIATED";
const STATUS_ASSOCIATED: &str = "ASSOCIATED";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that no function in a fresh emulator holds, so a precondition carrying it is STALE
/// rather than invalid.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

// ---------------------------------------------------------------------------
// Calling the publish
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/function/{Name}/publish`, carrying `if_match` as the `If-Match` header
/// the operation binds its required `IfMatch` member to.
pub(crate) async fn publish_function(
    router: &MultiServiceRouter,
    name: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/function/{name}/publish"),
        None,
        Some(if_match),
    )
    .await
}

/// Create the function `create` asks for and answer the version token of its `DEVELOPMENT`
/// stage, which is the version a publish of it is checked against.
pub(crate) async fn create_one(router: &MultiServiceRouter, create: &Create) -> String {
    let answer = create_function(router, &create.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        create.case, answer.status, answer.raw
    );
    answer.header("etag").to_string()
}

/// The same create under another name, so a row can be run against a function of its own
/// without the name of the previous one standing in the way.
fn renamed(create: &Create, name: &str) -> Create {
    let mut renamed = create.clone();
    renamed.name = name.to_string();
    renamed
}

/// The same function with a DIFFERENT configuration and code, so an update of it really
/// replaces what the create stored — and a publish taken after it has something of its own to
/// copy.
fn revised(create: &Create) -> Create {
    let mut revised = create.clone();
    // No leading or trailing space in the comment: XML text is read trimmed, so a comment
    // that carried one could not be compared against what was submitted.
    revised.comment = match create.comment.as_str() {
        "" => "revised".to_string(),
        existing => format!("{existing} revised"),
    };
    revised.code = {
        // The revision differs from what the create stored while keeping its LENGTH, so the
        // row whose code sits exactly at the published function-size quota is revised rather
        // than refused for being too large.
        let mut code: Vec<u8> = create.code.iter().rev().copied().collect();
        code[0] = code[0].wrapping_add(1);
        code
    };
    revised
}

/// Assert the `FunctionSummary` document `answer` carries is the function `create` asked
/// for, in `stage` and with `status`.
fn assert_summary(answer: &Answer, create: &Create, stage: &str, status: &str, context: &str) {
    let case = create.case;
    let summary = answer.root();
    let raw = answer.raw.as_str();

    let name = member(summary, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let reported_status = member(summary, "Status", case, raw);
    assert_eq!(
        reported_status, status,
        "case {case} ({context}): expected Status {status}, got {reported_status:?}"
    );

    let reported_stage = member(summary, "FunctionMetadata/Stage", case, raw);
    assert_eq!(
        reported_stage, stage,
        "case {case} ({context}): expected Stage {stage}, got {reported_stage:?}"
    );

    let comment = summary
        .text_at("FunctionConfig/Comment")
        .unwrap_or("")
        .to_string();
    assert_eq!(
        comment, create.comment,
        "case {case} ({context}): expected the submitted Comment, got {comment:?}"
    );

    let runtime = member(summary, "FunctionConfig/Runtime", case, raw);
    assert_eq!(
        runtime, create.runtime,
        "case {case} ({context}): expected the submitted Runtime, got {runtime:?}"
    );

    let associated: Vec<String> = summary
        .at("FunctionConfig/KeyValueStoreAssociations/Items")
        .map(|items| {
            items
                .children_named("KeyValueStoreAssociation")
                .iter()
                .map(|association| {
                    association
                        .text_at("KeyValueStoreARN")
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        associated, create.associations,
        "case {case} ({context}): expected the submitted key value store associations, got \
         {associated:?}"
    );
}

/// The ARN a function of `name` carries, as the summary of `answer` reports it.
fn reported_arn(answer: &Answer, case: &str) -> String {
    member(answer.root(), "FunctionMetadata/FunctionARN", case, &answer.raw)
}

/// The version token the `DEVELOPMENT` stage of `name` stands at.
async fn development_etag(router: &MultiServiceRouter, name: &str) -> String {
    let described = describe_function(router, name, Some(STAGE_DEVELOPMENT)).await;
    assert_eq!(
        described.status, 200,
        "the DEVELOPMENT stage of {name} must be readable, got {} with {}",
        described.status, described.raw
    );
    described.header("etag").to_string()
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A publish COPIES the `DEVELOPMENT` stage into `LIVE`: the summary it answers with, and
/// every later read of the `LIVE` stage, report the stored configuration and code under a
/// version token of their own — while the stage the publish read keeps its own status and
/// token. Across the table, no two published stages share a token.
#[tokio::test]
async fn a_publish_copies_development_into_live_and_the_reads_report_it() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_publish_store").await;
    let mut live_etags = BTreeSet::new();

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let created_etag = create_one(&router, &create).await;
        let created_time = member(
            describe_function(&router, &create.name, None).await.root(),
            "FunctionMetadata/CreatedTime",
            case,
            "the describe of the created function",
        );

        let published = publish_function(&router, &create.name, &created_etag).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected HTTP 200 from the publish, got {} with {}",
            published.status, published.raw
        );
        assert_summary(
            &published,
            &create,
            STAGE_LIVE,
            STATUS_UNASSOCIATED,
            "the publish",
        );

        // A copy keeps the created time of the function it was taken from.
        let published_created = member(
            published.root(),
            "FunctionMetadata/CreatedTime",
            case,
            &published.raw,
        );
        assert_eq!(
            published_created, created_time,
            "case {case}: expected the CreatedTime of the function the copy was taken from, got \
             {published_created:?}"
        );

        // The LIVE stage answers reads from the record the publish wrote, under a version
        // token of its own.
        let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;
        assert_eq!(
            live.status, 200,
            "case {case}: expected HTTP 200 describing the LIVE stage, got {} with {}",
            live.status, live.raw
        );
        assert_summary(
            &live,
            &create,
            STAGE_LIVE,
            STATUS_UNASSOCIATED,
            "the describe of LIVE",
        );
        let live_etag = live.header("etag").to_string();
        assert!(
            is_version_token(&live_etag),
            "case {case}: expected a version token for the LIVE stage, got {live_etag:?}"
        );
        assert_ne!(
            live_etag, created_etag,
            "case {case}: expected the published stage to carry a version token of its own"
        );
        assert!(
            live_etags.insert(live_etag.clone()),
            "case {case}: the version token {live_etag} repeated"
        );

        // The code the publish copied is served from the LIVE stage, byte for byte.
        let code = get_function(&router, &create.name, Some(STAGE_LIVE)).await;
        assert_eq!(
            code.status, 200,
            "case {case}: expected HTTP 200 getting the LIVE code, got {}",
            code.status
        );
        assert_eq!(
            code.body, create.code,
            "case {case}: expected the {} bytes of code the publish copied, got {} bytes",
            create.code.len(),
            code.body.len()
        );
        assert_eq!(
            code.etag, live_etag,
            "case {case}: expected the LIVE version token {live_etag:?} from the get, got {:?}",
            code.etag
        );

        // The stage the publish READ is untouched: still the unpublished stage, at the
        // version the create handed back, serving the code it stored.
        let development = describe_function(&router, &create.name, Some(STAGE_DEVELOPMENT)).await;
        assert_eq!(
            development.status, 200,
            "case {case}: expected HTTP 200 describing DEVELOPMENT, got {} with {}",
            development.status, development.raw
        );
        assert_summary(
            &development,
            &create,
            STAGE_DEVELOPMENT,
            STATUS_UNPUBLISHED,
            "the describe of DEVELOPMENT",
        );
        assert_eq!(
            development.header("etag"),
            created_etag,
            "case {case}: expected the create's version token from DEVELOPMENT, got {:?}",
            development.header("etag")
        );
    }
}

/// The `If-Match` header one precondition row sends.
enum Token {
    /// Built from the current version token of the stage the publish would copy.
    Current(fn(&str) -> String),
    /// A value that stands on its own, whatever version the store holds.
    Literal(String),
}

/// One `If-Match` header, and what the service is expected to answer it with: `None` to
/// publish, or the status and error code the refusal carries.
struct Precondition {
    case: &'static str,
    token: Token,
    refusal: Option<(u16, &'static str)>,
}

/// The copy only happens at the version its caller read. A header carrying the current
/// version token — bare, quoted as an HTTP entity tag, or quoted as a weak one — publishes;
/// anything that is no version token at all is refused as missing or not valid; a well-formed
/// token that belongs to another version is a failed precondition. A refused publish leaves
/// the function with no `LIVE` stage and its `DEVELOPMENT` version untouched.
#[tokio::test]
async fn a_publish_needs_the_current_version_of_the_stage_it_copies() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_precondition_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    // A version token this service really minted that belongs to another function, so the
    // stale case is a well-formed token rather than a made-up string.
    let foreign = create_one(&router, &renamed(&row, "foreign_version_source")).await;

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
            refusal: Some((412, "PreconditionFailed")),
        },
        Precondition {
            case: "the version token of another function",
            token: Token::Literal(foreign.clone()),
            refusal: Some((412, "PreconditionFailed")),
        },
    ];

    for (index, precondition) in cases.iter().enumerate() {
        let case = precondition.case;
        // One function per row, so what a row publishes cannot change what the next one
        // reads.
        let create = renamed(&row, &format!("precondition_probe_{index}"));
        let created_etag = create_one(&router, &create).await;
        let sent = match &precondition.token {
            Token::Current(render) => render(&created_etag),
            Token::Literal(literal) => literal.clone(),
        };

        let answer = publish_function(&router, &create.name, &sent).await;
        let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;

        match precondition.refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected HTTP 200 publishing with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                let stage = member(answer.root(), "FunctionMetadata/Stage", case, &answer.raw);
                assert_eq!(
                    stage, STAGE_LIVE,
                    "case {case}: expected the published stage, got {stage:?}"
                );
                assert_eq!(
                    live.status, 200,
                    "case {case}: expected the LIVE stage to be readable, got {} with {}",
                    live.status, live.raw
                );
            }
            Some((status, code)) => {
                assert_eq!(
                    answer.status, status,
                    "case {case}: expected HTTP {status} publishing with {sent:?}, got {} with {}",
                    answer.status, answer.raw
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case}: expected {code}, got {}",
                    answer.raw
                );
                assert_eq!(
                    live.status, 404,
                    "case {case}: a refused publish must promote nothing to LIVE, got {} with {}",
                    live.status, live.raw
                );
                // The stage the publish would have copied is untouched.
                let development = development_etag(&router, &create.name).await;
                assert_eq!(
                    development, created_etag,
                    "case {case}: a refused publish must leave the DEVELOPMENT version alone, got \
                     {development:?}"
                );
            }
        }
    }
}

/// The version an update SUPERSEDED is a well-formed token that is no longer the current one,
/// so a publish holding it is a failed precondition that promotes nothing — and the publish
/// that carries the version the update handed back copies THAT revision, never the one the
/// refused call held.
#[tokio::test]
async fn the_version_an_update_superseded_no_longer_publishes() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_superseded_store").await;

    for (index, create) in create_table(&kvs_arn).into_iter().enumerate() {
        let case = create.case;
        let create = renamed(&create, &format!("superseded_probe_{index}"));
        let stale = create_one(&router, &create).await;

        // The update replaces what the create stored and mints a fresh version token, so the
        // token the create handed back is out of date from here on.
        let revised = revised(&create);
        let updated = update_function(&router, &create.name, &revised, Some(&stale)).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200 from the update, got {} with {}",
            updated.status, updated.raw
        );
        let current = development_etag(&router, &create.name).await;
        assert_ne!(
            current, stale,
            "case {case}: expected the update to mint a version token of its own"
        );

        let refused = publish_function(&router, &create.name, &stale).await;
        assert_eq!(
            refused.status, 412,
            "case {case}: expected HTTP 412 publishing the superseded version, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "PreconditionFailed",
            "case {case}: expected PreconditionFailed, got {}",
            refused.raw
        );
        let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;
        assert_eq!(
            live.status, 404,
            "case {case}: the refused publish must promote nothing to LIVE, got {} with {}",
            live.status, live.raw
        );

        // The publish that carries the current version promotes the revision the update
        // stored, which is what the LIVE stage then serves.
        let published = publish_function(&router, &create.name, &current).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected HTTP 200 publishing the current version, got {} with {}",
            published.status, published.raw
        );
        assert_summary(
            &published,
            &revised,
            STAGE_LIVE,
            STATUS_UNASSOCIATED,
            "the publish of the updated revision",
        );
        let code = get_function(&router, &create.name, Some(STAGE_LIVE)).await;
        assert_eq!(
            code.status, 200,
            "case {case}: expected HTTP 200 getting the LIVE code, got {}",
            code.status
        );
        assert_eq!(
            code.body, revised.code,
            "case {case}: expected the LIVE stage to serve the updated revision ({} bytes), got \
             {} bytes",
            revised.code.len(),
            code.body.len()
        );
    }
}

/// The operation addresses the function by "The name of the function that you are
/// publishing", so a name no function holds is "The function does not exist" (HTTP 404) and
/// publishes nothing — whatever version the header carries.
#[tokio::test]
async fn a_name_no_function_holds_is_not_found() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_unknown_name_store").await;
    let table = create_table(&kvs_arn);
    let create = renamed(
        table
            .first()
            .expect("the create table carries at least one row"),
        "present_function",
    );
    let created_etag = create_one(&router, &create).await;

    let unknown = [
        ("another admissible name", "absent_function".to_string()),
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
            let answer = publish_function(&router, &name, token).await;
            assert_eq!(
                answer.status, 404,
                "case {case} / {token_case}: expected HTTP 404 publishing {name:?}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchFunctionExists",
                "case {case} / {token_case}: expected NoSuchFunctionExists, got {}",
                answer.raw
            );
        }
    }

    // Nothing was published: the function that is there still has no LIVE stage, and its
    // DEVELOPMENT version stands.
    let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;
    assert_eq!(
        live.status, 404,
        "a refused publish must promote nothing to LIVE, got {} with {}",
        live.status, live.raw
    );
    let development = development_etag(&router, &create.name).await;
    assert_eq!(
        development, created_etag,
        "a refused publish must leave the DEVELOPMENT version alone, got {development:?}"
    );
}

/// Where a distribution attaches the function under test. A behaviour carries its functions
/// in a `FunctionAssociations` list, and a distribution's behaviours are its DEFAULT cache
/// behaviour together with its ordered ones, so both placements are exercised.
#[derive(Debug, Clone, Copy)]
enum Attachment {
    Default,
    Ordered,
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
    let (default_attachment, behaviors) = match attachment {
        Attachment::Default => (associations_xml(arn), "<Quantity>0</Quantity>".to_string()),
        // A behaviour that attaches nothing comes first, so the walk has to reach past it.
        Attachment::Ordered => (
            String::new(),
            format!(
                "<Quantity>2</Quantity><Items>\
                 <CacheBehavior><PathPattern>/static/*</PathPattern>\
                 <TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></CacheBehavior>\
                 <CacheBehavior><PathPattern>/api/*</PathPattern>\
                 <TargetOriginId>primary</TargetOriginId>\
                 <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{}</CacheBehavior>\
                 </Items>",
                associations_xml(arn)
            ),
        ),
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
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// The status a published stage reports is computed from the stored distributions: a function
/// a cache behaviour attaches by ARN — the default behaviour's or an ordered one's — is
/// published as associated, one nothing attaches as unassociated, and each publish mints a
/// version token of its own for the stage it writes.
#[tokio::test]
async fn the_published_status_reports_whether_a_behaviour_attaches_the_function() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_association_store").await;
    let table = create_table(&kvs_arn);
    let row = table
        .first()
        .cloned()
        .expect("the create table carries at least one row");

    // The function nothing ever attaches: the control for every row below.
    let control = renamed(&row, "function_nothing_attaches");
    let control_etag = create_one(&router, &control).await;

    for (index, attachment) in [Attachment::Default, Attachment::Ordered]
        .into_iter()
        .enumerate()
    {
        let case = format!("{attachment:?}");
        let create = renamed(&row, &format!("function_a_behaviour_attaches_{index}"));
        let created_etag = create_one(&router, &create).await;

        // While nothing attaches it, a publish reports the published-but-unassociated status.
        let first = publish_function(&router, &create.name, &created_etag).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected HTTP 200 from the first publish, got {} with {}",
            first.status, first.raw
        );
        let first_status = member(first.root(), "Status", &case, &first.raw);
        assert_eq!(
            first_status, STATUS_UNASSOCIATED,
            "case {case}: expected Status {STATUS_UNASSOCIATED} while nothing attaches the \
             function, got {first_status:?}"
        );
        let arn = reported_arn(&first, &case);
        let first_live_etag = describe_function(&router, &create.name, Some(STAGE_LIVE))
            .await
            .header("etag")
            .to_string();

        // A distribution whose cache behaviour attaches the function by that ARN.
        let distribution = create_distribution(
            &router,
            &distribution_xml(&format!("publish-association-{index}"), &arn, attachment),
        )
        .await;
        assert_eq!(
            distribution.status, 201,
            "case {case}: expected HTTP 201 creating the distribution, got {} with {}",
            distribution.status, distribution.raw
        );
        let stored_attachment = distribution
            .root()
            .descendant("FunctionAssociation")
            .and_then(|association| association.text_at("FunctionARN"))
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            stored_attachment, arn,
            "case {case}: expected the distribution to carry the submitted attachment, got \
             {stored_attachment:?}"
        );

        // So the next publish of that function reports the association, under a fresh version
        // token.
        let again = publish_function(&router, &create.name, &created_etag).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected HTTP 200 from the second publish, got {} with {}",
            again.status, again.raw
        );
        let again_status = member(again.root(), "Status", &case, &again.raw);
        assert_eq!(
            again_status, STATUS_ASSOCIATED,
            "case {case}: expected Status {STATUS_ASSOCIATED} once a behaviour attaches the \
             function, got {again_status:?}"
        );
        let live = describe_function(&router, &create.name, Some(STAGE_LIVE)).await;
        let live_status = member(live.root(), "Status", &case, &live.raw);
        assert_eq!(
            live_status, STATUS_ASSOCIATED,
            "case {case}: expected the describe to report Status {STATUS_ASSOCIATED}, got \
             {live_status:?}"
        );
        let live_etag = live.header("etag").to_string();
        assert_ne!(
            live_etag, first_live_etag,
            "case {case}: expected the second publish to mint a version token of its own"
        );
    }

    // The function no behaviour attaches is unaffected by any of those distributions.
    let answer = publish_function(&router, &control.name, &control_etag).await;
    assert_eq!(
        answer.status, 200,
        "case the function nothing attaches: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    let status = member(
        answer.root(),
        "Status",
        "the function nothing attaches",
        &answer.raw,
    );
    assert_eq!(
        status, STATUS_UNASSOCIATED,
        "case the function nothing attaches: expected Status {STATUS_UNASSOCIATED}, got {status:?}"
    );
}
