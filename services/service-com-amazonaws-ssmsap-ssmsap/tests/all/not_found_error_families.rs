// SPDX-License-Identifier: CC0-1.0

//! How this service words a reference it cannot resolve, and which error CLASS it words
//! it on, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! error serialization all run exactly as they do in the emulator binary. Nothing reaches
//! into `crate::state` directly: the references the rows resolve are the ones a
//! `RegisterApplication` reply handed back, and the ones they fail to resolve are
//! references no call of the suite ever created.
//!
//! The service deliberately words the SAME logical "that does not exist" condition
//! differently per call, so each wording — and the error class it arrives on — is the only
//! signal a client has. Three rules are pinned here, one per family:
//!
//! 1. a reference to an operation RECORD that resolves to nothing is the
//!    `ValidationException` those calls model (HTTP 400), stating `Operation Not Found: `
//!    and the `OperationId` the request carried;
//! 2. a `RegisterApplication` whose `ApplicationId` is already registered here is the
//!    `ConflictException` it models (HTTP 409), stating the id in single quotes and that
//!    it already exists;
//! 3. an ARN-addressed TAGGING call whose `ResourceArn` names no resource of this account
//!    and Region is refused on the not-found class those routes carry
//!    (`scaffolded::NOT_FOUND_CODE`, HTTP 404) with one fixed sentence that reports
//!    nothing of the request.
//!
//! The tables span what each rule generalises over: for (1) the forms the modelled
//! `OperationId` pattern admits and both calls that take one; for (2) the forms and
//! lengths the modelled `ApplicationId` admits; for (3) all three tagging routes crossed
//! with the ways an ARN can name nothing (never registered, deregistered, a database the
//! registration never named, another Region, another service's resource on either branch
//! of the modelled ARN pattern).

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::scaffolded::NOT_FOUND_CODE;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    duplicate_application_id_message, no_operation_message, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::resource_tags::{
    application_arn, database_arn, list_tags_in_region, register, registration,
};
use crate::tag_resource::{pairs, tag_in_region};
use crate::untag_resource::untag_in_region;
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a resource can be addressed at a Region it was not
/// registered in.
const OTHER_REGION: &str = "us-west-2";

/// The modelled error code and status a reference to an operation record that resolves to
/// nothing is answered with (`smithy_query(query="operation",
/// operation="GetOperation,ListOperationEvents")`: `ValidationException` `@httpError 400`
/// — neither call models a 404).
const OPERATION_NOT_FOUND_STATUS: u16 = 400;

/// The modelled error code and status a duplicate `ApplicationId` is answered with
/// (`smithy_query(query="operation", operation="RegisterApplication")`:
/// `ConflictException` `@httpError 409`).
const CONFLICT_EXCEPTION: &str = "ConflictException";
const CONFLICT_STATUS: u16 = 409;

/// The status the tagging routes' own not-found class carries.
const NOT_FOUND_STATUS: u16 = 404;

/// `OperationId` values that satisfy the modelled pattern
/// (`smithy_query(query="operation", operation="GetOperation")`:
/// `^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`) and name no record this
/// suite ever filed. The rows span what that pattern generalises over: hex digit case, the
/// optional braces, and digits-only versus letters-only groups.
const UNFILED_OPERATION_IDS: &[&str] = &[
    "ef3dabaa-b0d6-445e-ae78-e9a29f8d81f4",
    "EF3DABAA-B0D6-445E-AE78-E9A29F8D81F4",
    "{ef3dabaa-b0d6-445e-ae78-e9a29f8d81f4}",
    "00000000-0000-0000-0000-000000000000",
    "abcdefab-abcd-abcd-abcd-abcdefabcdef",
];

/// The calls that take an `OperationId` and resolve it against the operation records of
/// this account and Region (`smithy_query(query="digest")`: `GetOperation` and
/// `ListOperationEvents` are the two).
const OPERATION_RECORD_CALLS: &[&str] = &["/get-operation", "/list-operation-events"];

/// `ApplicationId` values the model admits (`smithy_query(query="operation",
/// operation="RegisterApplication")`: `^[\w\d\.-]+$`, length 1..60). The rows span the
/// character class (letters in both cases, digits, `.`, `-`, `_`) and both length bounds.
fn application_ids() -> Vec<String> {
    vec![
        "a".to_string(),
        "HDB-TEST".to_string(),
        "dup.app-1".to_string(),
        "under_scored_9".to_string(),
        "x".repeat(60),
    ]
}

/// One tagging route: which of the three operations the model binds to
/// `/tags/{resourceArn}` a case calls (`smithy_query(query="operation",
/// operation="ListTagsForResource,TagResource,UntagResource")`: `GET` lists, `POST` tags,
/// `DELETE` untags).
#[derive(Debug, Clone, Copy)]
enum Tagging {
    List,
    Tag,
    Untag,
}

impl Tagging {
    /// Every route the rule covers.
    const ALL: &'static [Self] = &[Self::List, Self::Tag, Self::Untag];

    /// How an assertion names the operation.
    fn operation(self) -> &'static str {
        match self {
            Self::List => "ListTagsForResource",
            Self::Tag => "TagResource",
            Self::Untag => "UntagResource",
        }
    }

    /// Call this route for `arn` at `region`'s endpoint, each through the harness the
    /// route's own suite drives rather than a second spelling of its binding.
    async fn call(self, emulator: &Emulator, region: &str, arn: &str) -> Reply {
        match self {
            Self::List => list_tags_in_region(emulator, region, arn).await,
            Self::Tag => tag_in_region(emulator, region, arn, &pairs(&[("env", "dev")])).await,
            Self::Untag => untag_in_region(emulator, region, arn, &["env".to_string()]).await,
        }
    }
}

/// RULE: a reference to an operation record that resolves to nothing is refused with the
/// `ValidationException` those calls model (HTTP 400), stating `Operation Not Found: `
/// and the `OperationId` the request carried — the same wording from either call that
/// takes one.
#[tokio::test]
async fn an_operation_id_naming_no_record_is_refused_in_the_services_own_wording() {
    let emulator = Emulator::new();

    // A record that DOES resolve, so the refusals below are about the reference and not
    // about the route: the id a registration handed back.
    let filed = emulator
        .send("/register-application", valid_registration("op-filed", "HANA"))
        .await;
    assert_eq!(
        filed.status, 200,
        "expected 200 from RegisterApplication, got {filed:?}"
    );
    let filed_id = filed
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !filed_id.is_empty(),
        "expected RegisterApplication to report an OperationId, got {filed:?}"
    );

    for path in OPERATION_RECORD_CALLS {
        let served = emulator
            .send(path, json!({ "OperationId": filed_id.clone() }))
            .await;
        assert_eq!(
            served.status, 200,
            "case {path:?}/{filed_id:?}: the id a registration filed resolves, got {served:?}"
        );

        for id in UNFILED_OPERATION_IDS {
            let reply = emulator.send(path, json!({ "OperationId": id })).await;
            assert_eq!(
                reply.status, OPERATION_NOT_FOUND_STATUS,
                "case {path:?}/{id:?}: expected HTTP {OPERATION_NOT_FOUND_STATUS}, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                VALIDATION_EXCEPTION,
                "case {path:?}/{id:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
                reply.code()
            );
            let stated = message(&reply);
            let expected = no_operation_message(id);
            assert_eq!(
                stated, expected,
                "case {path:?}/{id:?}: expected {expected:?}, got {stated:?}"
            );
            // The id is the only variable part: the fixed text is what a client matches
            // this condition on, and the request's own id is what it reads out of it.
            assert!(
                stated.starts_with("Operation Not Found: ") && stated.ends_with(id),
                "case {path:?}/{id:?}: expected the fixed text around the request's own \
                 id, got {stated:?}"
            );
        }
    }
}

/// RULE: a `RegisterApplication` naming an `ApplicationId` already registered in this
/// account and Region is refused with the `ConflictException` it models (HTTP 409),
/// stating that id in single quotes and that it already exists.
#[tokio::test]
async fn a_duplicate_application_id_conflicts_in_the_services_own_wording() {
    let emulator = Emulator::new();

    for id in application_ids() {
        register(&emulator, &id, registration(&id, &[], None)).await;

        // The second call differs from the first in a member the service would otherwise
        // accept, so the conflict is about the ID and nothing else.
        let mut again = valid_registration(&id, "HANA");
        again
            .as_object_mut()
            .expect("a registration body is a JSON object")
            .insert("Instances".to_string(), json!(["i-0deadbeefdeadbeef"]));
        let reply = emulator.send("/register-application", again).await;

        assert_eq!(
            reply.status, CONFLICT_STATUS,
            "case {id:?}: expected HTTP {CONFLICT_STATUS}, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            CONFLICT_EXCEPTION,
            "case {id:?}: expected {CONFLICT_EXCEPTION}, got {:?}",
            reply.code()
        );
        let stated = message(&reply);
        let expected = duplicate_application_id_message(&id);
        assert_eq!(
            stated, expected,
            "case {id:?}: expected {expected:?}, got {stated:?}"
        );
        // The id is the only variable part, and it is quoted: a client telling a
        // duplicate id apart from this operation's other conflicts reads the fixed text.
        assert!(
            stated.contains(&format!("'{id}'")),
            "case {id:?}: expected the id quoted in {stated:?}"
        );
    }
}

/// RULE: every ARN-addressed tagging route refuses a `ResourceArn` that names no resource
/// of this account and Region on its own not-found class (HTTP 404) with ONE fixed
/// sentence, which reports nothing of the request — not the ARN, not the kind of resource
/// it named.
#[tokio::test]
async fn the_tagging_routes_refuse_an_unresolvable_arn_on_their_own_not_found_class() {
    let emulator = Emulator::new();

    register(
        &emulator,
        "a registered application",
        registration("nf-present", &[], Some(&["HDB"])),
    )
    .await;
    register(
        &emulator,
        "an application to deregister",
        registration("nf-gone", &[], None),
    )
    .await;
    let reply = emulator
        .send(
            "/deregister-application",
            json!({ "ApplicationId": "nf-gone" }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Deregistration is asynchronous: the application stops resolving when the removal
    // the clock owes it falls due.
    settle().await;

    // (label, the region the call is made at, the ARN it names): the ways an ARN can name
    // nothing, including both branches of the modelled `SsmSapArn` pattern
    // (`^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$`).
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an application never registered",
            REGION,
            application_arn(REGION, "nf-never-registered"),
        ),
        (
            "an application that was deregistered",
            REGION,
            application_arn(REGION, "nf-gone"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, "nf-present", "NOPE"),
        ),
        (
            "a registered application addressed at another regional endpoint",
            OTHER_REGION,
            application_arn(REGION, "nf-present"),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            application_arn(OTHER_REGION, "nf-present"),
        ),
        (
            "another service's resource, on the path branch of the ARN pattern",
            REGION,
            "arn:aws:s3:::a-bucket/an-object".to_string(),
        ),
        (
            "another service's resource, on the colon branch of the ARN pattern",
            REGION,
            format!("arn:aws:sqs:{REGION}:{}:a-queue", engine::EMULATOR_ACCOUNT_ID),
        ),
    ];

    for route in Tagging::ALL {
        for (label, region, arn) in &rows {
            let case = format!("{}/{label}", route.operation());
            let reply = route.call(&emulator, region, arn).await;

            assert_eq!(
                reply.status, NOT_FOUND_STATUS,
                "case {case:?}: expected HTTP {NOT_FOUND_STATUS}, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                NOT_FOUND_CODE,
                "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
                reply.code()
            );
            assert_eq!(
                reply.error_type_header.as_deref(),
                Some(NOT_FOUND_CODE),
                "case {case:?}: expected the {NOT_FOUND_CODE} error type header, got {reply:?}"
            );
            let stated = message(&reply);
            assert_eq!(
                stated, RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {RESOURCE_NOT_FOUND_MESSAGE:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(arn.as_str()),
                "case {case:?}: the sentence is fixed, so it reports no part of {arn:?}, \
                 got {stated:?}"
            );
        }

        // The route itself serves an ARN that DOES resolve, so the refusals above are
        // about what the ARN names rather than about the call.
        let present = application_arn(REGION, "nf-present");
        let served = route.call(&emulator, REGION, &present).await;
        assert_eq!(
            served.status, 200,
            "case {}/\"a registered application\": expected 200, got {served:?}",
            route.operation()
        );
    }
}
