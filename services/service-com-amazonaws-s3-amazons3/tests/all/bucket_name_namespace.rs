// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the bucket NAMESPACE a general purpose bucket name lives in.
//!
//! The rule, from `CreateBucket`'s own documentation: "General purpose buckets exist in a
//! global namespace, which means that each bucket name must be unique across all Amazon
//! Web Services accounts in all the Amazon Web Services Regions within a partition." So
//! there is no Region left in which a name an account already holds can be minted a second
//! time: such a create is refused with `BucketAlreadyOwnedByYou` (HTTP 409 per the model)
//! and the existing bucket keeps the Region it was created in.
//!
//! The ONE documented exception is a re-create of the bucket as it stands: "For legacy
//! compatibility, if you re-create an existing bucket that you already own in the North
//! Virginia Region, Amazon S3 returns 200 OK"
//! (`smithy_query(query="shape",shape="BucketAlreadyOwnedByYou")`) — the Region the create
//! asks for has to be the one the bucket already lives in, and that Region has to be
//! `us-east-1`.
//!
//! The table below spans both sides of that split along what the rule generalises over:
//! which Region the bucket was created in, which Region the second create asks for, and
//! whether the two are the same. Every constraint value is one of
//! `BucketLocationConstraint`'s enum values
//! (`smithy_query(query="shape",shape="BucketLocationConstraint")`), where `us-east-1` is
//! deliberately absent — an absent constraint is the only way to ask for it — and `EU` is
//! the legacy alias for `eu-west-1`.

use super::bucket_lifecycle::{create_bucket, head_bucket, router};
use super::bucket_location::{get_bucket_location, root_element};

/// One row: the case name, the constraint the FIRST create carried, the constraint the
/// SECOND create carried, the status the second create must answer, the error code it must
/// carry (none for the legacy 200), and the Region the bucket must still be in afterwards.
type ReuseRow<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    u16,
    Option<&'a str>,
    &'a str,
);

#[tokio::test]
async fn a_name_an_account_holds_cannot_be_minted_in_another_region() {
    let router = router();

    let rows: [ReuseRow<'_>; 7] = [
        (
            "re-creating a us-east-1 bucket in us-east-1 is the documented legacy 200",
            None,
            None,
            200,
            None,
            "us-east-1",
        ),
        (
            "a us-east-1 bucket cannot be re-minted in another Region",
            None,
            Some("us-west-2"),
            409,
            Some("BucketAlreadyOwnedByYou"),
            "us-east-1",
        ),
        (
            "a bucket elsewhere cannot be re-minted in us-east-1 either",
            Some("us-west-2"),
            None,
            409,
            Some("BucketAlreadyOwnedByYou"),
            "us-west-2",
        ),
        (
            "the legacy EU alias bucket cannot be re-minted in a Region the issue never \
             cited",
            Some("EU"),
            Some("ap-southeast-7"),
            409,
            Some("BucketAlreadyOwnedByYou"),
            "eu-west-1",
        ),
        (
            "neither Region being us-east-1 changes nothing: still refused, still where it \
             was",
            Some("ap-northeast-3"),
            Some("us-gov-east-1"),
            409,
            Some("BucketAlreadyOwnedByYou"),
            "ap-northeast-3",
        ),
        (
            "asking for the very Region the bucket is in is refused when it is not \
             us-east-1",
            Some("us-west-1"),
            Some("us-west-1"),
            409,
            Some("BucketAlreadyOwnedByYou"),
            "us-west-1",
        ),
        (
            "the EU alias and eu-west-1 name one Region, and it is not us-east-1",
            Some("EU"),
            Some("eu-west-1"),
            409,
            Some("BucketAlreadyOwnedByYou"),
            "eu-west-1",
        ),
    ];

    for (index, (case, first_constraint, second_constraint, expected_status, expected_code, expected_region)) in
        rows.iter().enumerate()
    {
        let bucket = format!("namespace-bucket-{index}");

        let first = create_bucket(&router, &bucket, *first_constraint, None).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create of {bucket:?} must succeed, got {} with body \
             {:?}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );

        let second = create_bucket(&router, &bucket, *second_constraint, None).await;
        assert_eq!(
            second.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from the second create, got {} \
             with body {:?}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        match expected_code {
            Some(code) => assert_eq!(
                second.error_code(),
                *code,
                "case {case:?}: expected error code {code:?}, got {:?}",
                second.error_code()
            ),
            None => assert_eq!(
                second.header("location").as_deref(),
                Some(format!("/{bucket}").as_str()),
                "case {case:?}: the legacy 200 still reports the bucket's Location, got {:?}",
                second.header("location")
            ),
        }

        // The bucket stayed where it was: both readers of a bucket's Region still report
        // the Region of the FIRST create, so nothing moved the bucket and nothing minted a
        // second one somewhere else.
        let head = head_bucket(&router, &bucket).await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: expected HTTP 200 from HeadBucket after the second create, got \
             {}",
            head.status
        );
        assert_eq!(
            head.header("x-amz-bucket-region").as_deref(),
            Some(*expected_region),
            "case {case:?}: expected HeadBucket to still report {expected_region:?}, got \
             {:?}",
            head.header("x-amz-bucket-region")
        );

        let location = get_bucket_location(&router, &bucket).await;
        assert_eq!(
            location.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketLocation, got {} with body {:?}",
            location.status,
            String::from_utf8_lossy(&location.body)
        );
        // "Buckets in Region us-east-1 have a LocationConstraint of null"
        // (`GetBucketLocationOutput$LocationConstraint`), so that one Region reports no
        // text at all and every other reports its own code.
        let (_, reported) = root_element(&location);
        let expected_constraint = if *expected_region == "us-east-1" {
            ""
        } else {
            expected_region
        };
        assert_eq!(
            reported, expected_constraint,
            "case {case:?}: expected the location document to report \
             {expected_constraint:?}, got {reported:?} in body {:?}",
            String::from_utf8_lossy(&location.body)
        );
    }
}

#[tokio::test]
async fn the_refused_reuse_leaves_the_bucket_and_its_objects_alone() {
    // A refused create writes NOTHING: the bucket it did not re-mint still holds the
    // object that was put in it, so the refusal cannot be a delete-and-recreate.
    let router = router();
    let created = create_bucket(&router, "namespace-objects", Some("us-west-2"), None).await;
    assert_eq!(
        created.status, 200,
        "the first create must succeed, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    let put = super::bucket_lifecycle::send(
        &router,
        "PUT",
        "/namespace-objects/keep.txt",
        &[],
        "payload",
    )
    .await;
    assert_eq!(
        put.status, 200,
        "the object write must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    for (case, constraint) in [
        ("another Region", Some("eu-central-1")),
        ("us-east-1 by omission", None),
        ("the Region it is already in", Some("us-west-2")),
    ] {
        let reused = create_bucket(&router, "namespace-objects", constraint, None).await;
        assert_eq!(
            reused.status, 409,
            "case {case:?}: expected HTTP 409 from the reuse, got {} with body {:?}",
            reused.status,
            String::from_utf8_lossy(&reused.body)
        );
        assert_eq!(
            reused.error_code(),
            "BucketAlreadyOwnedByYou",
            "case {case:?}: expected error code BucketAlreadyOwnedByYou, got {:?}",
            reused.error_code()
        );

        let got = super::bucket_lifecycle::send(
            &router,
            "GET",
            "/namespace-objects/keep.txt",
            &[],
            "",
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: the object must survive the refused reuse, got {} with body {:?}",
            got.status,
            String::from_utf8_lossy(&got.body)
        );
        assert_eq!(
            String::from_utf8_lossy(&got.body),
            "payload",
            "case {case:?}: expected the stored bytes back, got {:?}",
            String::from_utf8_lossy(&got.body)
        );
    }
}
