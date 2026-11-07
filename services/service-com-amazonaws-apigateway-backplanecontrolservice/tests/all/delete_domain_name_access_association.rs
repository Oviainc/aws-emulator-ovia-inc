// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDomainNameAccessAssociation`, read back through the
//! operations that report the record it removes (`GetDomainNameAccessAssociations`,
//! `GetTags` by the association's ARN).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a delete removes exactly the one stored association its ARN addresses, so the reads
//!   that reported that record stop reporting it while every sibling is untouched, and
//!   nothing the record carries conditions the removal (its tags, the form of its source
//!   identifier, or which private domain name it was created against — including two
//!   private records that share one name, which are two domain names);
//! * removing an association releases what depended on it: the pair
//!   `(domain name, source)` is free for a new create, and a domain name whose last
//!   association is gone can be deleted;
//! * an ARN this service cannot read as one of ITS access association ARNs is the
//!   modelled `BadRequestException` (400), while a well-formed one no stored record
//!   answers to is the modelled `NotFoundException` (404) rather than a silent success —
//!   which is what makes a second delete of one record, and concurrent deletes of one
//!   record over a table of racer counts, resolve to one success and 404s rather than to a
//!   conflict;
//! * a refused delete changes nothing a later caller sees.
//!
//! None of those answers is a 501: every request a caller can make against this operation
//! is one of the statuses its model declares.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness; `private_domain`, `association_body`,
//! `create_association`, `list_associations` and `listed_arns` are the access-association
//! suite's own calls, and `apigateway_arn` / `encode_label` / `get_tags` the tagging
//! suite's one spelling of an ARN, of a path label and of a tag read.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    INVALID_ARN, NO_SUCH_ACCESS_ASSOCIATION,
};

use crate::domain_name_access_associations::{
    association_body, create_association, list_associations, listed_arns, private_domain,
};
use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};

const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";
const CONFLICT: &str = "ConflictException";

/// The status `DeleteDomainNameAccessAssociation`'s model declares for a success
/// (`smithy_query(query="operation", operation="DeleteDomainNameAccessAssociation")`:
/// `DELETE /domainnameaccessassociations/{domainNameAccessAssociationArn} -> 202`).
const DELETED: u16 = 202;

/// `DeleteDomainNameAccessAssociation` for `arn`, which the model binds as a path label —
/// so the whole ARN travels percent-encoded inside ONE path segment.
async fn delete_association(gateway: &Gateway, arn: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/domainnameaccessassociations/{}", encode_label(arn)),
            None,
        )
        .await
}

/// A well-formed ARN of a domain name access association of this service.
fn association_arn(id: &str) -> String {
    apigateway_arn(&format!("/domainnameaccessassociations/{id}"))
}

/// The `DeleteDomainName` URI that addresses the record `domain_name_arn` names.
///
/// A private record is keyed `<domainName>+<domainNameId>` — several private records may
/// carry one name, so both members address one — and that key is the resource portion of
/// its ARN, which is what this reads it out of.
fn delete_domain_uri(domain_name_arn: &str) -> String {
    let key = domain_name_arn
        .rsplit("/domainnames/")
        .next()
        .expect("a domain name ARN carries its key");
    match key.split_once('+') {
        Some((domain_name, domain_name_id)) => {
            format!("/domainnames/{domain_name}?domainNameId={domain_name_id}")
        }
        None => format!("/domainnames/{key}"),
    }
}

/// Assert a delete was accepted with the status its model declares and an empty body.
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

/// The ARNs of every stored association, in the order the collection reports them.
async fn stored_arns(gateway: &Gateway, case: &str) -> Vec<String> {
    let answer = list_associations(gateway, "limit=500").await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected GetDomainNameAccessAssociations to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    listed_arns(&answer.body, case)
}

/// `CreateDomainNameAccessAssociation` for the pair, asserting it was stored and answering
/// the ARN the service assigned it.
async fn stored_association(gateway: &Gateway, domain_name_arn: &str, source: &str) -> String {
    let answer = create_association(gateway, association_body(domain_name_arn, source)).await;
    assert_eq!(
        answer.status, 201,
        "case CreateDomainNameAccessAssociation({source}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(
        &answer.body,
        "domainNameAccessAssociationArn",
        &format!("CreateDomainNameAccessAssociation({source})"),
    )
}

#[tokio::test]
async fn deleting_an_association_removes_that_record_alone_and_a_second_delete_is_not_found() {
    let gateway = Gateway::new();
    // Two private records carrying ONE name are two domain names — a `domainNameId` is
    // "supported only for private custom domain names" and is what identifies one — so an
    // association against each is an association against a different domain name.
    let first = private_domain(&gateway, "private.example.com").await;
    let second = private_domain(&gateway, "private.example.com").await;
    let third = private_domain(&gateway, "other.example.org").await;

    // Rows differ along what the rule generalises over: the domain name the association
    // was created against, the form of its source identifier, and the tag collection it
    // carries — none of which conditions the removal.
    let rows: Vec<(&str, &str, &str, serde_json::Value)> = vec![
        (
            "an untagged association",
            first.as_str(),
            "vpce-0123456789abcdef0",
            json!({}),
        ),
        (
            "a tagged association of the same domain name",
            first.as_str(),
            "vpce-00000000000000001",
            json!({ "Name": "payments" }),
        ),
        (
            "an association of the other private record of one name",
            second.as_str(),
            "vpce-0123456789abcdef0",
            json!({ "cost:center": "team/platform", "empty-value": "" }),
        ),
        (
            "an association of another domain name, the shorter source id width",
            third.as_str(),
            "vpce-0a1b2c3d",
            json!({ "Name": "orders" }),
        ),
    ];

    let mut remaining: Vec<(String, String)> = Vec::new();
    for (case, domain_name_arn, source, tags) in &rows {
        let mut body = association_body(domain_name_arn, source);
        if tags.as_object().is_some_and(|entries| !entries.is_empty()) {
            body["tags"] = tags.clone();
        }
        let answer = create_association(&gateway, body).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        remaining.push((
            (*case).to_string(),
            string_member(&answer.body, "domainNameAccessAssociationArn", case),
        ));
    }

    // Each association is deleted in turn: the delete is accepted, the record stops being
    // listed and stops answering a tag read by its ARN, every sibling is untouched, and a
    // second delete of the same ARN is the modelled not-found.
    while let Some((case, arn)) = remaining.first().cloned() {
        remaining.remove(0);
        assert_deleted(&delete_association(&gateway, &arn).await, &case);

        let again = delete_association(&gateway, &arn).await;
        assert_failure(&again, 404, NOT_FOUND, &format!("a second delete of {case}"));
        assert_eq!(
            again.body["message"].as_str(),
            Some(NO_SUCH_ACCESS_ASSOCIATION),
            "case a second delete of {case}: expected {NO_SUCH_ACCESS_ASSOCIATION:?}, got {:?}",
            again.body["message"]
        );

        let tags = get_tags(&gateway, &arn).await;
        assert_failure(&tags, 404, NOT_FOUND, &format!("GetTags after {case}"));

        let expected: Vec<String> = remaining.iter().map(|(_, arn)| arn.clone()).collect();
        let listed = stored_arns(&gateway, &format!("the collection after {case}")).await;
        assert_eq!(
            listed, expected,
            "case the collection after {case}: expected the records that are left \
             {expected:?}, got {listed:?}"
        );
        for (sibling_case, sibling) in &remaining {
            let sibling_tags = get_tags(&gateway, sibling).await;
            assert_eq!(
                sibling_tags.status, 200,
                "case GetTags({sibling_case}) after {case}: expected the sibling to still \
                 read, got {} with {:?}",
                sibling_tags.status, sibling_tags.body
            );
        }
    }

    let listed = stored_arns(&gateway, "the collection after every delete").await;
    assert!(
        listed.is_empty(),
        "case the collection after every delete: expected an empty page, got {listed:?}"
    );
}

#[tokio::test]
async fn removing_an_association_releases_the_pair_and_the_domain_name_it_named() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "released.example.com").await;
    // Rows differ along HOW MANY associations the domain name carries when the delete runs,
    // so the release is asserted while another association still stands and once the last
    // one is gone.
    let sources = ["vpce-0123456789abcdef0", "vpce-00000000000000002"];
    let mut arns: Vec<String> = Vec::new();
    for source in sources {
        arns.push(stored_association(&gateway, &domain, source).await);
    }

    // The domain name cannot be deleted while an association names it.
    let uri = delete_domain_uri(&domain);
    let blocked = gateway.call("DELETE", &uri, None).await;
    assert_failure(
        &blocked,
        409,
        CONFLICT,
        "DeleteDomainName while two associations stand",
    );

    for (index, source) in sources.iter().enumerate() {
        let case = format!("the association of {source}");
        assert_deleted(&delete_association(&gateway, &arns[index]).await, &case);

        // The pair is free again: a create of the very same pair is accepted and gets its
        // OWN ARN, so what the delete released is the pair and not just the identifier.
        let recreated = stored_association(&gateway, &domain, source).await;
        assert_ne!(
            recreated, arns[index],
            "case {case} recreated: expected a new association ARN, got {recreated:?} again"
        );
        assert_deleted(
            &delete_association(&gateway, &recreated).await,
            &format!("{case} recreated"),
        );
    }

    // With the last association gone the domain name is deleted by the call that was
    // refused above: it is the dependency that refuses the delete, not the record.
    let freed = gateway.call("DELETE", &uri, None).await;
    assert_deleted(&freed, "DeleteDomainName once no association stands");
}

#[tokio::test]
async fn an_arn_this_service_cannot_read_is_the_modelled_bad_request() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "refusals.example.com").await;
    let stored = stored_association(&gateway, &domain, "vpce-0123456789abcdef0").await;

    // Rows differ along HOW the value fails to be one of this service's access association
    // ARNs: not an ARN at all, an ARN missing fields, an ARN of another vendor, an API
    // Gateway ARN naming another kind of resource, and an ARN with no identifier.
    let rows: Vec<(&str, String)> = vec![
        ("a value that is not an arn", "not-a-valid-arn".to_string()),
        ("a bare identifier", "dnaa-1a2b3c4d".to_string()),
        ("an arn with too few fields", "arn:aws:apigateway".to_string()),
        (
            "an arn of another vendor",
            "arn:aws:s3:::my-bucket".to_string(),
        ),
        (
            "an arn naming a rest api",
            apigateway_arn("/restapis/abcde12345"),
        ),
        (
            "an arn naming a domain name",
            apigateway_arn("/domainnames/refusals.example.com"),
        ),
        (
            "an arn naming an api key",
            apigateway_arn("/apikeys/abcde12345"),
        ),
        ("an empty resource portion", apigateway_arn("/")),
        (
            "an arn with no identifier",
            apigateway_arn("/domainnameaccessassociations/"),
        ),
    ];
    for (case, arn) in &rows {
        let answer = delete_association(&gateway, arn).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        // ONE sentence for a value this service could not read as an ARN, and it carries
        // none of that value: the same fixed text whichever member took it.
        assert_eq!(
            answer.body["message"].as_str(),
            Some(INVALID_ARN),
            "case {case}: expected the unreadable-ARN sentence, got {:?}",
            answer.body["message"]
        );
    }

    // None of the refusals removed anything.
    let listed = stored_arns(&gateway, "the collection after the bad requests").await;
    assert_eq!(
        listed,
        vec![stored.clone()],
        "case the collection after the bad requests: expected {stored:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_well_formed_arn_naming_no_stored_association_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "absent.example.com").await;
    let stored = stored_association(&gateway, &domain, "vpce-0123456789abcdef0").await;
    let stored_id = stored
        .rsplit('/')
        .next()
        .expect("the stored ARN carries an identifier")
        .to_string();

    // Rows differ along the identifier the readable ARN carries: one no record answers to,
    // the stored one in another case, the stored one with a suffix, and the stored one with
    // its last character dropped. Nothing about which identifier is named changes the
    // answer while no record answers to it.
    let rows: Vec<(&str, String)> = vec![
        (
            "an identifier no record answers to",
            association_arn("zzzzz99999"),
        ),
        (
            "the stored identifier upper-cased",
            association_arn(&stored_id.to_uppercase()),
        ),
        (
            "the stored identifier with a suffix",
            association_arn(&format!("{stored_id}x")),
        ),
        (
            "the stored identifier one character short",
            association_arn(&stored_id[..stored_id.len() - 1]),
        ),
    ];
    for (case, arn) in &rows {
        let answer = delete_association(&gateway, arn).await;
        assert_failure(&answer, 404, NOT_FOUND, case);
        // The same sentence every reader of this ARN answers an unresolvable identifier
        // with: one condition, one message.
        assert_eq!(
            answer.body["message"].as_str(),
            Some(NO_SUCH_ACCESS_ASSOCIATION),
            "case {case}: expected {NO_SUCH_ACCESS_ASSOCIATION:?}, got {:?}",
            answer.body["message"]
        );
    }

    // None of the refusals removed the record that IS stored.
    let listed = stored_arns(&gateway, "the collection after the not-founds").await;
    assert_eq!(
        listed,
        vec![stored.clone()],
        "case the collection after the not-founds: expected {stored:?}, got {listed:?}"
    );
    assert_deleted(
        &delete_association(&gateway, &stored).await,
        "the stored association after the refusals",
    );
}

/// RULE: concurrent deletes of ONE association resolve to exactly one success — the record
/// is looked up and removed under one hold of the store — so every other caller is refused
/// the modelled not-found, whatever the racer count.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_of_one_association_resolve_to_exactly_one_success() {
    for racers in [2_usize, 3, 6] {
        let case = format!("{racers} concurrent deletes of one association");
        let gateway = Arc::new(Gateway::new());
        let domain = private_domain(&gateway, "race.example.com").await;
        let arn = stored_association(&gateway, &domain, "vpce-0123456789abcdef0").await;
        // A sibling association of the same domain name that no racer addresses: what the
        // contended delete removes is the record its ARN names and nothing else.
        let sibling = stored_association(&gateway, &domain, "vpce-00000000000000002").await;

        let mut tasks = Vec::with_capacity(racers);
        for _ in 0..racers {
            let gateway = Arc::clone(&gateway);
            let arn = arn.clone();
            tasks.push(tokio::spawn(async move {
                let answer = delete_association(&gateway, &arn).await;
                (answer.status, answer.error)
            }));
        }
        let mut answers = Vec::with_capacity(racers);
        for task in tasks {
            answers.push(task.await.expect("the racing delete answered"));
        }

        let accepted = answers
            .iter()
            .filter(|(status, _)| *status == DELETED)
            .count();
        assert_eq!(
            accepted, 1,
            "case {case}: expected exactly one accepted delete, got {answers:?}"
        );
        for (status, error) in &answers {
            if *status == DELETED {
                continue;
            }
            assert_eq!(
                (*status, error.as_deref()),
                (404, Some(NOT_FOUND)),
                "case {case}: every loser is the modelled not-found, got {answers:?}"
            );
        }

        let listed = stored_arns(&gateway, &case).await;
        assert_eq!(
            listed,
            vec![sibling.clone()],
            "case {case}: expected only the sibling {sibling:?} to be left, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_collection_a_delete_leaves_is_what_a_later_reader_sees() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "stable.example.com").await;
    let mut arns: Vec<String> = Vec::new();
    for index in 0..4 {
        arns.push(
            stored_association(&gateway, &domain, &format!("vpce-0000000000000000{index}")).await,
        );
    }
    // Delete the middle record: the records around it keep their creation order, which is
    // what the collection pages over.
    assert_deleted(
        &delete_association(&gateway, &arns[1]).await,
        "the second of four associations",
    );
    let expected = vec![arns[0].clone(), arns[2].clone(), arns[3].clone()];
    let listed = stored_arns(&gateway, "the collection after the middle delete").await;
    assert_eq!(
        listed, expected,
        "case the collection after the middle delete: expected creation order {expected:?}, \
         got {listed:?}"
    );

    // Paging one record at a time reassembles the same sequence, and reading twice reports
    // the same page: the delete is what changed the collection, not the reads.
    let mut seen: Vec<String> = Vec::new();
    let mut position: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match &position {
            None => "limit=1".to_string(),
            Some(cursor) => format!("limit=1&position={cursor}"),
        };
        let answer = list_associations(&gateway, &query).await;
        assert_eq!(
            answer.status, 200,
            "case paging after the middle delete: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        seen.extend(listed_arns(&answer.body, "paging after the middle delete"));
        pages += 1;
        assert!(
            pages <= 20,
            "case paging after the middle delete: pagination did not terminate"
        );
        match answer.body["position"].as_str() {
            None => break,
            Some(cursor) => position = Some(cursor.to_owned()),
        }
    }
    assert_eq!(
        seen, expected,
        "case paging after the middle delete: expected {expected:?}, got {seen:?}"
    );

    let once = list_associations(&gateway, "limit=500").await;
    let twice = list_associations(&gateway, "limit=500").await;
    assert_eq!(
        once.body, twice.body,
        "case the collection read twice: expected the same page, got {:?}",
        twice.body
    );
    assert!(
        !page_items(&once.body, "the collection read twice").is_empty(),
        "case the collection read twice: expected the records that are left, got {:?}",
        once.body
    );
}
