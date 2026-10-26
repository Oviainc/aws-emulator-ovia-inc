// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDomainName`, read back through the operations that report
//! the record it removes (`GetDomainName`, `GetDomainNames`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a delete removes exactly the record its identifiers address — `domainName`, plus
//!   `domainNameId` for a private record — so the reads that reported that record stop
//!   reporting it while every sibling record is untouched, and nothing the record carries
//!   conditions the removal (its endpoint type, its certificate, its tags, its policy, or
//!   a `domainNameStatus` the clock still owes it);
//! * identifiers no stored record answers to are the modelled `NotFoundException` (404)
//!   rather than a silent success, which is what makes a second delete of one record
//!   resolve to one success and one 404 rather than to a conflict;
//! * a record another stored resource still DEPENDS on is the modelled
//!   `ConflictException` (409) while that dependency stands, and a refused delete changes
//!   nothing a later caller sees.
//!
//! None of those answers is a 501: every request a caller can make against this operation
//! is one of the statuses its model declares.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `created_domain` is the domain-name suite's one create of a domain name
//! and `association_body` / `create_association` the access-association suite's one
//! create of an association.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    domain_name_has_access_associations, DOMAIN_NAME_PENDING, NO_SUCH_DOMAIN_NAME,
    PRIVATE_ENDPOINT_TYPE, REGIONAL_ENDPOINT_TYPE,
};

use crate::domain_name_access_associations::{association_body, create_association};
use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};

const NOT_FOUND: &str = "NotFoundException";
const CONFLICT: &str = "ConflictException";

/// The status `DeleteDomainName`'s model declares for a success
/// (`smithy_query(query="operation", operation="DeleteDomainName")`:
/// `DELETE /domainnames/{domainName} -> 202`).
const DELETED: u16 = 202;

/// One stored domain name, as this suite addresses it: the name every operation takes in
/// its path label, and the `domainNameId` a PRIVATE record is addressed by as well.
#[derive(Clone, Debug)]
struct Stored {
    case: String,
    domain_name: String,
    domain_name_id: Option<String>,
}

impl Stored {
    /// The query string that addresses this record, empty for a record with no id.
    fn query(&self) -> String {
        match &self.domain_name_id {
            Some(id) => format!("domainNameId={id}"),
            None => String::new(),
        }
    }

    /// The key that tells this record apart from every other stored one — the pair of
    /// identifiers that address it, which is the name alone for a record with no id.
    fn key(&self) -> String {
        match &self.domain_name_id {
            Some(id) => format!("{}+{id}", self.domain_name),
            None => self.domain_name.clone(),
        }
    }
}

/// `DeleteDomainName` for `domain_name`, with `query` appended when it is not empty.
async fn delete_domain(gateway: &Gateway, domain_name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/domainnames/{domain_name}")
    } else {
        format!("/domainnames/{domain_name}?{query}")
    };
    gateway.call("DELETE", &uri, None).await
}

/// `GetDomainName` for `domain_name`, with `query` appended when it is not empty.
async fn get_domain(gateway: &Gateway, domain_name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/domainnames/{domain_name}")
    } else {
        format!("/domainnames/{domain_name}?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// The key of every record the account's `GetDomainNames` collection reports, in page
/// order; see [`Stored::key`].
async fn listed_keys(gateway: &Gateway, case: &str) -> Vec<String> {
    let answer = gateway.call("GET", "/domainnames?limit=500", None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected GetDomainNames to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .iter()
        .map(|item| {
            let name = string_member(item, "domainName", case);
            match item["domainNameId"].as_str() {
                Some(id) => format!("{name}+{id}"),
                None => name,
            }
        })
        .collect()
}

/// `CreateDomainName` for `domain_name` carrying `body`, and the identifiers the stored
/// record is addressed by.
async fn stored_domain(gateway: &Gateway, case: &str, domain_name: &str, body: Value) -> Stored {
    let record = created_domain(gateway, domain_name, body).await;
    Stored {
        case: case.to_owned(),
        domain_name: domain_name.to_owned(),
        domain_name_id: record["domainNameId"].as_str().map(str::to_owned),
    }
}

/// Assert a delete succeeded with the status its model declares and an empty body.
fn assert_deleted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, DELETED,
        "case {case}: expected {DELETED}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body.is_null(),
        "case {case}: expected an empty body, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn deleting_a_domain_name_removes_that_record_alone_and_a_second_delete_is_not_found() {
    let gateway = Gateway::new();

    // Rows differ along what the rule generalises over: the endpoint type the record was
    // created with (every value the model declares for a custom domain name, plus the
    // request that names none), the certificate and TLS members it carries, its tags and
    // its policy — nothing of that conditions the removal — and, for the private rows,
    // the fact that two records may carry ONE name and are told apart by `domainNameId`.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a regional record with a certificate",
            "regional.example.com",
            json!({
                "endpointConfiguration": { "types": [REGIONAL_ENDPOINT_TYPE] },
                "regionalCertificateArn":
                    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555",
                "regionalCertificateName": "regional-cert",
            }),
        ),
        (
            "an edge-optimized record naming no endpoint type, still PENDING",
            "edge.example.com",
            json!({
                "certificateArn":
                    "arn:aws:acm:us-east-1:000000000000:certificate/66666666-7777-8888-9999-000000000000",
                "certificateName": "edge-cert",
            }),
        ),
        (
            "a tagged record with a security policy",
            "tagged.example.net",
            json!({
                "endpointConfiguration": { "types": [REGIONAL_ENDPOINT_TYPE] },
                "securityPolicy": "TLS_1_2",
                "tags": { "team": "platform", "cost:center": "api" },
            }),
        ),
        (
            "a private record carrying a policy",
            "private.example.org",
            json!({
                "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] },
                "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        ),
        (
            "a second private record of the one name",
            "private.example.org",
            json!({ "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] } }),
        ),
        (
            "a bare record, mutual TLS configured",
            "mtls.example.com",
            json!({
                "endpointConfiguration": { "types": [REGIONAL_ENDPOINT_TYPE] },
                "mutualTlsAuthentication": {
                    "truststoreUri": "s3://truststore-bucket/truststore.pem",
                    "truststoreVersion": "1",
                },
            }),
        ),
    ];

    let mut remaining: Vec<Stored> = Vec::new();
    for (case, domain_name, body) in &rows {
        remaining.push(stored_domain(&gateway, case, domain_name, body.clone()).await);
    }
    // The two private rows carry one name and are two records, each addressed by its own
    // `domainNameId`.
    assert_eq!(
        remaining
            .iter()
            .map(Stored::key)
            .collect::<std::collections::BTreeSet<String>>()
            .len(),
        rows.len(),
        "case the created records: expected {} records a delete can tell apart, got {:?}",
        rows.len(),
        remaining.iter().map(Stored::key).collect::<Vec<String>>()
    );
    // The edge-optimized record is in the transient status the clock owes a CloudFront
    // distribution, so this suite deletes a record a caller cannot yet reach.
    let pending = get_domain(&gateway, "edge.example.com", "").await;
    assert_eq!(
        pending.body["domainNameStatus"].as_str(),
        Some(DOMAIN_NAME_PENDING),
        "case the edge-optimized record straight after the create: expected \
         {DOMAIN_NAME_PENDING}, got {:?}",
        pending.body
    );

    // Each record is deleted in turn: the delete is accepted, the record stops being
    // readable and stops being listed, every sibling is untouched, and the second delete
    // of the same identifiers is the modelled not-found.
    while let Some(target) = remaining.first().cloned() {
        remaining.remove(0);
        let case = &target.case;
        let answer = delete_domain(&gateway, &target.domain_name, &target.query()).await;
        assert_deleted(&answer, case);

        let read = get_domain(&gateway, &target.domain_name, &target.query()).await;
        assert_failure(&read, 404, NOT_FOUND, &format!("GetDomainName after {case}"));

        let again = delete_domain(&gateway, &target.domain_name, &target.query()).await;
        assert_failure(
            &again,
            404,
            NOT_FOUND,
            &format!("a second DeleteDomainName of {case}"),
        );

        let expected: Vec<String> = remaining.iter().map(Stored::key).collect();
        let listed = listed_keys(&gateway, &format!("GetDomainNames after {case}")).await;
        assert_eq!(
            listed, expected,
            "case GetDomainNames after {case}: expected the records that are left \
             {expected:?}, got {listed:?}"
        );
        for sibling in &remaining {
            let still_there = get_domain(&gateway, &sibling.domain_name, &sibling.query()).await;
            assert_eq!(
                still_there.status,
                200,
                "case GetDomainName({}) after {case}: expected the sibling to still read, \
                 got {} with {:?}",
                sibling.key(),
                still_there.status,
                still_there.body
            );
        }
    }
}

#[tokio::test]
async fn identifiers_no_stored_domain_name_answers_to_are_the_modelled_not_found() {
    let gateway = Gateway::new();
    let regional = stored_domain(
        &gateway,
        "a regional record",
        "stored.example.com",
        json!({ "endpointConfiguration": { "types": [REGIONAL_ENDPOINT_TYPE] } }),
    )
    .await;
    let private = stored_domain(
        &gateway,
        "a private record",
        "stored-private.example.com",
        json!({ "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] } }),
    )
    .await;
    let private_id = private
        .domain_name_id
        .clone()
        .expect("a private record is created with a domainNameId");

    // Rows differ along HOW the request fails to address a stored record: a name no record
    // carries, a name that is not even a domain name, a stored name with an id no record
    // carries, a stored private name WITHOUT the id that identifies it, and a stored name
    // addressed with ANOTHER record's id.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a name no record carries",
            "absent.example.com".to_string(),
            String::new(),
        ),
        (
            "a name that is not a domain name",
            "not-a-domain-name".to_string(),
            String::new(),
        ),
        (
            "a stored name with an id no record carries",
            regional.domain_name.clone(),
            "domainNameId=zzzzz9".to_string(),
        ),
        (
            "a stored private name without its id",
            private.domain_name.clone(),
            String::new(),
        ),
        (
            "a stored private name with another record's id",
            private.domain_name.clone(),
            format!("domainNameId={private_id}x"),
        ),
        (
            "a stored name with ANOTHER record's id",
            regional.domain_name.clone(),
            format!("domainNameId={private_id}"),
        ),
    ];
    for (case, domain_name, query) in &rows {
        let answer = delete_domain(&gateway, domain_name, query).await;
        assert_failure(&answer, 404, NOT_FOUND, case);
        // The same sentence the reader answers an unresolvable domain-name identifier
        // with: one condition, one message.
        assert_eq!(
            answer.body["message"].as_str(),
            Some(NO_SUCH_DOMAIN_NAME),
            "case {case}: expected {NO_SUCH_DOMAIN_NAME:?}, got {:?}",
            answer.body["message"]
        );
    }

    // None of the refusals removed anything: both records still read and are still listed.
    for stored in [&regional, &private] {
        let read = get_domain(&gateway, &stored.domain_name, &stored.query()).await;
        assert_eq!(
            read.status,
            200,
            "case GetDomainName({}) after the refusals: expected 200, got {} with {:?}",
            stored.key(),
            read.status,
            read.body
        );
    }
    let listed = listed_keys(&gateway, "GetDomainNames after the refusals").await;
    let expected = vec![regional.key(), private.key()];
    assert_eq!(
        listed, expected,
        "case GetDomainNames after the refusals: expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_domain_name_a_stored_access_association_depends_on_is_the_modelled_conflict() {
    let gateway = Gateway::new();
    // Three private records: two carrying ONE name (so each is addressed by its own
    // `domainNameId`) and one of its own, so the rows differ along HOW MANY associations
    // depend on the record — none, one, several.
    let mut records: Vec<Stored> = Vec::new();
    for (case, domain_name) in [
        ("one association", "depends.example.com"),
        ("several associations", "depends.example.com"),
        ("no association", "free.example.com"),
    ] {
        records.push(
            stored_domain(
                &gateway,
                case,
                domain_name,
                json!({ "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] } }),
            )
            .await,
        );
    }

    // The association names the domain name by its ARN, which is what the create reported.
    let mut arns: Vec<String> = Vec::new();
    for stored in &records {
        let read = get_domain(&gateway, &stored.domain_name, &stored.query()).await;
        arns.push(string_member(
            &read.body,
            "domainNameArn",
            &format!("GetDomainName({})", stored.key()),
        ));
    }
    // Rows: one association on the first record, two on the second, none on the third.
    for (index, sources) in [
        (0_usize, vec!["vpce-0123456789abcdef0"]),
        (1, vec!["vpce-00000000000000001", "vpce-00000000000000002"]),
    ] {
        for source in sources {
            let answer = create_association(&gateway, association_body(&arns[index], source)).await;
            assert_eq!(
                answer.status, 201,
                "case CreateDomainNameAccessAssociation({source}): expected 201, got {} \
                 with {:?}",
                answer.status, answer.body
            );
        }
    }

    // While an association depends on the record, every delete of it is the modelled
    // conflict — however many associations there are, and however often it is tried.
    for index in [0_usize, 1] {
        let target = &records[index];
        for attempt in 1..=2 {
            let answer = delete_domain(&gateway, &target.domain_name, &target.query()).await;
            let case = format!(
                "DeleteDomainName({}) with {} (attempt {attempt})",
                target.key(),
                target.case
            );
            assert_failure(&answer, 409, CONFLICT, &case);
            // The conflict says WHICH call to make first, in the one sentence this service
            // words the condition with.
            assert_eq!(
                answer.body["message"].as_str(),
                Some(domain_name_has_access_associations(&target.domain_name).as_str()),
                "case {case}: expected the dependency sentence, got {:?}",
                answer.body["message"]
            );
        }
        // A refused delete changed nothing a later caller sees.
        let read = get_domain(&gateway, &target.domain_name, &target.query()).await;
        assert_eq!(
            read.status,
            200,
            "case GetDomainName({}) after the conflict: expected the record to still read, \
             got {} with {:?}",
            target.key(),
            read.status,
            read.body
        );
    }

    // The record NOTHING depends on is deleted by the same call: it is the dependency that
    // refuses the delete, not the kind of record.
    let free = &records[2];
    assert_deleted(
        &delete_domain(&gateway, &free.domain_name, &free.query()).await,
        &format!("DeleteDomainName({}) with {}", free.key(), free.case),
    );
    let listed = listed_keys(&gateway, "GetDomainNames after the conflicts").await;
    let expected = vec![records[0].key(), records[1].key()];
    assert_eq!(
        listed, expected,
        "case GetDomainNames after the conflicts: expected the records an association \
         depends on {expected:?}, got {listed:?}"
    );
}
