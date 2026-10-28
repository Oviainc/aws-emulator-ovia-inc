// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetImageAncestry`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=GetImageAncestry`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation WALKS THE PARENT LINK OF THE STORED IMAGES:
//! "a list of entries in the AMI ancestry chain, from the specified AMI to the root AMI"
//! (`GetImageAncestryResult$ImageAncestryEntries`), one entry per AMI, each read off that
//! AMI's own stored record — its id, its creation date, its owner alias, and "the ID of the
//! source AMI from which the AMI was created" (`Image$SourceImageId`) with that parent's
//! Region — until an AMI that carries no parent, which is the root.
//!
//! So the tables below span CHAIN LENGTH and the forms a link takes rather than one
//! recorded request: a root on its own, chains of two, three and four AMIs, a parent in
//! another Region, a parent published under an owner alias, a parent of another account, a
//! parent the store no longer holds, a parent the caller cannot see, and lineage that points
//! back into the chain.
//!
//! The images are seeded through the store's own writer
//! (`ComAmazonawsEc2Amazonec2State::insert_image`, what a register operation calls) and the
//! parent link is written onto the stored image afterwards, because `CopyImage` — the
//! operation that records one — is still a scaffold with its own stub issue
//! (`20261022T235108Z-ec2-copyimage-stub.md`); that is exactly the lineage a copy of the
//! parent would have stored.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "eu-west-1";

/// Another Region, so a parent AMI can live somewhere other than the Region the call is
/// made in — what a cross-Region copy records in `Image$SourceImageRegion`.
const OTHER_REGION: &str = "us-east-1";

/// An account that is not the one this emulator serves, so an image of its own is another
/// account's.
const PARTNER_ACCOUNT: &str = "210987654321";

/// "The owner alias (`amazon` | `aws-backup-vault` | `aws-marketplace`) of this AMI, if one
/// is assigned" (`ImageAncestryEntry$ImageOwnerAlias`) — the alias the published AMIs of
/// this store carry.
const OWNER_ALIAS_AMAZON: &str = "amazon";

/// A well-formed long-form AMI id nothing in this store mints, so an image whose parent link
/// names it has a parent the store holds no record of.
const UNKNOWN_IMAGE_ID: &str = "ami-0123456789abcdef0";

/// One `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `GetImageAncestry` call for `image_id`.
async fn ancestry(
    router: &MultiServiceRouter,
    image_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("ImageId", image_id)];
    params.extend_from_slice(extra);
    call(router, REGION, "GetImageAncestry", &params).await
}

/// The AMI an image was created from, as the store records it: the parent's id and the
/// Region it lives in.
struct Parent {
    /// `Image$SourceImageId`.
    image_id: String,
    /// `Image$SourceImageRegion`.
    region: String,
}

/// Insert one AMI, carrying the lineage a copy of `parent` would have recorded on it.
fn seed(guard: &mut Ec2State, spec: ImageSpec, parent: Option<Parent>) -> String {
    let image_id = guard.insert_image(spec);
    if let Some(parent) = parent {
        if let Some(image) = guard.images.get_mut(&image_id) {
            image.source_image_id = Some(parent.image_id);
            image.source_image_region = Some(parent.region);
        }
    }
    image_id
}

/// The parent link naming `image_id` in the Region the calls here are made in.
fn local_parent(image_id: &str) -> Option<Parent> {
    Some(Parent {
        image_id: image_id.to_string(),
        region: REGION.to_string(),
    })
}

/// A private AMI of the calling account, named `name` and created at `creation_date`.
fn own_spec(name: &str, creation_date: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        description: format!("The ancestry suite's {name}"),
        creation_date: creation_date.to_string(),
        ..ImageSpec::default()
    }
}

/// The AMIs the tables below ask about.
struct Seeded {
    /// A private AMI of the account with no parent at all: a root on its own.
    root: String,
    /// A copy of [`Self::root`]: a chain of two.
    child: String,
    /// A copy of [`Self::child`]: a chain of three.
    grandchild: String,
    /// A copy of [`Self::grandchild`]: a chain of four.
    great_grandchild: String,
    /// A public AMI of another account published under the `amazon` owner alias.
    published: String,
    /// A copy of [`Self::published`] taken from another Region.
    from_published: String,
    /// A private AMI of another account — one this account cannot see at all.
    partner_private: String,
    /// A copy whose parent is that invisible AMI.
    from_partner_private: String,
    /// A copy whose parent is an AMI this store holds no record of.
    orphan: String,
    /// Two AMIs whose lineage names each other.
    loop_first: String,
    /// The second of that pair.
    loop_second: String,
}

/// Write the AMIs of the tables into the engine's store, the way a register and a copy
/// would have.
fn seed_images(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let root = seed(&mut guard, own_spec("ancestry-root", "2026-01-01T00:00:00.000Z"), None);
    let child = seed(
        &mut guard,
        own_spec("ancestry-child", "2026-01-02T00:00:00.000Z"),
        local_parent(&root),
    );
    let grandchild = seed(
        &mut guard,
        own_spec("ancestry-grandchild", "2026-01-03T00:00:00.000Z"),
        local_parent(&child),
    );
    let great_grandchild = seed(
        &mut guard,
        own_spec("ancestry-great-grandchild", "2026-01-04T00:00:00.000Z"),
        local_parent(&grandchild),
    );

    let published = seed(
        &mut guard,
        ImageSpec {
            public: true,
            owner_id: PARTNER_ACCOUNT.to_string(),
            owner_alias: Some(OWNER_ALIAS_AMAZON.to_string()),
            ..own_spec("ancestry-published", "2025-12-01T00:00:00.000Z")
        },
        None,
    );
    let from_published = seed(
        &mut guard,
        own_spec("ancestry-from-published", "2026-01-05T00:00:00.000Z"),
        Some(Parent {
            image_id: published.clone(),
            region: OTHER_REGION.to_string(),
        }),
    );

    let partner_private = seed(
        &mut guard,
        ImageSpec {
            owner_id: PARTNER_ACCOUNT.to_string(),
            ..own_spec("ancestry-partner-private", "2025-12-02T00:00:00.000Z")
        },
        None,
    );
    let from_partner_private = seed(
        &mut guard,
        own_spec("ancestry-from-partner-private", "2026-01-06T00:00:00.000Z"),
        Some(Parent {
            image_id: partner_private.clone(),
            region: OTHER_REGION.to_string(),
        }),
    );

    let orphan = seed(
        &mut guard,
        own_spec("ancestry-orphan", "2026-01-07T00:00:00.000Z"),
        local_parent(UNKNOWN_IMAGE_ID),
    );

    let loop_first = seed(
        &mut guard,
        own_spec("ancestry-loop-first", "2026-01-08T00:00:00.000Z"),
        None,
    );
    let loop_second = seed(
        &mut guard,
        own_spec("ancestry-loop-second", "2026-01-09T00:00:00.000Z"),
        local_parent(&loop_first),
    );
    if let Some(image) = guard.images.get_mut(&loop_first) {
        image.source_image_id = Some(loop_second.clone());
        image.source_image_region = Some(REGION.to_string());
    }

    Seeded {
        root,
        child,
        grandchild,
        great_grandchild,
        published,
        from_published,
        partner_private,
        from_partner_private,
        orphan,
        loop_first,
        loop_second,
    }
}

/// The `<item>` entries of the response's `imageAncestryEntrySet`, each as the (member,
/// text) pairs it carries, in document order.
///
/// Read with the XML parser rather than by substring, because the rule under test is about
/// the ORDER of the entries and which members each one carries.
fn entries(case: &str, body: &str) -> Vec<Vec<(String, String)>> {
    let document = roxmltree::Document::parse(body).unwrap_or_else(|error| {
        panic!("case {case:?}: the response body must be XML ({error}): {body}")
    });
    document
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "imageAncestryEntrySet")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .map(|item| {
                    item.children()
                        .filter(|node| node.is_element())
                        .map(|node| {
                            (
                                node.tag_name().name().to_string(),
                                node.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .collect::<Vec<Vec<(String, String)>>>()
        })
        .collect()
}

/// The text one entry carries for `member`, or `None` when it carries no such member.
fn member_of(entry: &[(String, String)], member: &str) -> Option<String> {
    entry
        .iter()
        .find(|(name, _)| name == member)
        .map(|(_, text)| text.clone())
}

/// The `imageId` of every entry of the chain, in the order they were reported.
fn chain(case: &str, body: &str) -> Vec<String> {
    entries(case, body)
        .iter()
        .map(|entry| member_of(entry, "imageId").unwrap_or_default())
        .collect()
}

/// What `DescribeImages` reports for `image_id`, as the (member, text) pairs of the one
/// image it answers with — the report an ancestry entry must agree with.
async fn described(router: &MultiServiceRouter, image_id: &str) -> Vec<(String, String)> {
    let (status, body) = call(router, REGION, "DescribeImages", &[p("ImageId.1", image_id)]).await;
    assert_eq!(
        status, 200,
        "DescribeImages for {image_id} must succeed, got {status} with body {body}"
    );
    let document = roxmltree::Document::parse(&body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "imagesSet")
        .into_iter()
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .flat_map(|item| {
                    item.children()
                        .filter(|node| node.is_element())
                        .map(|node| {
                            (
                                node.tag_name().name().to_string(),
                                node.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect::<Vec<(String, String)>>()
                })
                .collect::<Vec<(String, String)>>()
        })
        .collect()
}

/// Rule 1: the chain reported for an AMI is that AMI, then its parent, then its parent's
/// parent, down to the AMI that carries no parent — and every entry's members are the ones
/// the stored image carries.
#[tokio::test]
async fn reports_the_chain_from_the_named_ami_to_the_root() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);

    // Rows: (case, the AMI asked about, the chain it must report). The chains differ in
    // LENGTH (one, two, three and four AMIs) and in the form of their links (a parent in
    // this Region, a parent in another, a parent of another account published under an owner
    // alias), so no single row is the specification.
    let cases: Vec<(&str, &String, Vec<&String>)> = vec![
        (
            "an AMI with no parent at all is its own root",
            &images.root,
            vec![&images.root],
        ),
        (
            "a copy of a root AMI",
            &images.child,
            vec![&images.child, &images.root],
        ),
        (
            "a copy of a copy",
            &images.grandchild,
            vec![&images.grandchild, &images.child, &images.root],
        ),
        (
            "four generations",
            &images.great_grandchild,
            vec![
                &images.great_grandchild,
                &images.grandchild,
                &images.child,
                &images.root,
            ],
        ),
        (
            "a cross-Region copy of another account's published AMI",
            &images.from_published,
            vec![&images.from_published, &images.published],
        ),
        (
            "the published root AMI itself",
            &images.published,
            vec![&images.published],
        ),
    ];

    for (case, image_id, expected) in cases {
        let (status, body) = ancestry(&router, image_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let expected_chain: Vec<String> = expected.iter().map(|id| (*id).clone()).collect();
        let reported = chain(case, &body);
        assert_eq!(
            reported, expected_chain,
            "case {case:?}: expected the chain {expected_chain:?}, got {reported:?} in {body}"
        );

        // Every entry reports the facts of ITS OWN AMI, as `DescribeImages` reports them for
        // the same image: the id, the creation date, the owner alias, and the parent link.
        for (position, entry) in entries(case, &body).iter().enumerate() {
            let entry_id = member_of(entry, "imageId").unwrap_or_default();
            let image = described(&router, &entry_id).await;
            let from_image = |member: &str| member_of(&image, member).unwrap_or_default();

            let reported_date = member_of(entry, "creationDate").unwrap_or_default();
            assert_eq!(
                as_timestamp(&reported_date),
                as_timestamp(&from_image("creationDate")),
                "case {case:?}: entry {position} reported the creation date \
                 {reported_date:?} where the image reports {:?}",
                from_image("creationDate")
            );
            assert_eq!(
                member_of(entry, "imageOwnerAlias").unwrap_or_default(),
                from_image("imageOwnerAlias"),
                "case {case:?}: entry {position} must report the owner alias of {entry_id}, \
                 got {entry:?}"
            );
            assert_eq!(
                member_of(entry, "sourceImageId").unwrap_or_default(),
                from_image("sourceImageId"),
                "case {case:?}: entry {position} must report the parent of {entry_id}, got \
                 {entry:?}"
            );
            assert_eq!(
                member_of(entry, "sourceImageRegion").unwrap_or_default(),
                from_image("sourceImageRegion"),
                "case {case:?}: entry {position} must report the parent's Region of \
                 {entry_id}, got {entry:?}"
            );

            // The chain is the parent link: every entry but the last names the next one as
            // its source, and the last one — the root — names no source at all.
            let next = chain(case, &body).get(position + 1).cloned();
            assert_eq!(
                member_of(entry, "sourceImageId").filter(|id| !id.is_empty()),
                next,
                "case {case:?}: entry {position} of {reported:?} must name the next AMI of \
                 the chain as its source, got {entry:?}"
            );
        }

        // A read: asking twice answers the same chain, so reporting an ancestry changed
        // nothing a later caller sees.
        let (again_status, again_body) = ancestry(&router, image_id, &[]).await;
        assert_eq!(
            (again_status, entries(case, &again_body)),
            (status, entries(case, &body)),
            "case {case:?}: a second read must answer exactly the same, got {again_body}"
        );
    }
}

/// Rule 1b: the walk stops where the chain can no longer be read — at a parent the store
/// holds no record of, at one this account cannot see, and at an AMI already reported — and
/// the entry for the child still names the parent the store recorded on it.
#[tokio::test]
async fn stops_where_the_chain_can_no_longer_be_read() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);

    // Rows: (case, the AMI asked about, the chain it must report, the source the LAST
    // reported entry must name).
    let cases: Vec<(&str, &String, Vec<&String>, &str)> = vec![
        (
            "a copy whose parent the store holds no record of",
            &images.orphan,
            vec![&images.orphan],
            UNKNOWN_IMAGE_ID,
        ),
        (
            "a copy whose parent is another account's private AMI",
            &images.from_partner_private,
            vec![&images.from_partner_private],
            &images.partner_private,
        ),
        (
            "lineage that points back at an AMI of the chain",
            &images.loop_second,
            vec![&images.loop_second, &images.loop_first],
            &images.loop_second,
        ),
    ];

    for (case, image_id, expected, last_source) in cases {
        let (status, body) = ancestry(&router, image_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let expected_chain: Vec<String> = expected.iter().map(|id| (*id).clone()).collect();
        let reported = chain(case, &body);
        assert_eq!(
            reported, expected_chain,
            "case {case:?}: expected the chain {expected_chain:?}, got {reported:?} in {body}"
        );

        let last = entries(case, &body);
        let last = last.last().cloned().unwrap_or_default();
        assert_eq!(
            member_of(&last, "sourceImageId").unwrap_or_default(),
            last_source,
            "case {case:?}: the last entry must still name the parent the store recorded, \
             got {last:?}"
        );
    }

    // Asking about an AMI the caller cannot see is refused rather than served, so the
    // invisible AMI above is reachable neither as a chain of its own nor through its child.
    let (status, body) = ancestry(&router, &images.partner_private, &[]).await;
    assert_eq!(
        status, 400,
        "another account's private AMI must be refused, got {status} with body {body}"
    );
}

/// One refusal row: the request, and the status, code and message it must be refused with.
struct Refusal {
    /// What this row is a case of, named in every assertion of it.
    case: &'static str,
    /// The `ImageId` the request carries.
    image_id: String,
    /// Any further request members — the `DryRun` flag, for the probe rows.
    extra: Vec<(String, String)>,
    /// The HTTP status of the refusal.
    status: u16,
    /// Its `<Code>`.
    code: &'static str,
    /// Its `<Message>`, built from the request the row sends.
    message: String,
}

/// Rule 2: what it refuses — a value that is no AMI id at all, a well-formed id the account
/// can see no image for, and the `DryRun` probe, which answers about permission before the
/// resource is looked up at all.
#[tokio::test]
async fn refuses_what_it_cannot_report() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine);

    // Rows: a value that is not an AMI id at all (three forms of one), a well-formed id in
    // each of the two id forms that names no image, another account's private image, and the
    // two dry runs.
    let malformed = |case: &'static str, image_id: &'static str| Refusal {
        case,
        image_id: image_id.to_string(),
        extra: Vec::new(),
        status: 400,
        code: "InvalidAMIID.Malformed",
        message: format!("Invalid id: '{image_id}'"),
    };
    let not_found = |case: &'static str, image_id: &str| Refusal {
        case,
        image_id: image_id.to_string(),
        extra: Vec::new(),
        status: 400,
        code: "InvalidAMIID.NotFound",
        message: format!("The image id '[{image_id}]' does not exist"),
    };
    let cases: Vec<Refusal> = vec![
        malformed("a value that is not an AMI id at all", "not-an-ami"),
        malformed("an id of another resource kind", "snap-0123456789abcdef0"),
        malformed("an AMI id whose suffix is not hexadecimal", "ami-zzzzzzzz"),
        not_found("a long-form AMI id that names no image", UNKNOWN_IMAGE_ID),
        not_found("a short-form AMI id that names no image", "ami-0abc1234"),
        not_found("another account's private image", &images.partner_private),
        Refusal {
            case: "a dry run of a request that would have succeeded",
            image_id: images.child.clone(),
            extra: vec![p("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
        Refusal {
            case: "a dry run naming an AMI the account has none of",
            image_id: UNKNOWN_IMAGE_ID.to_string(),
            extra: vec![p("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
        Refusal {
            extra: vec![p("DryRun", "true")],
            ..malformed("a dry run naming no AMI id at all", "not-an-ami")
        },
    ];

    for row in cases {
        let case = row.case;
        let (status, body) = ancestry(&router, &row.image_id, &row.extra).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {:?}, got body {body}",
            row.code
        );
        assert_eq!(
            error_message(&body),
            row.message,
            "case {case:?}: expected the message {:?}, got body {body}",
            row.message
        );
    }

    // Nothing above wrote: the chain of the deepest AMI still reads exactly as it did.
    let (status, body) = ancestry(&router, &images.great_grandchild, &[]).await;
    assert_eq!(
        status, 200,
        "the stored chain must still be reported, got {status} with body {body}"
    );
    assert_eq!(
        chain("after the refusals", &body),
        vec![
            images.great_grandchild.clone(),
            images.grandchild.clone(),
            images.child.clone(),
            images.root.clone(),
        ],
        "the refusals must leave the stored lineage alone, got body {body}"
    );
}
