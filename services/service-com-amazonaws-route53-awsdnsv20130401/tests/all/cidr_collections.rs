// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CreateCidrCollection`, the operation that puts a
//! CIDR collection in the account's store.
//!
//! The rules under test:
//!
//! 1. A create PERSISTS a collection and answers with it: an id freshly minted
//!    in the form `UUID` fixes, `Version` 1, the `Name` the request carried, the
//!    collection's own ARN, and a `Location` naming that collection at the
//!    endpoint the request was sent to. The first table spans the name's length
//!    and character classes and the caller reference's length and ASCII
//!    content, because the rule holds for every value those shapes allow — not
//!    merely for a renamed collection.
//! 2. `CallerReference` is the idempotency token, so the SAME request again is
//!    served with the collection the first one created: the same id, version and
//!    ARN, and no second record. That is what makes the answer a report of the
//!    store rather than of the request.
//! 3. A `Name` the account already holds under a DIFFERENT caller reference is
//!    the modelled `CidrCollectionAlreadyExistsException` (HTTP 400), and the
//!    collection already holding the name is left exactly as it was.
//! 4. A value outside a bound the model declares — `CollectionName`'s length and
//!    pattern, `CidrNonce`'s length — is refused before the handler, under the
//!    one input error this service's model declares, and persists nothing.
//! 5. The account's collections are bounded: a create past the bound is the
//!    modelled `LimitsExceeded` (HTTP 400), while a RETRY of a collection the
//!    account already holds is still served, because it asks for no new record.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_CIDR_COLLECTIONS;

use crate::hosted_zone_records::{call, escape_xml, router, Reply, ENDPOINT};

/// `CollectionName` is `len 1..64, pattern ^[0-9A-Za-z_\-]+$`
/// (smithy_query(query="shape",shape="CollectionName")).
const NAME_MAX_LEN: usize = 64;

/// `CidrNonce` is `len 1..64, pattern ^\p{ASCII}+$`
/// (smithy_query(query="shape",shape="CidrNonce")).
const CALLER_REFERENCE_MAX_LEN: usize = 64;

/// The wire error code of the refusal of a `Name` the account already holds
/// under another caller reference (`CidrCollectionAlreadyExistsException`, the
/// shape name the model declares).
const ALREADY_EXISTS: &str = "CidrCollectionAlreadyExistsException";

/// The wire error code of the refusal of a create the account has no room for
/// (`LimitsExceeded`).
const LIMITS_EXCEEDED: &str = "LimitsExceeded";

/// The wire error code this service states every input-level refusal under
/// (`InvalidInput`).
const INVALID_INPUT: &str = "InvalidInput";

/// Create a CIDR collection.
async fn create_collection(router: &MultiServiceRouter, name: &str, caller_reference: &str) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/cidrcollection",
        &format!(
            "<CreateCidrCollectionRequest \
             xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <CallerReference>{}</CallerReference><Name>{}</Name>\
             </CreateCidrCollectionRequest>",
            escape_xml(caller_reference),
            escape_xml(name)
        ),
    )
    .await
}

/// One `CidrCollection` as the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CollectionView {
    id: String,
    name: String,
    version: String,
    arn: String,
}

/// `(Id, Name, Version, Arn)` as a reply carrying a `CidrCollection` reports
/// them.
fn collection_of(reply: &Reply, case: &str) -> CollectionView {
    let pick = |path: &str| {
        reply.first(path).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry Collection/{path}; body {}",
                reply.body
            )
        })
    };
    CollectionView {
        id: pick("Collection/Id").to_string(),
        name: pick("Collection/Name").to_string(),
        version: pick("Collection/Version").to_string(),
        arn: pick("Collection/Arn").to_string(),
    }
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form `UUID`'s pattern fixes
/// (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`), which is the form every
/// operation addressing a collection accepts for its id.
fn is_uuid(id: &str) -> bool {
    let segments: Vec<&str> = id.split('-').collect();
    segments.len() == 5
        && segments
            .iter()
            .map(|segment| segment.len())
            .eq([8, 4, 4, 4, 12])
        && id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// The ARN a collection with id `id` carries: Route 53 names no region in a
/// resource ARN, and the account is the one every ARN this emulator mints
/// carries.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:route53::{}:cidrcollection/{id}",
        engine::emulator_account_id()
    )
}

// ---------------------------------------------------------------------------
// Create, then the same create again
// ---------------------------------------------------------------------------

/// One row of the create table: the request sent, and what makes it its own.
struct CreateRow {
    case: &'static str,
    name: String,
    caller_reference: String,
}

#[tokio::test]
async fn a_created_cidr_collection_is_answered_out_of_the_store() {
    let rows = [
        CreateRow {
            case: "a hyphenated name",
            name: "office-ranges".to_string(),
            caller_reference: "create-office-ranges-1".to_string(),
        },
        CreateRow {
            case: "the shortest name and reference the model allows",
            name: "A".to_string(),
            caller_reference: "x".to_string(),
        },
        CreateRow {
            case: "the longest name and reference the model allows",
            name: "n".repeat(NAME_MAX_LEN),
            caller_reference: "r".repeat(CALLER_REFERENCE_MAX_LEN),
        },
        CreateRow {
            case: "digits and underscores only",
            name: "2026_10_24".to_string(),
            caller_reference: "0".to_string(),
        },
        CreateRow {
            case: "a reference of printable ASCII punctuation and spaces",
            name: "Mixed_Case-42".to_string(),
            caller_reference: "retry #1: {\"attempt\": 1} <&>".to_string(),
        },
    ];

    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for row in &rows {
        let created = create_collection(&router, &row.name, &row.caller_reference).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateCidrCollection, got {} with body {}",
            row.case, created.status, created.body
        );
        let answered = collection_of(&created, row.case);
        assert!(
            is_uuid(&answered.id),
            "case {:?}: expected an 8-4-4-4-12 lowercase-hex collection id, got {:?}",
            row.case,
            answered.id
        );
        let expected = CollectionView {
            id: answered.id.clone(),
            name: row.name.clone(),
            // "A sequential counter that Route 53 sets to 1 when you create a
            // CIDR collection" (`CidrCollection$Version`).
            version: "1".to_string(),
            arn: expected_arn(&answered.id),
        };
        assert_eq!(
            answered, expected,
            "case {:?}: CreateCidrCollection must answer with the collection it persisted, \
             got {answered:?}; body {}",
            row.case, created.body
        );
        assert!(
            !minted.contains(&answered.id),
            "case {:?}: every created collection must get its own id, but {:?} was minted twice",
            row.case,
            answered.id
        );
        minted.push(answered.id.clone());

        // The Location names the collection at the endpoint the request was
        // sent to — the same path the operations addressing a collection are
        // bound to.
        let expected_location = format!(
            "https://{ENDPOINT}/2013-04-01/cidrcollection/{}",
            answered.id
        );
        assert_eq!(
            created.location.as_deref(),
            Some(expected_location.as_str()),
            "case {:?}: expected the Location {expected_location:?}, got {:?}",
            row.case,
            created.location
        );

        // The same request again is the RETRY the caller reference exists for:
        // it is served with the collection the first create persisted, so the
        // answer can only have come out of the store.
        let repeated = create_collection(&router, &row.name, &row.caller_reference).await;
        assert_eq!(
            repeated.status, 201,
            "case {:?}: a repeated create must be served, got {} with body {}",
            row.case, repeated.status, repeated.body
        );
        let again = collection_of(&repeated, row.case);
        assert_eq!(
            again, answered,
            "case {:?}: the repeat must report the stored collection, got {again:?} against \
             {answered:?}; body {}",
            row.case, repeated.body
        );
        assert_eq!(
            repeated.location.as_deref(),
            Some(expected_location.as_str()),
            "case {:?}: the repeat must name the same collection in its Location, got {:?}",
            row.case,
            repeated.location
        );
    }

    // Every row's collection is still addressable by its own name after the
    // later rows were created, so one create did not displace another.
    for (row, id) in rows.iter().zip(&minted) {
        let repeated = create_collection(&router, &row.name, &row.caller_reference).await;
        let again = collection_of(&repeated, row.case);
        assert_eq!(
            &again.id, id,
            "case {:?}: expected the collection the account already holds ({id}), got {:?}",
            row.case, again.id
        );
    }
}

// ---------------------------------------------------------------------------
// A name the account already holds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_held_name_under_another_reference_is_refused_and_changes_nothing() {
    let rows = [
        CreateRow {
            case: "a hyphenated name",
            name: "office-ranges".to_string(),
            caller_reference: "first-create".to_string(),
        },
        CreateRow {
            case: "the shortest name the model allows",
            name: "A".to_string(),
            caller_reference: "a".to_string(),
        },
        CreateRow {
            case: "the longest name the model allows",
            name: "n".repeat(NAME_MAX_LEN),
            caller_reference: "r".repeat(CALLER_REFERENCE_MAX_LEN),
        },
    ];

    // The refusal the model documents for this error class, which names neither
    // the collection nor the reference.
    let expected_message = "A CIDR collection with this name and a different caller reference \
                            already exists in this account.";

    for row in &rows {
        let router = router();
        let created = create_collection(&router, &row.name, &row.caller_reference).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from the first create, got {} with body {}",
            row.case, created.status, created.body
        );
        let before = collection_of(&created, row.case);

        for other_reference in ["a-second-create", &"z".repeat(CALLER_REFERENCE_MAX_LEN)] {
            let refused = create_collection(&router, &row.name, other_reference).await;
            assert_eq!(
                refused.status, 400,
                "case {:?}: expected 400 for the reference {other_reference:?}, got {} with \
                 body {}",
                row.case, refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                ALREADY_EXISTS,
                "case {:?}: expected {ALREADY_EXISTS}, got {:?}; body {}",
                row.case,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.first("Error/Message"),
                Some(expected_message),
                "case {:?}: expected the modelled refusal text, got {:?}",
                row.case,
                refused.first("Error/Message")
            );
        }

        // The stored collection is exactly as it was: the refusals neither
        // replaced it nor moved its version.
        let retried = create_collection(&router, &row.name, &row.caller_reference).await;
        let after = collection_of(&retried, row.case);
        assert_eq!(
            after, before,
            "case {:?}: the held collection must be untouched, got {after:?} against \
             {before:?}; body {}",
            row.case, retried.body
        );
    }
}

// ---------------------------------------------------------------------------
// Values outside the modelled bounds
// ---------------------------------------------------------------------------

/// One row of the refusal table: a request outside a bound the model declares.
struct RefusalRow {
    case: &'static str,
    name: String,
    caller_reference: String,
}

#[tokio::test]
async fn a_value_outside_a_modelled_bound_is_refused_and_persists_nothing() {
    let rows = [
        RefusalRow {
            case: "an empty name",
            name: String::new(),
            caller_reference: "reference".to_string(),
        },
        RefusalRow {
            case: "a name one character past the length bound",
            name: "n".repeat(NAME_MAX_LEN + 1),
            caller_reference: "reference".to_string(),
        },
        RefusalRow {
            case: "a name carrying a character the pattern excludes",
            name: "office ranges".to_string(),
            caller_reference: "reference".to_string(),
        },
        RefusalRow {
            case: "a name carrying punctuation the pattern excludes",
            name: "office.ranges!".to_string(),
            caller_reference: "reference".to_string(),
        },
        RefusalRow {
            case: "an empty caller reference",
            name: "office-ranges".to_string(),
            caller_reference: String::new(),
        },
        RefusalRow {
            case: "a caller reference one character past the length bound",
            name: "office-ranges".to_string(),
            caller_reference: "r".repeat(CALLER_REFERENCE_MAX_LEN + 1),
        },
    ];

    for row in &rows {
        let router = router();
        let refused = create_collection(&router, &row.name, &row.caller_reference).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected 400, got {} with body {}",
            row.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            INVALID_INPUT,
            "case {:?}: expected {INVALID_INPUT}, got {:?}; body {}",
            row.case,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.first("Collection/Id").is_none(),
            "case {:?}: a refused create must answer with no collection; body {}",
            row.case,
            refused.body
        );

        // Nothing was persisted: the name the refused request carried is still
        // free, so a well-formed create for it is a FIRST create — version 1
        // under a fresh id.
        let created = create_collection(&router, "office-ranges", "after-the-refusal").await;
        assert_eq!(
            created.status, 201,
            "case {:?}: the create after the refusal must be served, got {} with body {}",
            row.case, created.status, created.body
        );
        let answered = collection_of(&created, row.case);
        assert_eq!(
            answered.version, "1",
            "case {:?}: expected Version 1 for a first create, got {:?}",
            row.case, answered.version
        );
    }
}

// ---------------------------------------------------------------------------
// The account's bound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_past_the_accounts_bound_is_refused_while_a_retry_is_still_served() {
    let router = router();
    let mut first_id = String::new();

    for index in 0..MAX_CIDR_COLLECTIONS {
        let name = format!("collection-{index}");
        let created = create_collection(&router, &name, &format!("create-{index}")).await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected 201 while the account has room, got {} with body {}",
            created.status, created.body
        );
        if index == 0 {
            first_id = collection_of(&created, &name).id;
        }
    }

    let name = format!("collection-{MAX_CIDR_COLLECTIONS}");
    let refused = create_collection(&router, &name, "one-too-many").await;
    assert_eq!(
        refused.status, 400,
        "case {name:?}: expected 400 past the bound, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        LIMITS_EXCEEDED,
        "case {name:?}: expected {LIMITS_EXCEEDED}, got {:?}; body {}",
        refused.error_code(),
        refused.body
    );

    // A retry asks for no new record, so the bound does not apply to it.
    let retried = create_collection(&router, "collection-0", "create-0").await;
    assert_eq!(
        retried.status, 201,
        "case \"retry at the bound\": expected 201, got {} with body {}",
        retried.status, retried.body
    );
    let again = collection_of(&retried, "retry at the bound");
    assert_eq!(
        again.id, first_id,
        "case \"retry at the bound\": expected the collection already held ({first_id}), got {:?}",
        again.id
    );
}
