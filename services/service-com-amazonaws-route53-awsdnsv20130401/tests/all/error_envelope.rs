// SPDX-License-Identifier: CC0-1.0

//! The error BODY every Route 53 refusal answers with, end to end over the router.
//!
//! Four rules are pinned here, each over a TABLE that spans the class it generalises
//! over rather than the one request that exposed it:
//!
//! * the `<ErrorResponse>` envelope names the request it answers — a `RequestId`
//!   element beside `<Error>`, on EVERY failure this service answers, whatever the
//!   status, the operation or the layer that refused it;
//! * a zone id that names no zone is reported in AWS's request-derived form,
//!   `No hosted zone found with ID: <the requested id>`, by every zone-addressed
//!   operation;
//! * the two refusals AWS words as CONSTANTS — a repeated `CallerReference`
//!   (`HostedZoneAlreadyExists`) and a zone that still holds records of the caller's
//!   (`HostedZoneNotEmpty`) — carry that constant text, naming no server-generated id
//!   and echoing no request value;
//! * `InvalidChangeBatch` names every refused change, one entry per change in the
//!   modelled `messages` list, each naming the record set by `name=`/`type=`, with the
//!   flat `message` the list rendered as a list.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from
//! `hosted_zone_records`, so these suites drive the service through the same router a
//! socket-served request goes through.

use super::hosted_zone_records::{
    call, change_batch_body, change_element, create_zone_body, create_zone_ok, router, RecordRow,
};

/// The refusal text `CreateHostedZone` answers a repeated `CallerReference` with, as
/// AWS words it: a constant that names neither the reference nor the zone it already
/// names (`crate::state::HOSTED_ZONE_ALREADY_EXISTS_MESSAGE`).
const ALREADY_EXISTS: &str = "A hosted zone has already been created with the specified caller \
                              reference.";

/// The refusal text `DeleteHostedZone` answers a zone that still holds the caller's
/// records with, as AWS words it: a constant naming no zone, ending in what the caller
/// has to do next (`crate::state::HOSTED_ZONE_NOT_EMPTY_MESSAGE`).
const NOT_EMPTY: &str = "The specified hosted zone contains resource record sets other than the \
                         default SOA and NS records and so cannot be deleted. Delete all \
                         non-default records and try again.";

/// A record set a zone starts WITHOUT, so creating it makes the zone non-empty and
/// creating it twice is refused.
const USER_RECORD: RecordRow = RecordRow {
    name: "www.example.com",
    rr_type: "A",
    ttl: 300,
    values: &["192.0.2.1"],
};

/// A valid single-change batch body, for a request whose refusal has nothing to do with
/// the batch itself (an unknown zone, say).
fn one_change(action: &str, row: &RecordRow) -> String {
    change_batch_body(&change_element(action, row), None)
}

/// The text of every `<RequestId>` element in a document, in document order.
///
/// Read off the raw body rather than through the flattened paths so the ELEMENT's
/// position in the envelope can be asserted beside its presence.
fn request_ids(body: &str) -> Vec<&str> {
    let mut ids = Vec::new();
    let mut rest = body;
    while let Some((_, after_open)) = rest.split_once("<RequestId>") {
        match after_open.split_once("</RequestId>") {
            Some((id, tail)) => {
                ids.push(id);
                rest = tail;
            }
            None => break,
        }
    }
    ids
}

/// One zone-addressed operation: its name, its HTTP method, the URI it addresses a
/// zone id at, and the body it sends.
type ZoneOperation = (&'static str, &'static str, fn(&str) -> String, fn() -> String);

/// One failure to drive: how to ask for it, and what it must be refused with.
struct Failure {
    /// What this row is, named in every assertion it drives.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: String,
    /// The status the refusal carries, or `None` to assert only that it IS a refusal —
    /// for the two rows refused by a layer in FRONT of the handlers, whose status is the
    /// framework's to choose.
    status: Option<u16>,
    /// The wire error code, or `""` where the refusing layer chooses it.
    code: &'static str,
}

#[tokio::test]
async fn every_error_envelope_names_the_request() {
    // The rule: a Route 53 error document is
    // `<ErrorResponse><Error>…</Error><RequestId>…</RequestId></ErrorResponse>` — the
    // request id is a property of the envelope, so EVERY failure carries exactly one,
    // beside `<Error>`, and two failures carry two different ids. The rows span the
    // statuses (400, 404, 409), the operations, and the three layers that can refuse a
    // request: a handler, the request boundary, and routing.
    let router = router();
    // The zone whose `CallerReference` the duplicate row below reuses.
    create_zone_ok(&router, "example.com", "ref-envelope-clean").await;
    let blocked = create_zone_ok(&router, "blocked.example.com", "ref-envelope-blocked").await;
    let with_record = call(
        &router,
        "POST",
        &format!("/2013-04-01/hostedzone/{blocked}/rrset"),
        &one_change(
            "CREATE",
            &RecordRow {
                name: "www.blocked.example.com",
                ..USER_RECORD
            },
        ),
    )
    .await;
    assert_eq!(
        with_record.status, 200,
        "the blocking record must be created, got {} with body {}",
        with_record.status, with_record.body
    );

    let unknown = "Z00000000000000";
    let failures = [
        Failure {
            case: "GetHostedZone on an unknown zone",
            method: "GET",
            uri: format!("/2013-04-01/hostedzone/{unknown}"),
            body: String::new(),
            status: Some(404),
            code: "NoSuchHostedZone",
        },
        Failure {
            case: "DeleteHostedZone on an unknown zone",
            method: "DELETE",
            uri: format!("/2013-04-01/hostedzone/{unknown}"),
            body: String::new(),
            status: Some(404),
            code: "NoSuchHostedZone",
        },
        Failure {
            case: "ChangeResourceRecordSets on an unknown zone",
            method: "POST",
            uri: format!("/2013-04-01/hostedzone/{unknown}/rrset"),
            body: one_change("CREATE", &USER_RECORD),
            status: Some(404),
            code: "NoSuchHostedZone",
        },
        Failure {
            case: "ListTagsForResource on an unknown zone",
            method: "GET",
            uri: format!("/2013-04-01/tags/hostedzone/{unknown}"),
            body: String::new(),
            status: Some(404),
            code: "NoSuchHostedZone",
        },
        Failure {
            case: "CreateHostedZone with a CallerReference already used",
            method: "POST",
            uri: "/2013-04-01/hostedzone".to_string(),
            body: create_zone_body("other.example.com", "ref-envelope-clean", ""),
            status: Some(409),
            code: "HostedZoneAlreadyExists",
        },
        Failure {
            case: "CreateHostedZone for a top-level domain",
            method: "POST",
            uri: "/2013-04-01/hostedzone".to_string(),
            body: create_zone_body("com", "ref-envelope-tld", ""),
            status: Some(400),
            code: "InvalidDomainName",
        },
        Failure {
            case: "DeleteHostedZone on a zone that still holds a record",
            method: "DELETE",
            uri: format!("/2013-04-01/hostedzone/{blocked}"),
            body: String::new(),
            status: Some(400),
            code: "HostedZoneNotEmpty",
        },
        Failure {
            case: "ChangeResourceRecordSets creating a record set that exists",
            method: "POST",
            uri: format!("/2013-04-01/hostedzone/{blocked}/rrset"),
            body: one_change(
                "CREATE",
                &RecordRow {
                    name: "www.blocked.example.com",
                    ..USER_RECORD
                },
            ),
            status: Some(400),
            code: "InvalidChangeBatch",
        },
        Failure {
            case: "CreateHostedZone with no CallerReference at all",
            method: "POST",
            uri: "/2013-04-01/hostedzone".to_string(),
            body: "<CreateHostedZoneRequest \
                   xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                   <Name>boundary.example.com</Name></CreateHostedZoneRequest>"
                .to_string(),
            status: None,
            code: "",
        },
        Failure {
            case: "a path naming no modelled operation",
            method: "GET",
            uri: "/2013-04-01/not-an-operation".to_string(),
            body: String::new(),
            status: None,
            code: "",
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for failure in &failures {
        let reply = call(&router, failure.method, &failure.uri, &failure.body).await;
        match failure.status {
            Some(status) => assert_eq!(
                reply.status, status,
                "case {:?}: expected {status}, got {} with body {}",
                failure.case, reply.status, reply.body
            ),
            None => assert!(
                reply.status >= 400,
                "case {:?}: expected a refusal, got {} with body {}",
                failure.case,
                reply.status,
                reply.body
            ),
        }
        if !failure.code.is_empty() {
            assert_eq!(
                reply.error_code(),
                failure.code,
                "case {:?}: expected {}, got body {}",
                failure.case,
                failure.code,
                reply.body
            );
        }

        let ids = request_ids(&reply.body);
        assert_eq!(
            ids.len(),
            1,
            "case {:?}: expected exactly one RequestId element, got {ids:?} in {}",
            failure.case,
            reply.body
        );
        let id = ids[0];
        assert!(
            !id.is_empty(),
            "case {:?}: the RequestId must name the request, got {}",
            failure.case,
            reply.body
        );
        assert!(
            reply
                .body
                .contains(&format!("</Error><RequestId>{id}</RequestId></ErrorResponse>")),
            "case {:?}: expected RequestId as the envelope's last child, beside Error, got {}",
            failure.case,
            reply.body
        );
        assert!(
            !seen.contains(&id.to_string()),
            "case {:?}: every response must name its OWN request, but id {id} was already \
             answered; seen {seen:?}",
            failure.case
        );
        seen.push(id.to_string());
    }
}

#[tokio::test]
async fn a_missing_zone_is_reported_in_awss_request_derived_form() {
    // The rule: a zone id that names no zone is refused `NoSuchHostedZone` (404) with
    // the message built FROM the request — `No hosted zone found with ID: <the id the
    // request asked about>` — by every zone-addressed operation. The rows cross the ids
    // (never-minted ones of three different lengths, and one that named a zone until it
    // was deleted) with the zone-addressed operations.
    let router = router();
    let deleted = create_zone_ok(&router, "gone.example.com", "ref-gone").await;
    assert_eq!(
        call(
            &router,
            "DELETE",
            &format!("/2013-04-01/hostedzone/{deleted}"),
            "",
        )
        .await
        .status,
        200,
        "the zone must delete so its id names no zone"
    );

    // `ResourceId` is `len 0..32`, so every id here is one the request can carry.
    let ids = [
        "Z00000000000000".to_string(),
        "Z00000000000000NOTREAL".to_string(),
        "ZABC123".to_string(),
        deleted.clone(),
    ];
    let operations: [ZoneOperation; 6] = [
        (
            "GetHostedZone",
            "GET",
            |id| format!("/2013-04-01/hostedzone/{id}"),
            String::new,
        ),
        (
            "DeleteHostedZone",
            "DELETE",
            |id| format!("/2013-04-01/hostedzone/{id}"),
            String::new,
        ),
        (
            "ListResourceRecordSets",
            "GET",
            |id| format!("/2013-04-01/hostedzone/{id}/rrset"),
            String::new,
        ),
        (
            "ChangeResourceRecordSets",
            "POST",
            |id| format!("/2013-04-01/hostedzone/{id}/rrset"),
            || one_change("CREATE", &USER_RECORD),
        ),
        (
            "ListTagsForResource",
            "GET",
            |id| format!("/2013-04-01/tags/hostedzone/{id}"),
            String::new,
        ),
        (
            "ChangeTagsForResource",
            "POST",
            |id| format!("/2013-04-01/tags/hostedzone/{id}"),
            || {
                "<ChangeTagsForResourceRequest \
                 xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
                 <AddTags><Tag><Key>Owner</Key><Value>dns</Value></Tag></AddTags>\
                 </ChangeTagsForResourceRequest>"
                    .to_string()
            },
        ),
    ];

    for id in &ids {
        for (operation, method, uri_of, body_of) in &operations {
            let case = format!("{operation} on {id}");
            let reply = call(&router, method, &uri_of(id), &body_of()).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchHostedZone",
                "case {case:?}: expected NoSuchHostedZone, got {}",
                reply.body
            );
            assert_eq!(
                reply.first("Error/Message"),
                Some(format!("No hosted zone found with ID: {id}").as_str()),
                "case {case:?}: the message must name the requested id in AWS's form, got {:?}",
                reply.first("Error/Message")
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_caller_reference_is_refused_in_awss_constant_wording() {
    // The rule: a `CallerReference` that already names a zone is refused
    // `HostedZoneAlreadyExists` (409) with AWS's CONSTANT text — it names neither the
    // reference the caller sent nor the server-generated id of the zone that holds it,
    // and the refusal changes nothing. The rows differ in the reference's form and
    // length and in whether the repeat asks for the same zone name or another one.
    let rows = [
        ("a short reference, same name", "cr7", "dup.example.com"),
        (
            "a uuid-shaped reference, another name",
            "b0f2a1c4-3d5e-4f60-8a71-9b2c3d4e5f60",
            "other.example.com",
        ),
        (
            "a long punctuated reference, a deeper name",
            "sim2real-probe-route53-dupzone-432ff155-cr",
            "deep.sub.example.com",
        ),
    ];

    for (case, caller_reference, repeat_name) in rows {
        let router = router();
        let created = call(
            &router,
            "POST",
            "/2013-04-01/hostedzone",
            &create_zone_body("first.example.com", caller_reference, ""),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the first create must succeed, got {} with body {}",
            created.status, created.body
        );
        let zone_id = created
            .first("HostedZone/Id")
            .unwrap_or_default()
            .trim_start_matches("/hostedzone/")
            .to_string();

        let repeat = call(
            &router,
            "POST",
            "/2013-04-01/hostedzone",
            &create_zone_body(repeat_name, caller_reference, ""),
        )
        .await;
        assert_eq!(
            repeat.status, 409,
            "case {case:?}: expected 409, got {} with body {}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error_code(),
            "HostedZoneAlreadyExists",
            "case {case:?}: expected HostedZoneAlreadyExists, got {}",
            repeat.body
        );
        let message = repeat.first("Error/Message").unwrap_or_default();
        assert_eq!(
            message, ALREADY_EXISTS,
            "case {case:?}: expected AWS's constant refusal text, got {message:?}"
        );
        assert!(
            !message.contains(caller_reference) && !message.contains(&zone_id),
            "case {case:?}: the refusal must echo neither the caller reference nor the \
             generated zone id, got {message:?}"
        );

        // The refusal changed nothing: the zone the reference names is still the first
        // one, readable back through GetHostedZone.
        let read_back = call(
            &router,
            "GET",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
            "",
        )
        .await;
        assert_eq!(
            read_back.first("HostedZone/Name"),
            Some("first.example.com."),
            "case {case:?}: the stored zone must be untouched, got {}",
            read_back.body
        );
    }
}

#[tokio::test]
async fn a_non_empty_zone_is_refused_in_awss_constant_wording() {
    // The rule: a zone that still holds a record set other than its default SOA and NS
    // records is refused `HostedZoneNotEmpty` (400) with AWS's CONSTANT text, which
    // names no zone and ends with the remediation. The rows differ in the kind of
    // record that blocks the delete; each is then removed, which lets the same zone
    // delete, so the guard is read off state rather than from the wording.
    let blockers = [
        RecordRow {
            name: "www.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.1"],
        },
        RecordRow {
            name: "example.com",
            rr_type: "TXT",
            ttl: 60,
            values: &["\"v=spf1 -all\""],
        },
        RecordRow {
            name: "deep.sub.example.com",
            rr_type: "CNAME",
            ttl: 900,
            values: &["target.example.net."],
        },
    ];

    for blocker in &blockers {
        let case = format!("{} {}", blocker.rr_type, blocker.name);
        let router = router();
        let zone_id = create_zone_ok(&router, "example.com", "ref-not-empty").await;
        assert_eq!(
            call(
                &router,
                "POST",
                &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
                &one_change("CREATE", blocker),
            )
            .await
            .status,
            200,
            "case {case:?}: the blocking record must be created"
        );

        let refused = call(
            &router,
            "DELETE",
            &format!("/2013-04-01/hostedzone/{zone_id}"),
            "",
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "HostedZoneNotEmpty",
            "case {case:?}: expected HostedZoneNotEmpty, got {}",
            refused.body
        );
        let message = refused.first("Error/Message").unwrap_or_default();
        assert_eq!(
            message, NOT_EMPTY,
            "case {case:?}: expected AWS's constant refusal text, got {message:?}"
        );
        assert!(
            !message.contains(&zone_id),
            "case {case:?}: the refusal must not name the zone's generated id, got {message:?}"
        );

        assert_eq!(
            call(
                &router,
                "POST",
                &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
                &one_change("DELETE", blocker),
            )
            .await
            .status,
            200,
            "case {case:?}: the blocking record must delete"
        );
        assert_eq!(
            call(
                &router,
                "DELETE",
                &format!("/2013-04-01/hostedzone/{zone_id}"),
                "",
            )
            .await
            .status,
            200,
            "case {case:?}: with the record gone the zone must delete"
        );
    }
}

#[tokio::test]
async fn an_invalid_change_batch_names_every_refused_change() {
    // The rule: `InvalidChangeBatch` carries a `messages` LIST with one entry per
    // refused change — "Each error message indicates one error in the change batch" —
    // each naming the record set the change was about by `name=` and `type=`, and the
    // flat `message` is that list rendered as a list. The rows differ in how many
    // changes are refused, in the record names and types they name, and in the action
    // that was refused.
    let existing: [RecordRow; 3] = [
        RecordRow {
            name: "www.example.com",
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.1"],
        },
        RecordRow {
            name: "mail.example.com",
            rr_type: "MX",
            ttl: 600,
            values: &["10 mx.example.net."],
        },
        RecordRow {
            name: "text.example.com",
            rr_type: "TXT",
            ttl: 60,
            values: &["\"hello\""],
        },
    ];
    let absent = RecordRow {
        name: "absent.example.com",
        rr_type: "AAAA",
        ttl: 300,
        values: &["2001:db8::1"],
    };

    // (case, the refused changes as (action, row), the expected message per change)
    let rows: [(&str, Vec<(&str, RecordRow)>); 4] = [
        (
            "one duplicate CREATE",
            vec![("CREATE", existing[0])],
        ),
        (
            "two duplicate CREATEs of different types",
            vec![("CREATE", existing[0]), ("CREATE", existing[1])],
        ),
        (
            "three duplicate CREATEs",
            vec![
                ("CREATE", existing[0]),
                ("CREATE", existing[1]),
                ("CREATE", existing[2]),
            ],
        ),
        (
            "a DELETE of a record set the zone does not hold",
            vec![("DELETE", absent)],
        ),
    ];

    for (case, changes) in &rows {
        let router = router();
        let zone_id = create_zone_ok(&router, "example.com", "ref-batch").await;
        for row in &existing {
            assert_eq!(
                call(
                    &router,
                    "POST",
                    &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
                    &one_change("CREATE", row),
                )
                .await
                .status,
                200,
                "case {case:?}: the record {} must be created first",
                row.name
            );
        }

        let batch: String = changes
            .iter()
            .map(|(action, row)| change_element(action, row))
            .collect();
        let reply = call(
            &router,
            "POST",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            &change_batch_body(&batch, None),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidChangeBatch",
            "case {case:?}: expected InvalidChangeBatch, got {}",
            reply.body
        );

        // One entry per refused change, each naming its record set by name and type.
        let expected: Vec<String> = changes
            .iter()
            .map(|(action, row)| {
                let verb = if *action == "CREATE" { "create" } else { "delete" };
                let tail = if *action == "CREATE" {
                    "but it already exists"
                } else {
                    "but it was not found"
                };
                format!(
                    "Tried to {verb} resource record set [name='{}.', type='{}'] {tail}",
                    row.name, row.rr_type
                )
            })
            .collect();
        let reported = reply.all("messages/member");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected one entry per refused change in AWS's form, got {:?} in {}",
            reported, reply.body
        );

        // The flat member an SDK surfaces is the same list, rendered as a list.
        assert_eq!(
            reply.first("Error/Message"),
            Some(format!("[{}]", expected.join(", ")).as_str()),
            "case {case:?}: the flat message must render the list, got {:?}",
            reply.first("Error/Message")
        );

        // Nothing internal to the handler leaks into the entries: no action tuple and
        // no literal `null` for a member the request never gave.
        let message = reply.first("Error/Message").unwrap_or_default();
        assert!(
            !message.contains("Action=") && !message.contains("SetIdentifier"),
            "case {case:?}: the refusal must not dump the internal change struct, got {message:?}"
        );

        // The refused batch landed nothing: the zone still holds exactly the records it
        // held before it.
        let listed = call(
            &router,
            "GET",
            &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            "",
        )
        .await;
        for row in &existing {
            assert!(
                listed
                    .all("ResourceRecordSet/Name")
                    .contains(&format!("{}.", row.name).as_str()),
                "case {case:?}: {} must still be listed after the refusal, got {}",
                row.name,
                listed.body
            );
        }
    }
}
