// SPDX-License-Identifier: CC0-1.0

//! What `ResourceNotFoundException$Message` says when a reference does not resolve.
//!
//! The rule: it is ONE fixed service text for every reference under an `OutpostId` that
//! does not resolve — the same text whichever branch of `OutpostId`'s
//! `^(op-[a-f0-9]{17}|\d{12}|ec2)$` the request used, whichever operation was called and
//! whichever resource under the Outpost (the Outpost itself or an endpoint on it) failed
//! to resolve. It is never built from the request's own ids, and it is never an
//! authorization text: this service models `AccessDeniedException` (403) separately, so a
//! 404 reports a missing or unprovisioned Outpost.
//!
//! Every case here goes through the router, so each exercises routing, deserialization,
//! boundary validation, the handler and serialization — the path a client takes. The
//! expected text is written out here rather than imported from the crate, so a wrong
//! constant in the service cannot make these cases pass.

use serde_json::{json, Value};

use crate::endpoint_lifecycle::{
    create, delete_endpoint, emulator, endpoint_id_of, list_shared, minimal_create,
    owned_outposts, shared_outpost, string_at, Answer,
};

/// The one text an unresolvable reference under an `OutpostId` answers with. Recorded AWS
/// behaviour: the model states no message for `ResourceNotFoundException`.
const OUTPOST_MISSING: &str =
    "The specified outpost does not exist, or does not have S3 capacity provisioned on it.";

/// The authorization wording a 404 must never carry: an authorization failure is this
/// service's `AccessDeniedException` (403), per
/// `smithy_query(query="operation", operation="CreateEndpoint")`, which lists both errors.
const AUTHORIZATION_TEXT: &str = "Access Denied";

/// Every branch of `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`. Source of the
/// catalogue: `smithy_query(query="shape", shape="OutpostId")`. None of these ids is in
/// the store: the provisioned Outposts are discovered through the API by the cases below.
///
/// Rows differ along what the rule generalises over — the branch of the pattern, and
/// within a branch the actual characters, including the `ec2` branch the issue did not
/// cite.
const UNRESOLVABLE: &[(&str, &str)] = &[
    ("op- branch, all f", "op-fffffffffffffffff"),
    ("op- branch, all zero", "op-00000000000000000"),
    ("op- branch, mixed hex", "op-0123456789abcdef0"),
    ("ec2 branch", "ec2"),
    ("12-digit branch", "111122223333"),
    ("12-digit branch, all nine", "999999999999"),
    ("12-digit branch, all zero", "000000000000"),
];

/// Well-formed `EndpointId`s (`^[a-zA-Z0-9]{19}$`) this emulator never issued: upper case,
/// digits and lower case, so no case depends on the characters in the id.
const NEVER_ISSUED_ENDPOINT_IDS: &[&str] = &[
    "A1B2C3D4E5F6G7H8I9J",
    "B1C2D3E4F5G6H7I8J9K",
    "0000000000000000000",
    "zzzzzzzzzzzzzzzzzzz",
];

/// The modelled 404 with exactly the one fixed text as its Message, with nothing of the
/// request in it and no authorization wording.
fn assert_not_found(case: &str, answer: &Answer, request_ids: &[&str]) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected 404/ResourceNotFoundException, got {}/{:?} with {:?}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    assert_eq!(
        answer.message(),
        OUTPOST_MISSING,
        "case {case:?}: expected Message {OUTPOST_MISSING:?}, got {:?}",
        answer.message()
    );
    assert!(
        !answer.message().contains(AUTHORIZATION_TEXT),
        "case {case:?}: a not-found answer must not report an authorization failure, got {:?}",
        answer.message()
    );
    for id in request_ids {
        assert!(
            !answer.message().contains(id),
            "case {case:?}: the Message must not disclose the requested id {id:?}, got {:?}",
            answer.message()
        );
    }
}

/// Create requests that differ in every member beyond `OutpostId`: both widths
/// `SubnetId` / `SecurityGroupId` admit, each `EndpointAccessType` value, whether the
/// access type is named at all and whether a CoIP pool is carried. The not-found answer
/// may not vary with any of them.
fn create_variants(outpost_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("only the required members", minimal_create(outpost_id)),
        (
            "the 8-hex-digit subnet and security group widths",
            json!({
                "OutpostId": outpost_id,
                "SubnetId": "subnet-aaaaaaaa",
                "SecurityGroupId": "sg-aaaaaaaa",
            }),
        ),
        (
            "a CoIP pool with no access type",
            json!({
                "OutpostId": outpost_id,
                "SubnetId": "subnet-00000000",
                "SecurityGroupId": "sg-00000000",
                "CustomerOwnedIpv4Pool": "ipv4pool-coip-00000000000000000",
            }),
        ),
        (
            "AccessType CustomerOwnedIp with its pool",
            json!({
                "OutpostId": outpost_id,
                "SubnetId": "subnet-0a1b2c3d4e5f60718",
                "SecurityGroupId": "sg-0a1b2c3d4e5f60718",
                "AccessType": "CustomerOwnedIp",
                "CustomerOwnedIpv4Pool": "ipv4pool-coip-0a1b2c3d4e5f60718",
            }),
        ),
        (
            "AccessType Private",
            json!({
                "OutpostId": outpost_id,
                "SubnetId": "subnet-00000000000000002",
                "SecurityGroupId": "sg-00000000000000002",
                "AccessType": "Private",
            }),
        ),
    ]
}

/// An `OutpostId` no Outpost resolves for answers the one fixed text on every operation
/// that takes one, for every branch of the id's pattern and whatever else the request
/// carries — and the bodies are byte-identical, not merely the same Message.
#[tokio::test]
async fn an_unresolvable_outpost_answers_one_fixed_text_for_every_id_form() {
    let router = emulator();
    let mut bodies: Vec<(String, Value)> = Vec::new();

    for (form, outpost_id) in UNRESOLVABLE {
        // CreateEndpoint: the answer is the same for every other member combination.
        for (variant, request) in create_variants(outpost_id) {
            let case = format!("CreateEndpoint on {form} with {variant}");
            let answer = create(&router, request).await;
            assert_not_found(&case, &answer, &[outpost_id]);
            bodies.push((case, answer.body));
        }

        // ListSharedEndpoints: with and without a page size.
        for query in ["", "&maxResults=100", "&maxResults=1"] {
            let case = format!("ListSharedEndpoints on {form} with {query:?}");
            let answer = list_shared(&router, &format!("?outpostId={outpost_id}{query}")).await;
            assert_not_found(&case, &answer, &[outpost_id]);
            bodies.push((case, answer.body));
        }

        // DeleteEndpoint: the Outpost is resolved first, so its text wins over any
        // endpoint-level one — for every well-formed endpoint id.
        for endpoint_id in NEVER_ISSUED_ENDPOINT_IDS {
            let case = format!("DeleteEndpoint on {form} with endpoint {endpoint_id}");
            let answer = delete_endpoint(&router, outpost_id, endpoint_id).await;
            assert_not_found(&case, &answer, &[outpost_id, endpoint_id]);
            bodies.push((case, answer.body));
        }
    }

    // One body for every branch of the pattern, every operation and every request shape:
    // nothing in the answer is keyed to the form of the id.
    let (first_case, first_body) = &bodies[0];
    for (case, body) in &bodies[1..] {
        assert_eq!(
            body, first_body,
            "case {case:?}: the body must be identical to {first_case:?}, got {body:?}"
        );
    }
}

/// An endpoint reference that does not resolve under an Outpost that DOES resolve carries
/// the same fixed text as the Outpost's own failure: the service reports neither which
/// half of the pair failed nor the ids it was given.
#[tokio::test]
async fn an_unresolvable_endpoint_under_a_resolvable_outpost_carries_the_same_text() {
    let router = emulator();
    let owned = owned_outposts(&router).await;
    assert!(
        owned.len() >= 2,
        "this case needs two Outposts of this account, got {owned:?}"
    );
    let shared = shared_outpost(&router).await;

    // One endpoint of this account, and one the sharing account owns.
    let created = create(&router, minimal_create(&owned[0])).await;
    assert_eq!(
        created.status, 200,
        "the setup create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let mine = endpoint_id_of(&string_at(&created.body, "EndpointArn"));
    let listing = list_shared(&router, &format!("?outpostId={shared}&maxResults=100")).await;
    let others = endpoint_id_of(&string_at(
        listing
            .endpoints()
            .first()
            .unwrap_or_else(|| panic!("the shared Outpost carries an endpoint: {:?}", listing.body)),
        "EndpointArn",
    ));

    let cases = [
        (
            "the Outpost resolves, the endpoint was never issued",
            owned[0].clone(),
            NEVER_ISSUED_ENDPOINT_IDS[0].to_string(),
        ),
        (
            "the endpoint exists, but on another Outpost of this account",
            owned[1].clone(),
            mine.clone(),
        ),
        (
            "the endpoint exists and another account owns it",
            shared.clone(),
            others,
        ),
    ];
    for (case, outpost_id, endpoint_id) in &cases {
        let answer = delete_endpoint(&router, outpost_id, endpoint_id).await;
        assert_not_found(case, &answer, &[outpost_id, endpoint_id]);
    }

    // And a delete that did remove a record: the repeat is the same fixed text.
    let deleted = delete_endpoint(&router, &owned[0], &mine).await;
    assert_eq!(
        deleted.status, 200,
        "deleting an endpoint of this account must succeed, got {} with {:?}",
        deleted.status, deleted.body
    );
    let repeated = delete_endpoint(&router, &owned[0], &mine).await;
    assert_not_found(
        "a second delete of the same endpoint",
        &repeated,
        &[owned[0].as_str(), mine.as_str()],
    );
}

/// The boundary still refuses an id the model's pattern does not admit, BEFORE anything is
/// resolved: that is a 400 `ValidationException` naming the member and its constraint, not
/// the not-found text.
#[tokio::test]
async fn a_malformed_id_is_still_a_validation_error() {
    let router = emulator();

    let cases: Vec<(&str, &str, Answer)> = vec![
        (
            "CreateEndpoint with a malformed OutpostId",
            "not-a-valid-outpost-id",
            create(
                &router,
                json!({
                    "OutpostId": "not-a-valid-outpost-id",
                    "SubnetId": "subnet-00000000",
                    "SecurityGroupId": "sg-00000000",
                }),
            )
            .await,
        ),
        (
            "CreateEndpoint with a malformed SubnetId under an unresolvable Outpost",
            "bad-subnet-id",
            create(
                &router,
                json!({
                    "OutpostId": "999999999999",
                    "SubnetId": "bad-subnet-id",
                    "SecurityGroupId": "sg-00000000",
                }),
            )
            .await,
        ),
        (
            "CreateEndpoint with a malformed SecurityGroupId under an unresolvable Outpost",
            "bad-sg-id",
            create(
                &router,
                json!({
                    "OutpostId": "op-00000000000000000",
                    "SubnetId": "subnet-00000000",
                    "SecurityGroupId": "bad-sg-id",
                }),
            )
            .await,
        ),
        (
            "DeleteEndpoint with a malformed EndpointId",
            "short-id",
            delete_endpoint(&router, "op-00000000000000000", "short-id").await,
        ),
        (
            "ListSharedEndpoints with a malformed OutpostId",
            "not-a-valid-outpost-id",
            list_shared(&router, "?outpostId=not-a-valid-outpost-id").await,
        ),
    ];

    for (case, value, answer) in &cases {
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case {case:?}: expected 400/ValidationException, got {}/{:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        let expected_opening = format!("1 validation error detected: Value '{value}' at '");
        assert!(
            answer.message().starts_with(&expected_opening),
            "case {case:?}: expected a message opening {expected_opening:?}, got {:?} in {:?}",
            answer.message(),
            answer.body
        );
        assert!(
            answer
                .message()
                .contains("failed to satisfy constraint: Member must satisfy regular expression \
                           pattern: "),
            "case {case:?}: the message must name the pattern constraint, got {:?}",
            answer.message()
        );
        for text in [OUTPOST_MISSING, AUTHORIZATION_TEXT] {
            assert!(
                !answer.message().contains(text),
                "case {case:?}: a malformed id is not a not-found answer, got {:?}",
                answer.message()
            );
        }
    }
}
