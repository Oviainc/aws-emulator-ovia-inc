// SPDX-License-Identifier: CC0-1.0

//! The S3 Storage Lens group lifecycle, driven end to end over HTTP.
//!
//! The rule under test: a `CreateStorageLensGroup` records the submitted
//! `StorageLensGroup` document WHOLE under the name that document carries, for
//! the account the request names, so every later `GetStorageLensGroup` of that
//! name reports exactly the members the document carried — with
//! `StorageLensGroupArn`, which the model documents as read-only, assigned by
//! the service from the group's own identity and the region the request was
//! signed for rather than taken from the request. A name the account already
//! owns is refused and leaves the group that is there untouched (changing it is
//! `UpdateStorageLensGroup`'s job), a name is unique within the owning account
//! only, and a read of a name the account owns no group of is a 404. The
//! resource tags a create carries are a separate resource: they never change
//! the document a read reports.
//!
//! Every row below differs along something that rule generalises over: which
//! `StorageLensGroupFilter` criteria the group filters on (prefix, suffix,
//! object tag, age, size, and the `And`/`Or` combinations of them), how many
//! values each criterion lists, how deeply the document nests, whether it
//! already carries a `StorageLensGroupArn`, the length and character mix of the
//! group's name, the region the request is signed for, the account that owns
//! it, and how many tags the create carries. One test fills the store to the
//! emulator's own store-wide bound and pins that the refusal past it does not
//! depend on which account id the request names.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model, runs the handler and serializes the restXml
//! response. The router and the request builder are the access point suite's,
//! and the whole-document readers and writers are the Storage Lens
//! configuration suite's, reused rather than copied.

use crate::access_point_lifecycle::{
    call_in_region, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{
    document_xml, leaf, sorted, sorted_leaves, tags_xml, Leaf,
};

/// A second signing region, used to pin that a group's ARN names the region its
/// request was signed for rather than a fixed one.
///
/// `pub(crate)` because the update suite in `tests/all/` pins the same thing
/// about the ARN an update assigns, over this suite's table rather than a copy
/// of it.
pub(crate) const OTHER_REGION: &str = "ap-southeast-2";

/// `StorageLensGroupName`'s modelled maximum length
/// (`com.amazonaws.s3control#StorageLensGroupName`, len 1..64).
pub(crate) const NAME_MAX_LEN: usize = 64;

/// The largest resource tag set a create may carry: `TagList` is modelled at
/// 0..50 entries (`CreateStorageLensGroupRequest$Tags`).
const MAX_TAGS: usize = 50;

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One `StorageLensGroup` document, described as the leaves below its root.
pub(crate) struct Group {
    pub(crate) case: &'static str,
    /// The leaves below the group root, WITHOUT `Name`: the name is supplied
    /// per row, so one document can be created under any name.
    pub(crate) leaves: Vec<Leaf>,
}

impl Group {
    /// This document created under `name`: `Name` plus the row's own leaves.
    pub(crate) fn submitted(&self, name: &str) -> Vec<Leaf> {
        let mut leaves = vec![leaf("Name", name)];
        leaves.extend(self.leaves.iter().cloned());
        leaves
    }

    /// What a read of `name` must report: the submitted document, with
    /// `StorageLensGroupArn` the one the service assigns — the member is
    /// documented read-only, so a submitted value is replaced rather than
    /// stored.
    pub(crate) fn expected(&self, name: &str, region: &str) -> Vec<Leaf> {
        let mut leaves: Vec<Leaf> = self
            .submitted(name)
            .into_iter()
            .filter(|(path, _)| path != "StorageLensGroupArn")
            .collect();
        leaves.push(leaf("StorageLensGroupArn", &expected_arn(name, region)));
        leaves
    }

    pub(crate) fn request_body(&self, name: &str, tags: Option<&[(String, String)]>) -> String {
        format!(
            "<CreateStorageLensGroupRequest xmlns=\"{XMLNS}\">\
             <StorageLensGroup>{}</StorageLensGroup>{}\
             </CreateStorageLensGroupRequest>",
            document_xml(&self.submitted(name)),
            tags_xml(tags)
        )
    }
}

/// The ARN a group of this name must report, in the region the request was
/// signed for: the `storage-lens-group/<name>` resource in the `s3` ARN
/// namespace, under the emulator's canonical account — the form
/// `StorageLensGroupArn`'s modelled pattern fixes
/// (`^arn:[a-z\-]+:s3:[a-z0-9\-]+:\d{12}:storage\-lens\-group\/`).
pub(crate) fn expected_arn(name: &str, region: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:storage-lens-group/{name}",
        ::emulator_server::emulator_account_id()
    )
}

/// The group documents under test. Every member named below is a member of
/// `StorageLensGroupFilter` (or of its `And` / `Or` operators) as the model
/// declares them; the object-age and object-size bounds are given non-zero
/// values on both ends so no leaf is the member's modelled default.
pub(crate) fn groups() -> Vec<Group> {
    vec![
        Group {
            case: "one prefix",
            leaves: vec![leaf("Filter/MatchAnyPrefix/Prefix", "logs/")],
        },
        Group {
            case: "three prefixes",
            leaves: vec![
                leaf("Filter/MatchAnyPrefix/Prefix", "logs/"),
                leaf("Filter/MatchAnyPrefix/Prefix", "reports/2024/"),
                leaf("Filter/MatchAnyPrefix/Prefix", "archive/cold/deep/"),
            ],
        },
        Group {
            case: "two suffixes",
            leaves: vec![
                leaf("Filter/MatchAnySuffix/Suffix", ".log"),
                leaf("Filter/MatchAnySuffix/Suffix", ".csv.gz"),
            ],
        },
        Group {
            case: "prefixes and suffixes together",
            leaves: vec![
                leaf("Filter/MatchAnyPrefix/Prefix", "incoming/"),
                leaf("Filter/MatchAnySuffix/Suffix", ".parquet"),
            ],
        },
        Group {
            case: "one object tag",
            leaves: vec![
                leaf("Filter/MatchAnyTag/Tag/Key", "retention"),
                leaf("Filter/MatchAnyTag/Tag/Value", "long term"),
            ],
        },
        Group {
            case: "object age bounds",
            leaves: vec![
                leaf("Filter/MatchObjectAge/DaysGreaterThan", "30"),
                leaf("Filter/MatchObjectAge/DaysLessThan", "365"),
            ],
        },
        Group {
            case: "object size bounds past 32 bits",
            leaves: vec![
                leaf("Filter/MatchObjectSize/BytesGreaterThan", "1024"),
                leaf("Filter/MatchObjectSize/BytesLessThan", "5368709120"),
            ],
        },
        Group {
            case: "an And of a prefix and a size",
            leaves: vec![
                leaf("Filter/And/MatchAnyPrefix/Prefix", "datasets/"),
                leaf("Filter/And/MatchObjectSize/BytesGreaterThan", "1048576"),
                leaf("Filter/And/MatchObjectSize/BytesLessThan", "1073741824"),
            ],
        },
        Group {
            case: "an Or of a suffix and an age",
            leaves: vec![
                leaf("Filter/Or/MatchAnySuffix/Suffix", ".tmp"),
                leaf("Filter/Or/MatchObjectAge/DaysGreaterThan", "7"),
                leaf("Filter/Or/MatchObjectAge/DaysLessThan", "90"),
            ],
        },
        Group {
            case: "an And of every criterion",
            leaves: vec![
                leaf("Filter/And/MatchAnyPrefix/Prefix", "lake/"),
                leaf("Filter/And/MatchAnySuffix/Suffix", ".orc"),
                leaf("Filter/And/MatchAnyTag/Tag/Key", "tier"),
                leaf("Filter/And/MatchAnyTag/Tag/Value", "gold"),
                leaf("Filter/And/MatchObjectAge/DaysGreaterThan", "1"),
                leaf("Filter/And/MatchObjectAge/DaysLessThan", "2"),
                leaf("Filter/And/MatchObjectSize/BytesGreaterThan", "2"),
                leaf("Filter/And/MatchObjectSize/BytesLessThan", "3"),
            ],
        },
        Group {
            case: "a document that already carries an ARN",
            leaves: vec![
                leaf("Filter/MatchAnyPrefix/Prefix", "submitted-arn/"),
                leaf(
                    "StorageLensGroupArn",
                    "arn:aws:s3:eu-west-1:999999999999:storage-lens-group/not-this-one",
                ),
            ],
        },
    ]
}

/// One name per row of [`groups`], differing in length and character mix
/// within what `StorageLensGroupName` allows (len 1..64, pattern
/// `^[a-zA-Z0-9\-\_]+$`): the shortest a name can be, the longest, digits
/// alone, and the hyphen/underscore/mixed-case forms in between.
pub(crate) fn names() -> Vec<String> {
    let longest = format!("g{}", "r".repeat(NAME_MAX_LEN - 1));
    vec![
        "a".to_string(),
        "cold-storage".to_string(),
        "cold_storage".to_string(),
        "MixedCase-Group_7".to_string(),
        "0123456789".to_string(),
        "age-bounded".to_string(),
        "size-bounded".to_string(),
        "and-operator".to_string(),
        "or-operator".to_string(),
        longest,
        "already-has-an-arn".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Calling the Storage Lens group surface
// ---------------------------------------------------------------------------

pub(crate) async fn create_group(
    emulator: &Emulator,
    account_id: &str,
    name: &str,
    group: &Group,
    tags: Option<&[(String, String)]>,
) -> Reply {
    emulator
        .call(
            "POST",
            "/v20180820/storagelensgroup",
            account_id,
            &group.request_body(name, tags),
        )
        .await
}

pub(crate) async fn get_group(emulator: &Emulator, account_id: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/storagelensgroup/{name}"),
            account_id,
            "",
        )
        .await
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_create_records_the_whole_document_and_every_read_reports_it() {
    let emulator = Emulator::start();

    for (group, name) in groups().iter().zip(names()) {
        let case = group.case;
        let created = create_group(&emulator, ACCOUNT, &name, group, None).await;
        assert_eq!(
            created.status, 204,
            "case {case:?}: expected the create to be accepted with 204, got {} {}",
            created.status, created.body
        );

        let read = get_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from the read, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&group.expected(&name, REGION)),
            "case {case:?}: the read must report the document the create recorded"
        );

        // A read reports; it must not change what the next caller sees.
        let again = get_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            sorted_leaves(&again.body),
            sorted_leaves(&read.body),
            "case {case:?}: a second read must report the same document"
        );
    }
}

#[tokio::test]
async fn the_assigned_arn_follows_the_group_identity_and_the_signing_region() {
    let groups = groups();
    // The row that submits its own `StorageLensGroupArn`: whatever it carried
    // must be replaced by the ARN the service assigns.
    let group = groups
        .iter()
        .find(|group| group.case == "a document that already carries an ARN")
        .expect("the row that submits an ARN");

    // The same name in two regions, and two names in one region: the ARN
    // follows the group's identity and the region the request is signed for,
    // and never the ARN a request submitted. A fresh emulator per region keeps
    // the name free, since a name is unique within its account.
    for region in [REGION, OTHER_REGION] {
        let emulator = Emulator::start();
        for name in ["regional-group", "regional_group_2"] {
            let case = format!("region {region}, name {name:?}");
            let created = call_in_region(
                &emulator.router,
                region,
                "POST",
                "/v20180820/storagelensgroup",
                ACCOUNT,
                &group.request_body(name, None),
            )
            .await;
            assert_eq!(
                created.status, 204,
                "case {case}: expected the create to be accepted with 204, got {} {}",
                created.status, created.body
            );

            let read = call_in_region(
                &emulator.router,
                region,
                "GET",
                &format!("/v20180820/storagelensgroup/{name}"),
                ACCOUNT,
                "",
            )
            .await;
            assert_eq!(
                read.texts("StorageLensGroupArn"),
                vec![expected_arn(name, region)],
                "case {case}: the group must report exactly the ARN the service assigns"
            );
        }
    }
}

#[tokio::test]
async fn creating_a_name_the_account_already_owns_is_refused_and_changes_nothing() {
    let groups = groups();
    let name = "taken";

    // Every row is created over every other row under ONE name, so each refusal
    // is a different pair of documents: a richer one over a poorer one, a
    // poorer one over a richer one, And over Or, and back.
    for (index, first) in groups.iter().enumerate() {
        for second in groups.iter().skip(index + 1) {
            let case = format!("{:?} then {:?}", first.case, second.case);
            let emulator = Emulator::start();

            let created = create_group(&emulator, ACCOUNT, name, first, None).await;
            assert_eq!(
                created.status, 204,
                "case {case}: expected the first create to be accepted with 204, got {} {}",
                created.status, created.body
            );

            let refused = create_group(&emulator, ACCOUNT, name, second, None).await;
            assert_eq!(
                refused.status, 400,
                "case {case}: a second create of one name must be refused, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                "InvalidRequestException",
                "case {case}: unexpected error code in {}",
                refused.body
            );

            let read = get_group(&emulator, ACCOUNT, name).await;
            assert_eq!(
                sorted_leaves(&read.body),
                sorted(&first.expected(name, REGION)),
                "case {case}: the refused create changed the group that was there"
            );
        }
    }
}

#[tokio::test]
async fn groups_are_scoped_to_the_account_that_created_them() {
    let emulator = Emulator::start();
    let groups = groups();
    let name = "shared-name";

    let mine = &groups[1];
    let theirs = &groups[7];
    let created = create_group(&emulator, ACCOUNT, name, mine, None).await;
    assert_eq!(
        created.status, 204,
        "expected the create to be accepted with 204, got {} {}",
        created.status, created.body
    );
    let read = get_group(&emulator, ACCOUNT, name).await;
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&mine.expected(name, REGION)),
        "the caller must read back its own document"
    );

    // Every call naming another account is refused ahead of the lookup, so one
    // account's group is never created, replaced or read under another's — for
    // a name this account holds as much as for one it does not.
    for (case, reply) in [
        (
            "CreateStorageLensGroup under a name this account holds",
            create_group(&emulator, OTHER_ACCOUNT, name, theirs, None).await,
        ),
        (
            "GetStorageLensGroup of a name this account holds",
            get_group(&emulator, OTHER_ACCOUNT, name).await,
        ),
        (
            "GetStorageLensGroup of a name nothing holds",
            get_group(&emulator, OTHER_ACCOUNT, "mine-alone").await,
        ),
    ] {
        assert_eq!(
            (reply.status, reply.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected a call for another account to be refused, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The refused create stored nothing: the document is the one this account
    // put.
    let again = get_group(&emulator, ACCOUNT, name).await;
    assert_eq!(
        sorted_leaves(&again.body),
        sorted(&mine.expected(name, REGION)),
        "the refused create changed the stored document"
    );
}

#[tokio::test]
async fn a_read_of_a_group_that_is_not_there_is_not_found() {
    let emulator = Emulator::start();

    // A group really is there, so "not found" is about the name asked for
    // rather than about an empty store.
    let present = "present";
    let created = create_group(&emulator, ACCOUNT, present, &groups()[0], None).await;
    assert_eq!(
        created.status, 204,
        "expected the create to be accepted with 204, got {} {}",
        created.status, created.body
    );

    // Each row is a name the account owns no group of: one differing only in
    // case, a longer one, a shorter one, and the longest a name can be.
    let longest = format!("a{}", "b".repeat(NAME_MAX_LEN - 1));
    for absent in [
        "PRESENT".to_string(),
        format!("{present}-2"),
        "p".to_string(),
        longest,
    ] {
        let read = get_group(&emulator, ACCOUNT, &absent).await;
        assert_eq!(
            read.status, 404,
            "case {absent:?}: expected 404, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.code(),
            "NoSuchConfiguration",
            "case {absent:?}: unexpected error code in {}",
            read.body
        );
    }

    // The group that is there is still there, so a failed read changed nothing.
    let read = get_group(&emulator, ACCOUNT, present).await;
    assert_eq!(
        read.status, 200,
        "the group that is there must still read back, got {} {}",
        read.status, read.body
    );
}

#[tokio::test]
async fn the_tags_a_create_carries_do_not_change_the_document_it_records() {
    let emulator = Emulator::start();
    let groups = groups();

    // No tag set at all, an empty one, one tag, and exactly the modelled
    // maximum: none of them is part of the document a read reports.
    /// One row: how the case reads, and the tag set the create carries (`None`
    /// being a create with no `Tags` member at all).
    type TagSet = (String, Option<Vec<(String, String)>>);
    let sets: Vec<TagSet> = vec![
        ("no Tags member".to_string(), None),
        ("an empty tag set".to_string(), Some(Vec::new())),
        (
            "one tag".to_string(),
            Some(vec![("owner".to_string(), "data-platform".to_string())]),
        ),
        (
            format!("{MAX_TAGS} tags"),
            Some(
                (0..MAX_TAGS)
                    .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
                    .collect(),
            ),
        ),
    ];

    for (index, (case, tags)) in sets.into_iter().enumerate() {
        let group = &groups[index % groups.len()];
        let name = format!("tagged-{index}");
        let created = create_group(&emulator, ACCOUNT, &name, group, tags.as_deref()).await;
        assert_eq!(
            created.status, 204,
            "case {case:?}: expected the create to be accepted with 204, got {} {}",
            created.status, created.body
        );

        let read = get_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&group.expected(&name, REGION)),
            "case {case:?}: the tag set must not change the document the read reports"
        );
    }
}

#[tokio::test]
async fn a_tag_set_past_the_modelled_maximum_is_refused_and_creates_nothing() {
    let emulator = Emulator::start();
    let group = &groups()[0];

    // One tag past the modelled `TagList` maximum, and far past it.
    for count in [MAX_TAGS + 1, MAX_TAGS * 2] {
        let case = format!("{count} tags");
        let name = format!("over-tagged-{count}");
        let tags: Vec<(String, String)> = (0..count)
            .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
            .collect();

        let refused = create_group(&emulator, ACCOUNT, &name, group, Some(&tags)).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: a tag set past the modelled maximum must be refused, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "ValidationException",
            "case {case}: unexpected error code in {}",
            refused.body
        );

        let read = get_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case}: the refused create recorded a group, answered {} {}",
            read.status, read.body
        );
    }
}

/// The store-wide bound, filled through the front door the way the access point
/// suite fills its own. `DeleteStorageLensGroup` is still a scaffold, so nothing
/// here can release the bound; that belongs to that operation's own issue.
#[tokio::test]
async fn the_number_of_stored_groups_is_bounded_store_wide() {
    let emulator = Emulator::start();
    let group = &groups()[0];
    let quota =
        service_com_amazonaws_s3control_awss3controlservicev20180820::state::STORAGE_LENS_GROUP_QUOTA;

    // Fill the store to its bound. The names differ only in their index, which
    // is the point: the key is request-derived, so a caller could otherwise
    // mint records without end.
    for index in 0..quota {
        let name = format!("bulk-group-{index}");
        let created = create_group(&emulator, ACCOUNT, &name, group, None).await;
        assert_eq!(
            created.status, 204,
            "case group {index}: expected a create within the bound to succeed, got {} {}",
            created.status, created.body
        );
    }

    // Past the bound a create is refused, because the bound is on this
    // process's store.
    let refused = create_group(&emulator, ACCOUNT, "over-the-bound", group, None).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (400, "TooManyStorageLensGroups"),
        "expected a create past the bound to be refused, got {} {}",
        refused.status,
        refused.body
    );
    let read = get_group(&emulator, ACCOUNT, "over-the-bound").await;
    assert_eq!(
        read.status, 404,
        "the refused create recorded a group, answered {} {}",
        read.status, read.body
    );

    // A create under a DIFFERENT account id never reaches the bound at all: the
    // account is authorized first, so no caller can hand itself a fresh
    // allowance by naming another account.
    let denied = create_group(&emulator, OTHER_ACCOUNT, "over-the-bound", group, None).await;
    assert_eq!(
        (denied.status, denied.code().as_str()),
        (403, "AccessDenied"),
        "expected a create for another account to be refused, got {} {}",
        denied.status,
        denied.body
    );

    // The records within the bound are untouched by the refusals.
    let first = get_group(&emulator, ACCOUNT, "bulk-group-0").await;
    assert_eq!(
        first.status, 200,
        "a group created within the bound must still read back, got {} {}",
        first.status, first.body
    );
}
