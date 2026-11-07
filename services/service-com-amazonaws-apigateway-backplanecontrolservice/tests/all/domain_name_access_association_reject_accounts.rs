// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `RejectDomainNameAccessAssociation` answers once BOTH of its
//! ARNs resolve: the account the two resources are in is what decides the call.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The sibling `domain_name_access_association_reject`
//! suite covers the two refusals that come BEFORE this decision (an unreadable ARN, and a
//! well-formed pair naming records nothing holds); this one starts from a pair that names
//! stored records.
//!
//! The RULE under test, from the operation's own documentation: "To reject a domain name
//! access association with an access association source in ANOTHER AWS account, use this
//! operation. To remove a domain name access association with an access association source
//! in YOUR OWN account, use the DeleteDomainNameAccessAssociation operation." So
//!
//! * a pair whose two ARNs name ONE account is the modelled `BadRequestException` (400)
//!   carrying AWS's fixed sentence for that class, and changes nothing;
//! * a pair whose two ARNs name TWO accounts is the rejection itself: the modelled 202 with
//!   an empty body, and the association is gone from the account's records;
//! * the association named must be one the domain name named actually HAS, or the pair names
//!   no association to reject: the modelled `NotFoundException` (404) carrying the sentence
//!   API Gateway words that condition with.
//!
//! An ARN's fifth field is what a request says whose a resource is, and it is EMPTY on every
//! ARN this service mints (API Gateway's own ARNs carry no account), so a pair of minted
//! ARNs names one account — which is the recorded same-account case — while a caller that
//! names another account in either member is asking about two.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`] and [`string_member`] are this crate's one
//! wire-test harness; `private_domain`, `association_body`, `create_association`,
//! `list_associations` and `listed_arns` are the access-association suite's one create and
//! one read of these records, and `encode_label` the tagging suite's one query encoding.

use crate::domain_name_access_associations::{
    association_body, create_association, list_associations, listed_arns, private_domain,
};
use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};
use crate::tag_reads_by_arn::encode_label;

const BAD_REQUEST: &str = "BadRequestException";
const NOT_FOUND: &str = "NotFoundException";

/// What AWS refuses a SAME-ACCOUNT rejection with, verbatim — wholly fixed text for that
/// error class, with no part of it taken from the request or from the store.
const SAME_ACCOUNT_MESSAGE: &str = "Cannot reject the domain name access association when \
                                    the private custom domain and domain name access \
                                    association are in the same account.";

/// What an ARN naming no access association of the domain name named is refused with — the
/// sentence every operation of this service that resolves such an ARN answers.
const NO_SUCH_ASSOCIATION_MESSAGE: &str =
    "This private custom domain name doesn't have any domain name access associations.";

/// `RejectDomainNameAccessAssociation` for the pair `(association, domain_name)`, both
/// carried in the query string the way the model binds them.
async fn reject(gateway: &Gateway, association: &str, domain_name: &str) -> Answer {
    gateway
        .call(
            "POST",
            &format!(
                "/rejectdomainnameaccessassociations\
                 ?domainNameAccessAssociationArn={}&domainNameArn={}",
                encode_label(association),
                encode_label(domain_name)
            ),
            None,
        )
        .await
}

/// `arn` with its ACCOUNT field (the fifth of the six) replaced by `account`, every other
/// field left exactly as the service minted it.
///
/// This is how a case says whose a resource is: the resource portion still addresses the
/// very record the service stored, and only the owner the ARN names changes.
fn with_account(arn: &str, account: &str) -> String {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case with_account({arn:?}): expected a six-field ARN, got {fields:?}"
    );
    format!(
        "{}:{}:{}:{}:{}:{}",
        fields[0], fields[1], fields[2], fields[3], account, fields[5]
    )
}

/// A fresh PRIVATE custom domain name and one stored access association of `source` with
/// it, answering `(domain name ARN, association ARN)` as the service minted them.
async fn stored_pair(gateway: &Gateway, domain_name: &str, source: &str) -> (String, String) {
    let domain = private_domain(gateway, domain_name).await;
    let created = create_association(gateway, association_body(&domain, source)).await;
    assert_eq!(
        created.status, 201,
        "case CreateDomainNameAccessAssociation({domain_name}, {source}): expected 201, got \
         {} with {:?}",
        created.status, created.body
    );
    let association = string_member(
        &created.body,
        "domainNameAccessAssociationArn",
        &format!("CreateDomainNameAccessAssociation({domain_name}, {source})"),
    );
    (domain, association)
}

/// Every stored association's ARN, which is what says whether a rejection removed one.
async fn held_associations(gateway: &Gateway, case: &str) -> Vec<String> {
    let listed = list_associations(gateway, "limit=500").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected 200 from GetDomainNameAccessAssociations, got {} with {:?}",
        listed.status, listed.body
    );
    listed_arns(&listed.body, case)
}

#[tokio::test]
async fn a_same_account_pair_is_refused_with_aws_fixed_sentence_and_changes_nothing() {
    // Rows differ along how the pair SPELLS the one account it names: the empty field every
    // ARN this service mints carries, that field written out explicitly, and two different
    // explicit account ids — each on both members, so each row names one account.
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("both ARNs as the service minted them", None, None),
        ("both carrying an empty account field", Some(""), Some("")),
        (
            "both carrying one explicit account",
            Some("111122223333"),
            Some("111122223333"),
        ),
        (
            "both carrying another explicit account",
            Some("444455556666"),
            Some("444455556666"),
        ),
    ];

    for (index, (case, association_account, domain_account)) in rows.iter().enumerate() {
        let gateway = Gateway::new();
        let (domain, association) = stored_pair(
            &gateway,
            &format!("same-{index}.example.com"),
            &format!("vpce-0000000000000000{index}"),
        )
        .await;
        let named_association = match association_account {
            None => association.clone(),
            Some(account) => with_account(&association, account),
        };
        let named_domain = match domain_account {
            None => domain.clone(),
            Some(account) => with_account(&domain, account),
        };

        let answer = reject(&gateway, &named_association, &named_domain).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(SAME_ACCOUNT_MESSAGE),
            "case {case}: expected AWS's fixed same-account sentence, got {:?}",
            answer.body["message"]
        );

        // A refused rejection is not a removal: the association is still the account's.
        assert_eq!(
            held_associations(&gateway, case).await,
            vec![association.clone()],
            "case {case}: expected the association still stored, got the collection above"
        );
    }
}

#[tokio::test]
async fn a_cross_account_pair_is_rejected_with_an_empty_202_and_the_association_is_gone() {
    // Rows differ along WHICH member names the other account and how far apart the two
    // accounts are: the association elsewhere, the domain name elsewhere, two unrelated
    // accounts, and two accounts differing in a single digit.
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        (
            "the association in another account, the domain name as minted",
            Some("111122223333"),
            None,
        ),
        (
            "the domain name in another account, the association as minted",
            None,
            Some("111122223333"),
        ),
        (
            "two unrelated accounts",
            Some("111122223333"),
            Some("444455556666"),
        ),
        (
            "two accounts differing in one digit",
            Some("111122223333"),
            Some("111122223334"),
        ),
    ];

    for (index, (case, association_account, domain_account)) in rows.iter().enumerate() {
        let gateway = Gateway::new();
        let (domain, association) = stored_pair(
            &gateway,
            &format!("cross-{index}.example.com"),
            &format!("vpce-0000000000000001{index}"),
        )
        .await;
        let named_association = match association_account {
            None => association.clone(),
            Some(account) => with_account(&association, account),
        };
        let named_domain = match domain_account {
            None => domain.clone(),
            Some(account) => with_account(&domain, account),
        };

        let answer = reject(&gateway, &named_association, &named_domain).await;
        assert_eq!(
            answer.status, 202,
            "case {case}: expected the modelled 202, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_null(),
            "case {case}: expected an empty body, got {:?}",
            answer.body
        );

        // The rejection removed the association: nothing lists it any more, and rejecting
        // it a second time finds a domain name with no associations left at all.
        assert!(
            held_associations(&gateway, case).await.is_empty(),
            "case {case}: expected no association left, got the collection above"
        );
        let again = reject(&gateway, &named_association, &named_domain).await;
        assert_failure(&again, 404, NOT_FOUND, &format!("{case}, rejected twice"));
        assert_eq!(
            again.body["message"].as_str(),
            Some(NO_SUCH_ASSOCIATION_MESSAGE),
            "case {case}, rejected twice: expected the no-such-association sentence, got {:?}",
            again.body["message"]
        );
    }
}

#[tokio::test]
async fn the_association_must_be_one_the_domain_name_named_actually_has() {
    let gateway = Gateway::new();
    let (first_domain, first_association) =
        stored_pair(&gateway, "pair-a.example.com", "vpce-0123456789abcdef0").await;
    // A second PRIVATE record carrying the SAME name is a different domain name — a
    // `domainNameId` is "supported only for private custom domain names" and is what
    // identifies one — so an association against the first is not one against this.
    let same_name_other_record = private_domain(&gateway, "pair-a.example.com").await;
    assert_ne!(
        first_domain, same_name_other_record,
        "case CreateDomainName(two private records of one name): expected distinct ARNs, got \
         {first_domain:?} twice"
    );
    // A domain name that HAS an association of its own, so a pair naming the other domain
    // name's association is refused on the identifier and not on an empty domain name.
    let (populated_domain, populated_association) =
        stored_pair(&gateway, "pair-b.example.com", "vpce-00000000000000002").await;

    // Rows differ along HOW the pair names no association of the domain name it names:
    // another private record carrying the same name, which has none; the same with the
    // domain name or the association named in a second account; a domain name that has an
    // association of its own but not this one; and the mirror of that row.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "another private record carrying the same name, which has none",
            first_association.clone(),
            same_name_other_record.clone(),
        ),
        (
            "a domain name with none, named in a second account",
            first_association.clone(),
            with_account(&same_name_other_record, "111122223333"),
        ),
        (
            "a domain name with none and an association named in a second account",
            with_account(&first_association, "111122223333"),
            same_name_other_record.clone(),
        ),
        (
            "another domain name's association",
            first_association.clone(),
            populated_domain.clone(),
        ),
        (
            "the mirror of that pair",
            populated_association.clone(),
            first_domain.clone(),
        ),
    ];
    for (case, association, domain_name) in &rows {
        let answer = reject(&gateway, association, domain_name).await;
        assert_failure(&answer, 404, NOT_FOUND, case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(NO_SUCH_ASSOCIATION_MESSAGE),
            "case {case}: expected the no-such-association sentence, got {:?}",
            answer.body["message"]
        );
    }

    // None of those refusals removed anything.
    let mut held = held_associations(&gateway, "after the mismatched pairs").await;
    held.sort();
    let mut expected = vec![first_association.clone(), populated_association.clone()];
    expected.sort();
    assert_eq!(
        held, expected,
        "case after the mismatched pairs: expected both associations still stored, got {held:?}"
    );

    // The pair that does correspond is decided by the accounts, not by the lookup: these
    // two ARNs name one account, so it is the fixed same-account refusal.
    let answer = reject(&gateway, &first_association, &first_domain).await;
    assert_failure(&answer, 400, BAD_REQUEST, "the corresponding pair");
    assert_eq!(
        answer.body["message"].as_str(),
        Some(SAME_ACCOUNT_MESSAGE),
        "case the corresponding pair: expected AWS's fixed same-account sentence, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn the_same_account_sentence_carries_nothing_from_the_request_or_the_store() {
    // Rows differ along every value the refusal COULD have been built from — the domain
    // name, the access association source, and so the minted association id — and the
    // sentence must be byte-for-byte the same in each.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a short name", "a.example.com", "vpce-0a1b2c3d"),
        (
            "a long name and a long source",
            "deeply.nested.private.api.example.co.uk",
            "vpce-0123456789abcdef0",
        ),
        ("a wildcard name", "*.example.net", "vpce-00000000000000009"),
    ];

    let mut sentences: Vec<String> = Vec::new();
    for (case, domain_name, source) in &rows {
        let gateway = Gateway::new();
        let (domain, _association) = stored_pair(&gateway, domain_name, source).await;
        let created = list_associations(&gateway, "limit=500").await;
        let stored = listed_arns(&created.body, case);
        let answer = reject(&gateway, &stored[0], &domain).await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
        let message = string_member(&answer.body, "message", case);
        assert_eq!(
            message, SAME_ACCOUNT_MESSAGE,
            "case {case}: expected AWS's fixed same-account sentence, got {message:?}"
        );
        assert!(
            !message.contains(domain_name) && !message.contains(source),
            "case {case}: the sentence must carry nothing from the request, got {message:?}"
        );
        sentences.push(message);
    }
    let first = sentences[0].clone();
    assert!(
        sentences.iter().all(|sentence| *sentence == first),
        "case the fixed sentence across rows: expected one sentence, got {sentences:?}"
    );
}
