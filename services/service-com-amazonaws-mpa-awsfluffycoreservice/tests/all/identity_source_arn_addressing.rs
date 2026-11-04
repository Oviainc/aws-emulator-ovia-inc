// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an `IdentitySourceArn` ADDRESSES, over BOTH operations that
//! take one in their URI label: `DeleteIdentitySource` and `GetIdentitySource`.
//!
//! The rule under test: a request member documented to carry the Amazon Resource Name of
//! one kind of resource addresses only ARNs of that kind, so a value that is not an ARN
//! this service could have minted for that kind names no such resource whatever the store
//! holds and is refused with the sentence that names the member it rejected
//! (`ValidationException`, 400, [`state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE`]) — before
//! the account boundary and before any lookup — whichever operation reads the member. Neither the
//! member's modelled constraints (`String`, `@length(max: 1000)`, no `@pattern`, from
//! `smithy_query(query="shape",shape="DeleteIdentitySourceRequest,GetIdentitySourceRequest")`)
//! nor the operation's own error set changes which values address a resource: the two
//! operations part only AFTER the address resolves, where the `@idempotent` delete (which
//! models no `ResourceNotFoundException`) answers an identity-source ARN it holds nothing
//! under a success that writes nothing and the read answers it `ResourceNotFoundException`
//! (404).
//!
//! The tables span the class the rule generalises over: which part of the grammar
//! `arn:<partition>:mpa:<region>:<account>:identity-source/<id>` the value misses, which
//! service namespace and which resource kind it names, the partition, the region, the form
//! of the resource id, the account it names, and the length of the value up to the
//! member's published bound. Where a row names a published grammar or bound, a comment
//! names the modelled member it comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the
//! `DELETE /identity-sources/{IdentitySourceArn}` and
//! `GET /identity-sources/{IdentitySourceArn}` routes, the URI label, the error envelope
//! and the HTTP status are all exercised.

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};
use crate::identity_source_deletion::{create_source, delete_source, listed_status, stored};

/// The two operations whose `IdentitySourceArn` addresses an identity source, each as the
/// request it sends for a value.
#[derive(Clone, Copy, Debug)]
enum Op {
    Delete,
    Get,
}

impl Op {
    const ALL: [Self; 2] = [Self::Delete, Self::Get];

    async fn call(self, harness: &Harness, value: &str) -> (u16, Value) {
        match self {
            Self::Delete => delete_source(harness, value).await,
            Self::Get => {
                harness
                    .call(
                        "GET",
                        &format!("/identity-sources/{}", encode_label(value)),
                        None,
                    )
                    .await
            }
        }
    }
}

/// An identity-source ARN of this service, in the grammar every regional, account-owned
/// ARN of this service takes — the one `ApprovalTeamArn`'s own pattern spells
/// (`^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$`),
/// with the resource name the `IdentitySource` resource's `arn` template carries.
fn source_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:mpa:{region}:{account}:identity-source/{id}")
}

/// Values that address NO identity source of this service. Rows differ along which part of
/// the grammar the value misses, which service namespace and resource kind it names, and
/// how long the value is.
fn addresses_no_identity_source() -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        ("no colons at all", "not-an-arn".to_string()),
        (
            "an identity source's id, not its ARN",
            "ids-000000000001".to_string(),
        ),
        (
            "the arn prefix misspelled",
            format!("srn:aws:mpa:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        ("too few fields", "arn:aws:mpa".to_string()),
        (
            "five fields, no resource",
            format!("arn:aws:mpa:{REGION}:{account}"),
        ),
        (
            "an empty resource",
            format!("arn:aws:mpa:{REGION}:{account}:"),
        ),
        (
            "an empty partition",
            source_arn("", REGION, account, "ids-000000000001"),
        ),
        (
            // `aws(-[^:]+)?` is the partition segment of this service's ARN patterns.
            "a partition outside the aws family",
            source_arn("not-aws", REGION, account, "ids-000000000001"),
        ),
        (
            "an empty service namespace",
            format!("arn:aws::{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            "another service's namespace",
            format!("arn:aws:sso:{REGION}:{account}:identity-source/ids-000000000001"),
        ),
        (
            // `[a-z0-9-]{1,20}` in the same pattern, so an empty region is no ARN of this
            // service.
            "no region",
            source_arn("aws", "", account, "ids-000000000001"),
        ),
        (
            // `[0-9]{12}` in the same pattern.
            "an account that is not twelve digits",
            source_arn("aws", REGION, "12345", "ids-000000000001"),
        ),
        (
            "no account",
            source_arn("aws", REGION, "", "ids-000000000001"),
        ),
        (
            // `ApprovalTeamArn`: another resource kind of this same service.
            "an approval team's ARN",
            format!("arn:aws:mpa:{REGION}:{account}:approval-team/some-team"),
        ),
        (
            // `SessionArn`: a third resource kind of this same service.
            "an approval session's ARN",
            format!("arn:aws:mpa:{REGION}:{account}:session/some-team/session-000000000001"),
        ),
        (
            // `QualifiedPolicyArn`: an AWS-owned, global resource of this service.
            "an AWS-managed MPA policy's ARN",
            "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT".to_string(),
        ),
        (
            // The IAM Identity Center instance a source is associated with is not the
            // identity source: `IdcInstanceArn`'s own pattern, not this service's.
            "the IAM Identity Center instance's ARN",
            "arn:aws:sso:::instance/ssoins-1234567890abcdef".to_string(),
        ),
        (
            "the resource kind with no id",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source"),
        ),
        (
            "the resource kind with an empty id",
            source_arn("aws", REGION, account, ""),
        ),
        (
            // `[a-zA-Z0-9._-]+` is the resource-id class this service's ARN patterns
            // declare, so a tilde is not one of its characters.
            "a resource id outside the id character class",
            source_arn("aws", REGION, account, "ids~1"),
        ),
        (
            // An identity-source id is minted by this service, so a value outside that
            // form is no id this service could have issued (`identity_source_id_form.rs`
            // tables the form itself).
            "a resource id outside the form this service mints ids in",
            source_arn("aws", REGION, account, "0f5e0119-ab78-418e-b630-3b421ab2fd6f"),
        ),
        (
            "a resource id spanning the resource-id character class",
            source_arn("aws", REGION, account, "Ids.000_000-9"),
        ),
        (
            // `@length(max: 1000)`: the longest value a conforming request can carry.
            "a value at the member's maximum that is no ARN",
            "x".repeat(1000),
        ),
    ]
}

/// Values that ARE identity-source ARNs of this service in the caller's own account, under
/// which this store holds no identity source. Rows differ along the partition, the region,
/// the form of the resource id and the length of the ARN.
fn addresses_an_absent_identity_source() -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        (
            "an id this account never minted",
            source_arn("aws", REGION, account, "ids-000000009999"),
        ),
        (
            "a region nothing was created in",
            source_arn("aws", "eu-west-1", account, "ids-000000000001"),
        ),
        (
            // `aws(-[^:]+)?` admits every suffixed partition.
            "the China partition",
            source_arn("aws-cn", "cn-north-1", account, "ids-000000000001"),
        ),
        (
            "the GovCloud partition",
            source_arn("aws-us-gov", "us-gov-west-1", account, "ids-000000000001"),
        ),
        (
            // The body of a minted id is letters and digits.
            "an id whose body carries letters",
            source_arn("aws", REGION, account, "ids-a1b2c3"),
        ),
        (
            // `@length(max: 1000)`: an ARN padded in its id to the member's maximum.
            "an ARN at the member's maximum length",
            {
                let base = source_arn("aws", REGION, account, "ids-");
                format!("{base}{}", "0".repeat(1000 - base.len()))
            },
        ),
    ]
}

/// A value that addresses no identity source earns the malformed-ARN class from EVERY
/// operation that reads an `IdentitySourceArn` — the same status, code and sentence — and
/// nothing in the store moves: no source is removed, none is recorded, and no refusal is
/// reported as a delete that was carried out.
#[tokio::test]
async fn a_value_addressing_no_identity_source_is_refused_as_malformed_by_every_operation() {
    let harness = Harness::new();
    let bystander = create_source(&harness, 1, 0).await;

    for (case, value) in addresses_no_identity_source() {
        for op in Op::ALL {
            let (status, body) = op.call(&harness, &value).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {op:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} on {op:?}: expected error code ValidationException, got {body:?}"
            );
            assert_eq!(
                text(&body, "Message"),
                state::INVALID_IDENTITY_SOURCE_ARN_MESSAGE,
                "case {case:?} on {op:?}: expected the member-named refusal, got {body:?}"
            );
            assert_eq!(
                stored(&harness, &value),
                None,
                "case {case:?} on {op:?}: the refused call recorded an identity source"
            );
        }

        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {case:?}: a refused call removed the caller's own identity source"
        );
    }

    let store = state::service_state(&*harness.engine).expect("the service store");
    let remaining = store.lock().identity_sources.len();
    assert_eq!(
        remaining, 1,
        "the store holds {remaining} source(s), expected only the bystander"
    );
}

/// An identity-source ARN this store holds nothing under passes the addressing screen and
/// reaches each operation's OWN answer for an absent resource: the `@idempotent` delete
/// succeeds and writes nothing, the read is `ResourceNotFoundException` (404).
#[tokio::test]
async fn an_identity_source_arn_holding_nothing_reaches_each_operations_own_answer() {
    let harness = Harness::new();
    let bystander = create_source(&harness, 1, 0).await;

    for (case, value) in addresses_an_absent_identity_source() {
        let (status, read) = Op::Get.call(&harness, &value).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from the read, got {status} with {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {read:?}"
        );
        assert_eq!(
            text(&read, "Message"),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the not-found message, got {read:?}"
        );

        let (status, deleted) = Op::Delete.call(&harness, &value).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} with {deleted:?}"
        );
        assert_eq!(
            stored(&harness, &value),
            None,
            "case {case:?}: the no-op delete recorded an identity source"
        );
        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {case:?}: the no-op delete removed the caller's own identity source"
        );
    }
}

/// The addressing screen is decided BEFORE the account boundary: an identity-source ARN of
/// another account is the boundary's 403 on both operations, while a value that addresses
/// no identity source is the 400 above even when its fifth field is the caller's own
/// account.
#[tokio::test]
async fn the_addressing_screen_is_decided_before_the_account_boundary() {
    let harness = Harness::new();
    let own_account = engine::emulator_account_id();

    // (case, the ARN asked for) — identity-source ARNs of this service that name an
    // account the caller does not own, so the boundary is what refuses them.
    let foreign = [
        (
            "another account",
            source_arn("aws", REGION, "999999999999", "ids-000000000001"),
        ),
        (
            "an account one digit from the caller's",
            source_arn("aws", REGION, "000000000001", "ids-000000000001"),
        ),
        (
            "another account in another region",
            source_arn("aws", "eu-west-1", "999999999999", "ids-000000000002"),
        ),
    ];
    for (case, value) in foreign {
        for op in Op::ALL {
            let (status, body) = op.call(&harness, &value).await;
            assert_eq!(
                status, 403,
                "case {case:?} on {op:?}: expected HTTP 403, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?} on {op:?}: expected error code AccessDeniedException, got {body:?}"
            );
        }
    }

    // The same wrong-kind values, in the caller's OWN account: the boundary has nothing to
    // refuse, and the addressing screen answers 400 rather than letting the value through.
    let wrong_kind = [
        (
            "an approval team's ARN in the caller's account",
            format!("arn:aws:mpa:{REGION}:{own_account}:approval-team/some-team"),
        ),
        (
            "another service's namespace in the caller's account",
            format!("arn:aws:sso:{REGION}:{own_account}:identity-source/ids-000000000001"),
        ),
    ];
    for (case, value) in wrong_kind {
        for op in Op::ALL {
            let (status, body) = op.call(&harness, &value).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {op:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} on {op:?}: expected error code ValidationException, got {body:?}"
            );
        }
    }
}

/// The stricter screen refuses only values that address no identity source: an ARN this
/// service minted still reads back and still deletes, the delete reported on the record the
/// read reports.
#[tokio::test]
async fn an_arn_this_service_minted_still_reads_back_and_still_deletes() {
    let harness = Harness::new();
    let arn = create_source(&harness, 1, 0).await;

    let (status, before) = Op::Get.call(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "reading the minted source returned {status} with {before:?}"
    );
    assert_eq!(
        text(&before, "IdentitySourceArn"),
        arn,
        "the read reported another source: {before:?}"
    );

    let (status, deleted) = Op::Delete.call(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "deleting the minted source returned {status} with {deleted:?}"
    );

    // The removal is CLOCK-OWED, so straight after the call the record is reported in the
    // non-terminal status the model declares for it.
    let (status, after) = Op::Get.call(&harness, &arn).await;
    assert_eq!(
        status, 200,
        "re-reading the deleted source returned {status} with {after:?}"
    );
    assert_eq!(
        text(&after, "Status"),
        state::IdentitySourceStatus::Deleting.as_wire(),
        "expected the source reported DELETING, got {after:?}"
    );
}
