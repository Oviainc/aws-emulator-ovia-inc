// SPDX-License-Identifier: CC0-1.0

//! The TEXT of the health-check not-found refusal, end to end over the router.
//!
//! The rule under test: an operation that addresses a health check by id and
//! finds none refuses the request with `NoSuchHealthCheck` (HTTP 404) whose
//! `Message` is built from the REQUEST — the fixed sentence
//! `A health check with id <id> does not exist.`, with the id the request sent
//! as its single variable part, whatever that id looks like and whichever
//! operation was asked. The error shape's own documentation states the rule the
//! refusal reports, not the sentence it is reported in, so a message restating
//! that documentation is not the refusal a client reads.
//!
//! The rule is about how the message is BUILT, so it is asserted over a table of
//! ids (crossed with every operation that refuses one) rather than over the one
//! request that exposed it: the rows differ along the FORM of the id — a UUID of
//! the shape a create mints, one character, the modelled maximum length, a
//! well-formed id of another entity kind, the caller reference of a health check
//! that does exist, an existing id in another case, and an id carrying XML
//! metacharacters, which the envelope has to encode rather than write raw. A
//! message built from anything but the request's own id fails such a table.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from the
//! suites that own those operations, so these tests drive the service through the
//! same router a socket-served request goes through.

use smithy_server_runtime::MultiServiceRouter;

use crate::health_checks::{create_health_check, get_health_check, health_check_count, health_check_id};
use crate::hosted_zone_records::{call, change_batch_body, create_zone_ok, router};
use crate::refusal_message_rules::message_of;

/// The error code every refusal in this suite carries.
const NO_SUCH_HEALTH_CHECK: &str = "NoSuchHealthCheck";

/// The form this service words a health-check id that names nothing in, with the
/// request's own id filled in.
///
/// Written out here rather than read from the crate under test, so the sentence
/// is asserted against the rule rather than against whatever the code happens to
/// build.
fn missing_health_check_message(requested_id: &str) -> String {
    format!("A health check with id {requested_id} does not exist.")
}

/// One operation that addresses a health check by id, as a request.
struct OperationRow {
    operation: &'static str,
    method: &'static str,
    /// The request URI, with `{id}` standing for the health-check id as the
    /// request spells it.
    uri: &'static str,
    /// The request body, empty for the operations that carry none.
    body: &'static str,
}

/// Every implemented operation whose request names a health check by id, with a
/// body that is otherwise valid — so the missing health check is the only thing
/// wrong with the request.
const OPERATIONS: &[OperationRow] = &[
    OperationRow {
        operation: "GetHealthCheck",
        method: "GET",
        uri: "/2013-04-01/healthcheck/{id}",
        body: "",
    },
    OperationRow {
        operation: "GetHealthCheckStatus",
        method: "GET",
        uri: "/2013-04-01/healthcheck/{id}/status",
        body: "",
    },
    OperationRow {
        operation: "GetHealthCheckLastFailureReason",
        method: "GET",
        uri: "/2013-04-01/healthcheck/{id}/lastfailurereason",
        body: "",
    },
    OperationRow {
        operation: "UpdateHealthCheck",
        method: "POST",
        uri: "/2013-04-01/healthcheck/{id}",
        body: "<UpdateHealthCheckRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
               <Disabled>true</Disabled></UpdateHealthCheckRequest>",
    },
    OperationRow {
        operation: "ListTagsForResource",
        method: "GET",
        uri: "/2013-04-01/tags/healthcheck/{id}",
        body: "",
    },
    OperationRow {
        operation: "ChangeTagsForResource",
        method: "POST",
        uri: "/2013-04-01/tags/healthcheck/{id}",
        body: "<ChangeTagsForResourceRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
               <AddTags><Tag><Key>Owner</Key><Value>dns-team</Value></Tag></AddTags>\
               </ChangeTagsForResourceRequest>",
    },
];

/// One id that names no health check of the account.
struct IdRow {
    case: &'static str,
    /// The `{HealthCheckId}` label as the request writes it (percent-encoded
    /// where the id carries characters a URI reserves).
    sent: String,
    /// The id the service receives, which the message must name verbatim.
    received: String,
    /// Whether the id carries characters the XML envelope has to encode.
    needs_encoding: bool,
}

impl IdRow {
    /// A row whose id travels as it is written.
    fn plain(case: &'static str, id: &str) -> Self {
        Self {
            case,
            sent: id.to_string(),
            received: id.to_string(),
            needs_encoding: false,
        }
    }
}

/// The caller reference the health check that DOES exist is created with — one
/// row asks about it, because a caller reference is not an id.
const EXISTING_REFERENCE: &str = "not-found-message-existing";

/// Create the one health check these tests leave in place, and return its id.
async fn existing_health_check(router: &MultiServiceRouter) -> String {
    let created = create_health_check(
        router,
        EXISTING_REFERENCE,
        "<Type>HTTP</Type><IPAddress>192.0.2.70</IPAddress><Port>80</Port>",
    )
    .await;
    health_check_id(&created, EXISTING_REFERENCE)
}

#[tokio::test]
async fn every_operation_reports_a_missing_health_check_as_the_id_it_was_asked_about() {
    let router = router();
    let known = existing_health_check(&router).await;

    // `HealthCheckId` is modelled `@length(min: 0, max: 64)` with no pattern
    // (smithy_query(query="operation",operation="GetHealthCheckStatus")), so an
    // id of one character and an id of sixty-four are both ids a request may
    // carry — and neither names a health check here.
    let longest = "0123456789abcdef".repeat(4);
    let rows = [
        IdRow::plain(
            "a UUID of the shape a create mints, that no create minted",
            "7c9e6679-7425-40de-944b-e07fc1f90ae7",
        ),
        IdRow::plain("a single character", "x"),
        IdRow::plain("an id at the modelled maximum length", &longest),
        IdRow::plain("a hosted-zone id, which names another kind of resource", "Z1D633PJN98FT9"),
        IdRow::plain(
            "the caller reference of the health check that does exist",
            EXISTING_REFERENCE,
        ),
        IdRow::plain("an existing id in upper case", &known.to_uppercase()),
        IdRow {
            case: "an id carrying XML metacharacters",
            sent: "a%3Cb%26c%3Ed".to_string(),
            received: "a<b&c>d".to_string(),
            needs_encoding: true,
        },
    ];

    for row in &rows {
        if row.received == known {
            // An id that happens to name the health check this store holds is
            // no longer a missing one.
            continue;
        }
        let expected = missing_health_check_message(&row.received);
        for operation in OPERATIONS {
            let case = format!("{} asked about {}", operation.operation, row.case);
            let uri = operation.uri.replace("{id}", &row.sent);
            let reply = call(&router, operation.method, &uri, operation.body).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404 (501 = still scaffolded), got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                NO_SUCH_HEALTH_CHECK,
                "case {case:?}: expected {NO_SUCH_HEALTH_CHECK}, got {:?}; body {}",
                reply.error_code(),
                reply.body
            );
            let got = message_of(&reply, &case);
            assert_eq!(
                got, expected,
                "case {case:?}: expected message {expected:?}, got {got:?}"
            );
            if row.needs_encoding {
                // A caller-influenced value is output-encoded into the envelope
                // rather than written into it raw, so the body stays
                // well-formed XML whatever the id carried.
                assert!(
                    !reply.body.contains(&row.received),
                    "case {case:?}: the id must be encoded into the envelope, got body {}",
                    reply.body
                );
            }
        }
    }

    // The refusals changed nothing: the health check the store does hold is
    // still readable, and the population is the one create's.
    let reply = get_health_check(&router, &known).await;
    assert_eq!(
        reply.status, 200,
        "the health check the store holds must still be readable, got {} with body {}",
        reply.status, reply.body
    );
    let count = health_check_count(&router, "after every refusal").await;
    assert_eq!(
        count, 1,
        "the refusals must neither create nor destroy a health check, got HealthCheckCount {count}"
    );
}

#[tokio::test]
async fn a_record_set_naming_a_missing_health_check_reports_the_same_sentence() {
    // The wording belongs to the FACT (no health check of this account has that
    // id), not to the operation that ran into it: a change batch that references
    // a health check reports a missing one in the same sentence the readers do.
    // The rows differ along the id form, as above.
    let router = router();
    let zone = create_zone_ok(&router, "health-check-reference.example.com", "msg-rrset").await;

    for id in [
        "3f0c1d2e-4b5a-6978-8a9b-0c1d2e3f4a5b",
        "x",
        "Z1D633PJN98FT9",
    ] {
        let case = format!("a record set naming the health check {id:?}");
        let change = format!(
            "<Change><Action>CREATE</Action><ResourceRecordSet>\
             <Name>probe.health-check-reference.example.com</Name><Type>A</Type><TTL>60</TTL>\
             <ResourceRecords><ResourceRecord><Value>192.0.2.10</Value></ResourceRecord>\
             </ResourceRecords><HealthCheckId>{id}</HealthCheckId>\
             </ResourceRecordSet></Change>"
        );
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone}/rrset"),
            &change_batch_body(&change, None),
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            NO_SUCH_HEALTH_CHECK,
            "case {case:?}: expected {NO_SUCH_HEALTH_CHECK}, got {:?}; body {}",
            reply.error_code(),
            reply.body
        );
        let got = message_of(&reply, &case);
        let expected = missing_health_check_message(id);
        assert_eq!(
            got, expected,
            "case {case:?}: expected message {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn an_id_past_the_modelled_length_bound_is_refused_before_the_lookup() {
    // One past the published bound: `HealthCheckId` is `@length(max: 64)`, so a
    // 65-character id is outside the model's own constraint and is refused as a
    // bad request rather than reported as a health check that does not exist.
    let router = router();
    let too_long = "0123456789abcdef".repeat(4) + "0";
    assert_eq!(
        too_long.len(),
        65,
        "the row must be one character past the modelled maximum, got {}",
        too_long.len()
    );

    for operation in OPERATIONS {
        let case = format!("{} asked about an over-long id", operation.operation);
        let uri = operation.uri.replace("{id}", &too_long);
        let reply = call(&router, operation.method, &uri, operation.body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 from the modelled length bound, got {} with body {}",
            reply.status, reply.body
        );
        assert_ne!(
            reply.error_code(),
            NO_SUCH_HEALTH_CHECK,
            "case {case:?}: an id the model refuses must not be reported as a missing health \
             check; body {}",
            reply.body
        );
    }
}
