// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyAddressAttribute` — the operation that sets the reverse DNS
//! (PTR) record of an Elastic IP address — driven over the wire the service actually speaks
//! (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. A MODIFY SETS THE NAMED ADDRESS'S PTR RECORD and reports the change as OUTSTANDING
//!    beside it: the attribute it answers is the stored address's own (its public address and
//!    allocation id), carries the record as the fully qualified name a PTR record is — the
//!    root label's trailing dot included, whichever of the two forms the request sent — and
//!    `DescribeAddressesAttribute`, which reads the same stored record, reports exactly the
//!    same attribute. The outstanding change comes due one `TRANSITION_DELAY` later whether or
//!    not anybody calls again, after which the record alone is reported.
//! 2. ONE OUTSTANDING REQUEST PER RECORD: while a change on an address has not come due,
//!    another cannot be started on it — the second is refused `PendingRequestAlreadyExists`,
//!    which neither consumes nor replaces the outstanding one — and an address the request did
//!    not name is unaffected.
//! 3. A REQUEST THAT NAMES NO RECORD TO SET, or a value that is no domain name, or an id that
//!    names no address of the account, is refused and nothing is written.
//!
//! Each table row is a distinct input along what the rules generalise over — which address of
//! how many is named, which spelling and length of domain name, how many requests have already
//! been made on the record — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    ADDRESS_ATTRIBUTE_DOMAIN_NAME, MAX_DNS_LABEL_BYTES, PTR_UPDATE_STATUS_PENDING,
    TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// One row of the modify table: the case, which address of four it names, the domain name it
/// sends and the record that must end up in effect.
type ModifyRow = (&'static str, usize, String, String);

/// One row of the refusal table: the case, the allocation id, the domain name and the code
/// the refusal carries.
type RefusalRow = (&'static str, String, Option<String>, &'static str);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// Long enough past [`TRANSITION_DELAY`] that a change the clock owes a PTR record is
/// certainly due, with a margin so a slow machine cannot read the state before it is.
fn past_the_change() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The `domain-name` attribute of one address as a response reported it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Attribute {
    public_ip: Option<String>,
    allocation_id: Option<String>,
    ptr_record: Option<String>,
    update_status: Option<String>,
    update_value: Option<String>,
}

fn attribute_of(node: roxmltree::Node<'_, '_>) -> Attribute {
    let update = child(node, "ptrRecordUpdate");
    Attribute {
        public_ip: child_text(node, "publicIp"),
        allocation_id: child_text(node, "allocationId"),
        ptr_record: child_text(node, "ptrRecord"),
        update_status: update.and_then(|update| child_text(update, "status")),
        update_value: update.and_then(|update| child_text(update, "value")),
    }
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Allocate one Elastic IP address and answer `(allocation id, public address)`.
async fn allocate(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, body) = call(router, REGION, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateAddress, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let root = parsed.root_element();
    (
        child_text(root, "allocationId").unwrap_or_default(),
        child_text(root, "publicIp").unwrap_or_default(),
    )
}

/// One `ModifyAddressAttribute` call for `allocation_id`, with `extra` fields beside.
async fn modify(
    router: &MultiServiceRouter,
    allocation_id: &str,
    domain_name: Option<&str>,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("AllocationId", allocation_id)];
    if let Some(domain_name) = domain_name {
        params.push(p("DomainName", domain_name));
    }
    params.extend_from_slice(extra);
    call(router, REGION, "ModifyAddressAttribute", &params).await
}

/// The attribute a successful call answered, out of its `<address>`.
fn answered(case: &str, body: &str) -> Attribute {
    let parsed = document(case, body);
    let address = child(parsed.root_element(), "address")
        .unwrap_or_else(|| panic!("case {case:?}: no <address> in the response {body}"));
    attribute_of(address)
}

/// The attribute `DescribeAddressesAttribute` reports for `allocation_id`, or `None` when it
/// reports no row for that address.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
) -> Option<Attribute> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeAddressesAttribute",
        &[
            p("AllocationId.1", allocation_id),
            p("Attribute", ADDRESS_ATTRIBUTE_DOMAIN_NAME),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAddressesAttribute, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "addressSet")
        .into_iter()
        .map(attribute_of)
        .find(|attribute| attribute.allocation_id.as_deref() == Some(allocation_id))
}

/// RULE 1: a modify sets the record of exactly the address it names, reports the change as
/// outstanding beside it, and the describe reads the same state back — then the change comes
/// due on its own.
#[tokio::test]
async fn a_modify_sets_the_named_addresss_record_and_reports_the_change() {
    // Rows differing along which address of four is named and which spelling, length and
    // label count of domain name the request sends — including the fully qualified form with
    // the root label's trailing dot, which names the same record as the form without it.
    let longest_label = "a".repeat(MAX_DNS_LABEL_BYTES);
    let rows: Vec<ModifyRow> = vec![
        (
            "the first of four, two labels",
            0,
            "example.com".to_string(),
            "example.com.".to_string(),
        ),
        (
            "one in the middle of four, three labels",
            2,
            "mail.example.com".to_string(),
            "mail.example.com.".to_string(),
        ),
        (
            "the last of four, already fully qualified",
            3,
            "smtp.example.org.".to_string(),
            "smtp.example.org.".to_string(),
        ),
        (
            "a label of the longest length a DNS label may be",
            1,
            format!("{longest_label}.example.net"),
            format!("{longest_label}.example.net."),
        ),
    ];

    for (case, index, sent, expected_record) in rows {
        let router = emulator();
        let mut held: Vec<(String, String)> = Vec::new();
        for position in 0..4 {
            held.push(allocate(&router, &format!("{case}: address {position}")).await);
        }
        let (allocation_id, public_ip) = held[index].clone();

        let (status, body) = modify(&router, &allocation_id, Some(&sent), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = answered(case, &body);
        assert_eq!(
            reported,
            Attribute {
                public_ip: Some(public_ip.clone()),
                allocation_id: Some(allocation_id.clone()),
                ptr_record: Some(expected_record.clone()),
                update_status: Some(PTR_UPDATE_STATUS_PENDING.to_string()),
                update_value: Some(expected_record.clone()),
            },
            "case {case:?}: expected the record set with its change outstanding, \
             got {reported:?} from body {body}"
        );

        // The describe reads the SAME state back, and only this address carries an attribute
        // at all, the other three having none.
        let read_back = described(&router, case, &allocation_id).await;
        assert_eq!(
            read_back,
            Some(Attribute {
                public_ip: Some(public_ip),
                allocation_id: Some(allocation_id.clone()),
                ptr_record: Some(expected_record.clone()),
                update_status: Some(PTR_UPDATE_STATUS_PENDING.to_string()),
                update_value: Some(expected_record.clone()),
            }),
            "case {case:?}: the describe must report what the modify wrote"
        );
        for (other, _) in held.iter().filter(|(id, _)| *id != allocation_id) {
            let reported = described(&router, case, other).await;
            assert_eq!(
                reported, None,
                "case {case:?}: an address nothing modified carries no attribute, \
                 got {reported:?}"
            );
        }

        // The change was owed at its instant whether or not anybody called, so once it has
        // passed the record stands alone with nothing outstanding on it.
        tokio::time::sleep(past_the_change()).await;
        let settled = described(&router, case, &allocation_id).await;
        assert_eq!(
            settled.as_ref().and_then(|attribute| attribute.ptr_record.clone()),
            Some(expected_record.clone()),
            "case {case:?}: expected the record to remain in effect, got {settled:?}"
        );
        assert_eq!(
            settled.as_ref().and_then(|attribute| attribute.update_status.clone()),
            None,
            "case {case:?}: expected nothing outstanding once the change came due, \
             got {settled:?}"
        );
    }
}

/// RULE 1, continued: the two writers of this attribute go through ONE seam, so a reset of an
/// address whose record a modify put in effect takes that record away on the same terms — at
/// once, with the change reported outstanding until it comes due.
#[tokio::test]
async fn a_reset_of_a_record_a_modify_set_takes_it_away_on_the_same_terms() {
    let router = emulator();
    let (allocation_id, public_ip) = allocate(&router, "the address with a record").await;

    // A record IN EFFECT with nothing outstanding on it: what a modify leaves behind once its
    // change has come due.
    let (status, body) = modify(&router, &allocation_id, Some("mail.example.com"), &[]).await;
    assert_eq!(
        status, 200,
        "the modify must be served, got {status} with body {body}"
    );
    tokio::time::sleep(past_the_change()).await;
    let in_effect = described(&router, "the record in effect", &allocation_id).await;
    assert_eq!(
        in_effect,
        Some(Attribute {
            public_ip: Some(public_ip.clone()),
            allocation_id: Some(allocation_id.clone()),
            ptr_record: Some("mail.example.com.".to_string()),
            update_status: None,
            update_value: None,
        }),
        "expected the modify's record in effect with nothing outstanding, got {in_effect:?}"
    );

    // The reset names no new record, so what it leaves in effect is no record at all — and
    // the change it made is reported outstanding beside that until it comes due.
    let (status, body) = call(
        &router,
        REGION,
        "ResetAddressAttribute",
        &[
            p("AllocationId", allocation_id.clone()),
            p("Attribute", ADDRESS_ATTRIBUTE_DOMAIN_NAME),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the reset must be served, got {status} with body {body}"
    );
    let reported = answered("the reset", &body);
    assert_eq!(
        reported,
        Attribute {
            public_ip: Some(public_ip.clone()),
            allocation_id: Some(allocation_id.clone()),
            ptr_record: None,
            update_status: Some(PTR_UPDATE_STATUS_PENDING.to_string()),
            update_value: None,
        },
        "expected the reset to take the record away with its change outstanding, \
         got {reported:?} from body {body}"
    );
    let read_back = described(&router, "the reset", &allocation_id).await;
    assert_eq!(
        read_back.as_ref().and_then(|attribute| attribute.ptr_record.clone()),
        None,
        "the describe must read the same state back, got {read_back:?}"
    );

    // Once the change has come due the address carries no attribute at all, so the report has
    // no row for it.
    tokio::time::sleep(past_the_change()).await;
    let settled = described(&router, "the reset, once due", &allocation_id).await;
    assert_eq!(
        settled, None,
        "expected the address to carry no attribute at all, got {settled:?}"
    );
}

/// RULE 2: one outstanding request per record — a further modify while one is outstanding is
/// refused, the refusal leaves that change alone, another address is unaffected, and the
/// record takes a new request once the first has come due.
#[tokio::test]
async fn a_further_modify_while_one_is_outstanding_is_refused() {
    let router = emulator();
    let (first, _) = allocate(&router, "the address being modified").await;
    let (second, _) = allocate(&router, "another address of the account").await;

    let (status, body) = modify(&router, &first, Some("one.example.com"), &[]).await;
    assert_eq!(
        status, 200,
        "the first modify must be accepted, got {status} with body {body}"
    );

    // Rows differing along how many requests have already been refused, and along whether the
    // second names the same record or another: the refusal must not consume the outstanding
    // change.
    for (case, domain) in [
        ("the second request, the same record", "one.example.com"),
        ("the third request, a different record", "two.example.com"),
    ] {
        let (status, body) = modify(&router, &first, Some(domain), &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 while a change is outstanding, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "PendingRequestAlreadyExists",
            "case {case:?}: expected PendingRequestAlreadyExists, got body {body}"
        );
        let reported = described(&router, case, &first).await;
        assert_eq!(
            reported.as_ref().and_then(|attribute| attribute.ptr_record.clone()),
            Some("one.example.com.".to_string()),
            "case {case:?}: the refusal must leave the first request's record alone, \
             got {reported:?}"
        );
    }

    // The refusal is about ONE record: another address of the account still takes a request.
    let (status, body) = modify(&router, &second, Some("other.example.com"), &[]).await;
    assert_eq!(
        status, 200,
        "a modify of another address must be accepted, got {status} with body {body}"
    );

    // Once the outstanding change has come due the record takes a new request again, and the
    // new name replaces the old one.
    tokio::time::sleep(past_the_change()).await;
    let (status, body) = modify(&router, &first, Some("three.example.com"), &[]).await;
    assert_eq!(
        status, 200,
        "a modify after the outstanding change came due must be accepted, \
         got {status} with body {body}"
    );
    let reported = answered("a modify after the change came due", &body);
    assert_eq!(
        reported.ptr_record,
        Some("three.example.com.".to_string()),
        "the second name must replace the first, got {reported:?}"
    );
}

/// RULE 3: a request that names no record to set, a value that is no domain name, and an id
/// that names no address of the account are each refused, and nothing is written.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_and_writes_nothing() {
    let router = emulator();
    let (held, _) = allocate(&router, "the stored address").await;

    // (case, the allocation id, the domain name, the code the refusal carries)
    let rows: Vec<RefusalRow> = vec![
        ("no domain name at all", held.clone(), None, "MissingParameter"),
        (
            "an empty domain name",
            held.clone(),
            Some(String::new()),
            "MissingParameter",
        ),
        (
            "a single label, which nobody owns",
            held.clone(),
            Some("example".to_string()),
            "InvalidParameterValue",
        ),
        (
            "a label with a character no DNS label carries",
            held.clone(),
            Some("under_score.example.com".to_string()),
            "InvalidParameterValue",
        ),
        (
            "a label one byte longer than a DNS label may be",
            held.clone(),
            Some(format!("{}.example.com", "a".repeat(MAX_DNS_LABEL_BYTES + 1))),
            "InvalidParameterValue",
        ),
        (
            "something that is not an allocation id",
            "not-an-allocation".to_string(),
            Some("example.com".to_string()),
            "InvalidAllocationID.Malformed",
        ),
        (
            "a well-formed id the account holds no address for",
            "eipalloc-0fedcba987654321f".to_string(),
            Some("example.com".to_string()),
            "InvalidElasticIpID.NotFound",
        ),
    ];

    for (case, allocation_id, domain_name, code) in rows {
        let (status, body) = modify(&router, &allocation_id, domain_name.as_deref(), &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got body {body}"
        );
        // The stored address is untouched: no refusal starts a change on it.
        let reported = described(&router, case, &held).await;
        assert_eq!(
            reported, None,
            "case {case:?}: a refusal must leave the address with no attribute, \
             got {reported:?}"
        );
    }
}
