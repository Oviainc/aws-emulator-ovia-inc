// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CopyOptionGroup`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test:
//!
//! * copying a resource CREATES a new one that carries the source's own settings — for
//!   an option group its engine, its major engine version and every option the source
//!   holds with the configuration the source holds them under — under the identifying
//!   members the request gives (name, description, tags), and records the provenance the
//!   output shape publishes for a copy (`SourceOptionGroup`, `SourceAccountId`,
//!   `CopyTimestamp`), leaving the source itself unchanged and the two independent of
//!   each other afterwards;
//! * the source is resolved in the region the request was signed for, by name or by the
//!   group's own ARN, and a value that names no group there is the modelled
//!   `OptionGroupNotFoundFault` (HTTP 404);
//! * the target name is held to the constraints its member publishes
//!   (`InvalidParameterValue`), a name the region already holds is the modelled
//!   `OptionGroupAlreadyExistsFault` and a region already at its option group quota is
//!   the modelled `OptionGroupQuotaExceededFault` (both HTTP 400), and a refused request
//!   persists nothing.
//!
//! The tables below span what those rules generalise over: EVERY engine
//! `CreateOptionGroupMessage$EngineName` publishes (so an engine added to that list is
//! covered without touching this file), the options each of those engines publishes in
//! the catalogue (zero, one and several per group) with a value set on each option's
//! first modifiable setting, both forms the source may be named by, tag lists of three
//! lengths, a copy of a copy, the regional scope a name resolves in, the shapes the
//! target name's constraints admit and refuse including its published 255-character
//! bound and one past it, and the published per-region option group quota.

use service_com_amazonaws_rds_amazonrdsv19::state::option_catalog::{
    OptionFacts, OptionSettingFacts, OPTIONS,
};
use service_com_amazonaws_rds_amazonrdsv19::state::{
    OPTION_GROUP_ENGINE_NAMES, OPTION_GROUP_QUOTA_PER_REGION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, texts, REGION,
};
use crate::option_group_options::{create_group, major_engine_version, modify};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second region, so an assertion cannot pass by reading another region's records.
const OTHER_REGION: &str = "eu-west-1";

/// The description every copy below asks for, so an answer echoing the SOURCE's
/// description instead of the request's one fails.
const COPY_DESCRIPTION: &str = "the-copied-group";

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(String, String)>;

/// One row of the tag table: the case, the name the copy takes, and the tags the copy
/// request carries.
type TagRow = (&'static str, &'static str, Vec<(&'static str, &'static str)>);

/// The `og` ARN this service reports for an option group.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:og:{name}")
}

/// The awsQuery parameters of one `CopyOptionGroup` call.
fn copy_params(source: &str, target: &str, tags: &[(&str, &str)]) -> Params {
    let mut params: Params = vec![
        ("SourceOptionGroupIdentifier".to_string(), source.to_string()),
        ("TargetOptionGroupIdentifier".to_string(), target.to_string()),
        (
            "TargetOptionGroupDescription".to_string(),
            COPY_DESCRIPTION.to_string(),
        ),
    ];
    params.extend(tag_params(tags));
    params
}

/// The awsQuery parameters that carry `tags` on a call taking a `TagList`.
fn tag_params(tags: &[(&str, &str)]) -> Params {
    tags.iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            [
                (format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()),
                (format!("Tags.Tag.{}.Value", index + 1), (*value).to_string()),
            ]
        })
        .collect()
}

/// `CopyOptionGroup` in `region`, returning the raw status and body so a refusal row can
/// read them.
async fn copy_in(
    router: &MultiServiceRouter,
    region: &str,
    source: &str,
    target: &str,
    tags: &[(&str, &str)],
) -> (u16, String) {
    call_in(
        router,
        region,
        "CopyOptionGroup",
        &copy_params(source, target, tags),
    )
    .await
}

/// `CopyOptionGroup` in [`REGION`].
async fn copy(
    router: &MultiServiceRouter,
    source: &str,
    target: &str,
    tags: &[(&str, &str)],
) -> (u16, String) {
    copy_in(router, REGION, source, target, tags).await
}

/// `DescribeOptionGroups` for one group in `region`, returning the raw status and body.
async fn describe_in(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeOptionGroups",
        &[("OptionGroupName", name)],
    )
    .await
}

/// `DescribeOptionGroups` for one group in [`REGION`], asserting the 200.
async fn describe(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    expect_ok(case, describe_in(router, REGION, name).await)
}

/// The tags `ListTagsForResource` reports for `resource_name`, in the order it reports
/// them.
async fn list_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_name: &str,
) -> Vec<(String, String)> {
    let body = expect_ok(
        case,
        call(
            router,
            "ListTagsForResource",
            &[("ResourceName", resource_name)],
        )
        .await,
    );
    subtrees(&body, "Tag")
        .iter()
        .map(|tag| (text(tag, "Key"), text(tag, "Value")))
        .collect()
}

/// The options the catalogue publishes for `engine_name`, in catalogue order.
///
/// Read from the catalogue itself rather than written out here, so an option added to it
/// is covered by the table below without touching this file.
fn published_options(engine_name: &str) -> Vec<&'static OptionFacts> {
    OPTIONS
        .iter()
        .filter(|facts| facts.engines.contains(&engine_name))
        .collect()
}

/// A value to set on one published option setting, drawn from what the CATALOGUE
/// publishes for that setting rather than written here: an entry of its allowed-value
/// list, a number inside its published `low-high` range, or — for a setting that
/// publishes no allowed values at all — a token of its published data type.
///
/// Always DIFFERENT from the setting's published default, so a copy reporting the
/// catalogue's defaults instead of the source's own configuration could not pass.
fn a_value_other_than_the_default(facts: &OptionSettingFacts) -> String {
    // A comma-separated value list: the first entry that is not already the default.
    if let Some(value) = facts
        .allowed_values
        .split(',')
        .find(|value| !value.is_empty() && *value != facts.default_value)
    {
        return value.to_string();
    }
    // A `low-high` range: its low bound, or one past it when the low bound IS the
    // default, never past the high bound.
    if let Some((low, high)) = facts.allowed_values.split_once('-') {
        if let (Ok(low), Ok(high)) = (low.parse::<i64>(), high.parse::<i64>()) {
            let chosen = if low.to_string() == facts.default_value {
                (low + 1).min(high)
            } else {
                low
            };
            return chosen.to_string();
        }
    }
    // A setting publishing no allowed values at all is free-form, so any value of its
    // published data type will do.
    let free_form = if facts.data_type == "INTEGER" {
        "7"
    } else {
        "emu-copied-value"
    };
    assert_ne!(
        free_form, facts.default_value,
        "setting {:?}: this test needs a value other than the published default",
        facts.name
    );
    free_form.to_string()
}

/// Every option `engine_name` publishes, each with a value for its FIRST modifiable
/// setting, as `ModifyOptionGroup` names them.
///
/// Owned, because the values are computed from the catalogue; [`include_specs`] borrows
/// this back into the shape `modify` takes.
fn options_to_include(engine_name: &str) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    published_options(engine_name)
        .into_iter()
        .map(|facts| {
            let settings = facts
                .settings
                .iter()
                .find(|setting| setting.is_modifiable)
                .map(|setting| vec![(setting.name, a_value_other_than_the_default(setting))])
                .unwrap_or_default();
            (facts.name, settings)
        })
        .collect()
}

/// Add every option `engine_name` publishes to `group`, with the values
/// [`options_to_include`] chose.
async fn include_published_options(router: &MultiServiceRouter, case: &str, group: &str, engine_name: &str) {
    let owned = options_to_include(engine_name);
    if owned.is_empty() {
        return;
    }
    let settings: Vec<Vec<(&str, &str)>> = owned
        .iter()
        .map(|(_, settings)| {
            settings
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect()
        })
        .collect();
    let include: Vec<(&str, &[(&str, &str)])> = owned
        .iter()
        .zip(settings.iter())
        .map(|((option_name, _), settings)| (*option_name, settings.as_slice()))
        .collect();
    expect_ok(case, modify(router, group, &include, &[]).await);
}

/// The one `<OptionGroup>` subtree of an answer — every member of the group at once, so
/// comparing two of them compares the groups and not one member of each.
fn the_group(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "OptionGroup");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one <OptionGroup>, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// The `<Options>` subtree of the one option group an answer reports — the whole option
/// list with every option's configuration, so comparing two of them compares what the
/// groups hold rather than one member of it. A group holding nothing reports an empty
/// list, which is the empty string here.
fn reported_options(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "Options");
    assert!(
        found.len() <= 1,
        "case {case:?}: expected at most one <Options> list, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().unwrap_or_default()
}

/// The option names an answer reports for its group, sorted so the comparison is about
/// WHICH options the group holds.
fn reported_option_names(xml: &str) -> Vec<String> {
    let mut names = texts(xml, "OptionName");
    names.sort_unstable();
    names
}

/// Assert an answer reports `expected` for `tag`, naming the case and the actual value.
fn assert_member(case: &str, xml: &str, tag: &str, expected: &str) {
    let actual = text(xml, tag);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} in {xml}"
    );
}

/// Assert an answer reports NO `tag` at all.
fn assert_no_member(case: &str, xml: &str, tag: &str) {
    let found = texts(xml, tag);
    assert!(
        found.is_empty(),
        "case {case:?}: expected no <{tag}>, got {found:?} in {xml}"
    );
}

/// Assert the three members `OptionGroup` publishes for a COPIED group: the source it
/// was copied from, the account that held the source, and when the copy was taken.
fn assert_copy_provenance(case: &str, xml: &str, source_name: &str) {
    assert_member(case, xml, "SourceOptionGroup", source_name);
    assert_member(case, xml, "SourceAccountId", ACCOUNT);
    let copied_at = text(xml, "CopyTimestamp");
    assert!(
        copied_at
            .get(..4)
            .is_some_and(|year| year.parse::<i32>().is_ok())
            && copied_at.contains('T')
            && copied_at.ends_with('Z'),
        "case {case:?}: expected an ISO-8601 CopyTimestamp, got {copied_at:?} in {xml}"
    );
}

#[tokio::test]
async fn a_copy_carries_the_sources_engine_version_and_options_for_every_published_engine() {
    // Every engine the model publishes for an option group, each in its own emulator so
    // the per-region quota belongs to one engine's groups alone.
    for engine_name in OPTION_GROUP_ENGINE_NAMES {
        let router = emulator();
        let case_source = format!("{engine_name}: the source");
        create_group(&router, &case_source, "emu-copy-source", engine_name).await;
        include_published_options(&router, &case_source, "emu-copy-source", engine_name).await;
        let source_before = the_group(
            &case_source,
            &describe(&router, &case_source, "emu-copy-source").await,
        );
        let source_options = reported_options(&case_source, &source_before);
        let mut published_names: Vec<String> = published_options(engine_name)
            .iter()
            .map(|facts| facts.name.to_string())
            .collect();
        published_names.sort_unstable();

        // Both forms a member that must "specify a valid option group" names one by: the
        // group's own name, and the ARN this service reports for it.
        let source_arn = group_arn(REGION, "emu-copy-source");
        for (form, named_by, target) in [
            ("by name", "emu-copy-source", "emu-copy-by-name"),
            ("by ARN", source_arn.as_str(), "emu-copy-by-arn"),
        ] {
            let case = format!("{engine_name}: {form}");
            let copied = expect_ok(&case, copy(&router, named_by, target, &[]).await);
            // The copy's own answer, and then the copy read back through
            // `DescribeOptionGroups`: the round trip is what proves the record was
            // PERSISTED rather than echoed.
            let described = describe(&router, &case, target).await;
            for (label, xml) in [("CopyOptionGroup", &copied), ("DescribeOptionGroups", &described)]
            {
                let case = format!("{case} via {label}");
                assert_member(&case, xml, "OptionGroupName", target);
                assert_member(&case, xml, "OptionGroupDescription", COPY_DESCRIPTION);
                // The source's own settings: a copy is a group of the SOURCE's engine and
                // major engine version, holding the source's options.
                assert_member(&case, xml, "EngineName", engine_name);
                assert_member(
                    &case,
                    xml,
                    "MajorEngineVersion",
                    major_engine_version(engine_name),
                );
                assert_member(
                    &case,
                    xml,
                    "OptionGroupArn",
                    &group_arn(REGION, target),
                );
                let copied_options = reported_options(&case, xml);
                assert_eq!(
                    copied_options, source_options,
                    "case {case:?}: expected the source's options {source_options:?}, got \
                     {copied_options:?}"
                );
                // And those options are the ones the engine publishes, so an engine whose
                // group holds several carries all of them rather than the first.
                let actual_names = reported_option_names(xml);
                assert_eq!(
                    actual_names, published_names,
                    "case {case:?}: expected the options {published_names:?}, got \
                     {actual_names:?} in {xml}"
                );
                assert_member(
                    &case,
                    xml,
                    "AllowsVpcAndNonVpcInstanceMemberships",
                    &text(&source_before, "AllowsVpcAndNonVpcInstanceMemberships"),
                );
                assert_copy_provenance(&case, xml, "emu-copy-source");
            }
        }

        // Copying read the source and wrote only the copy: the source still reports the
        // same group it did before, and still reports no copy provenance of its own.
        let source_after = the_group(
            &case_source,
            &describe(&router, &case_source, "emu-copy-source").await,
        );
        assert_eq!(
            source_after, source_before,
            "case {case_source:?}: copying changed what the source reports"
        );
        for tag in ["SourceOptionGroup", "SourceAccountId", "CopyTimestamp"] {
            assert_no_member(&case_source, &source_after, tag);
        }
    }
}

#[tokio::test]
async fn the_copy_carries_the_requests_own_tags_and_the_source_keeps_its_own() {
    let router = emulator();
    create_group(&router, "the source", "emu-tagged-source", "mysql").await;
    // The source's own tags, which the copy must not inherit: `CopyOptionGroup` carries
    // no member asking for the source's tags, so the copy's tags are the request's.
    let mut source_params: Params = vec![(
        "ResourceName".to_string(),
        group_arn(REGION, "emu-tagged-source"),
    )];
    source_params.extend(tag_params(&[("owner", "the-source")]));
    expect_ok(
        "tag the source",
        call(&router, "AddTagsToResource", &source_params).await,
    );

    // Tag lists differing in length: none at all, one, and several.
    let rows: Vec<TagRow> = vec![
        ("no tags", "emu-copy-untagged", vec![]),
        ("one tag", "emu-copy-one-tag", vec![("owner", "the-copy")]),
        (
            "three tags",
            "emu-copy-three-tags",
            vec![("owner", "the-copy"), ("tier", "test"), ("stage", "beta")],
        ),
    ];
    for (case, target, tags) in rows {
        expect_ok(case, copy(&router, "emu-tagged-source", target, &tags).await);
        let listed = list_tags(&router, case, &group_arn(REGION, target)).await;
        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the copy to carry the request's tags {expected:?}, got \
             {listed:?}"
        );
    }

    let source_tags = list_tags(&router, "the source keeps its tags", &group_arn(REGION, "emu-tagged-source")).await;
    assert_eq!(
        source_tags,
        vec![("owner".to_string(), "the-source".to_string())],
        "case \"the source keeps its tags\": got {source_tags:?}"
    );
}

#[tokio::test]
async fn a_copy_is_independent_of_its_source_and_a_copy_of_a_copy_names_it() {
    let router = emulator();
    // An engine publishing SEVERAL options, so one can be added to the copy and another
    // removed from it without touching the source.
    let engine_name = "oracle-se2";
    create_group(&router, "the source", "emu-indep-source", engine_name).await;
    include_published_options(&router, "the source", "emu-indep-source", engine_name).await;
    let source_before = the_group(
        "the source",
        &describe(&router, "the source", "emu-indep-source").await,
    );
    expect_ok(
        "the copy",
        copy(&router, "emu-indep-source", "emu-indep-copy", &[]).await,
    );

    // Removing an option from the copy leaves the source holding it.
    let removable = published_options(engine_name)
        .into_iter()
        .find(|facts| !facts.permanent)
        .expect("the engine publishes a removable option");
    expect_ok(
        "remove an option from the copy",
        modify(&router, "emu-indep-copy", &[], &[removable.name]).await,
    );
    let copy_after = describe(&router, "the copy after the removal", "emu-indep-copy").await;
    assert!(
        !texts(&copy_after, "OptionName").iter().any(|name| name == removable.name),
        "case \"remove an option from the copy\": the copy still holds {:?}: {copy_after}",
        removable.name
    );
    let source_after = the_group(
        "the source after the removal",
        &describe(&router, "the source after the removal", "emu-indep-source").await,
    );
    assert_eq!(
        source_after, source_before,
        "case \"remove an option from the copy\": changing the copy changed the source"
    );

    // A copy of a copy names the group it was copied FROM, not that group's own source.
    let second = expect_ok(
        "a copy of a copy",
        copy(&router, "emu-indep-copy", "emu-indep-copy-of-copy", &[]).await,
    );
    assert_copy_provenance("a copy of a copy", &second, "emu-indep-copy");
    let described = describe(&router, "a copy of a copy", "emu-indep-copy-of-copy").await;
    assert_copy_provenance("a copy of a copy, read back", &described, "emu-indep-copy");
    // It carries the group it was copied from, which no longer holds the removed option.
    let second_options = reported_options("a copy of a copy", &described);
    let copy_options = reported_options("the copy after the removal", &copy_after);
    assert_eq!(
        second_options, copy_options,
        "case \"a copy of a copy\": expected the options of the group it was copied from \
         {copy_options:?}, got {second_options:?}"
    );
}

#[tokio::test]
async fn a_source_that_names_no_group_in_the_region_is_refused_as_not_found() {
    let router = emulator();
    create_group(&router, "the source", "emu-missing-source", "mysql").await;
    // A group of the same name in ANOTHER region, so a lookup that ignored the region
    // would find it.
    expect_ok(
        "a group elsewhere",
        call_in(
            &router,
            OTHER_REGION,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", "emu-elsewhere"),
                ("EngineName", "mysql"),
                ("MajorEngineVersion", major_engine_version("mysql")),
                ("OptionGroupDescription", "the group in the other region"),
            ],
        )
        .await,
    );

    // Values that name no group in the region the request is signed for, differing along
    // what the rule generalises over: a name nothing resembles, the stored name in
    // another case (the name is not case-folded), a prefix of it, a name at the published
    // 255-character bound, an ARN nothing answers to, the ARN of the group that exists in
    // another region, and the plain name of that group.
    let elsewhere_arn = group_arn(OTHER_REGION, "emu-elsewhere");
    let at_the_bound = "a".repeat(255);
    let rows: Vec<(&str, &str)> = vec![
        ("a name nothing resembles", "emu-no-such-group"),
        ("the stored name in another case", "EMU-MISSING-SOURCE"),
        ("a prefix of the stored name", "emu-missing"),
        ("a name at the 255-character bound", at_the_bound.as_str()),
        (
            "an ARN nothing answers to",
            "arn:aws:rds:us-west-2:000000000000:og:emu-no-such-group",
        ),
        ("another region's ARN", elsewhere_arn.as_str()),
        ("another region's group by name", "emu-elsewhere"),
    ];
    for (case, source) in rows {
        let body = expect_error(
            case,
            copy(&router, source, "emu-copy-of-nothing", &[]).await,
            404,
            "OptionGroupNotFoundFault",
        );
        // The message is built from the request, so it names the group that was asked
        // for and no other.
        let message = text(&body, "Message");
        assert!(
            message.contains(source),
            "case {case:?}: expected the message to name {source:?}, got {message:?}"
        );
        // A refused copy persists nothing: the target it named is still unknown.
        expect_error(
            case,
            describe_in(&router, REGION, "emu-copy-of-nothing").await,
            404,
            "OptionGroupNotFoundFault",
        );
    }

    // The same source names a group in the region that HOLDS it, so the refusals above
    // are about the region and not about the operation.
    let copied = expect_ok(
        "copy the group in its own region",
        copy_in(
            &router,
            OTHER_REGION,
            "emu-elsewhere",
            "emu-elsewhere-copy",
            &[],
        )
        .await,
    );
    assert_member(
        "copy the group in its own region",
        &copied,
        "OptionGroupArn",
        &group_arn(OTHER_REGION, "emu-elsewhere-copy"),
    );
}

#[tokio::test]
async fn a_copy_refuses_a_target_the_api_does_not_admit_or_the_region_already_holds() {
    let router = emulator();
    create_group(&router, "the source", "emu-target-source", "mysql").await;

    // `TargetOptionGroupIdentifier`: "Can't be null, empty, or blank. Must contain from 1
    // to 255 letters, numbers, or hyphens. First character must be a letter. Can't end
    // with a hyphen or contain two consecutive hyphens." The rows span those constraints,
    // including the published bound and one character past it.
    let past_the_bound = format!("a{}", "b".repeat(255));
    let rows: Vec<(&str, &str)> = vec![
        ("first character not a letter", "9emu-copy"),
        ("ends with a hyphen", "emu-copy-"),
        ("two consecutive hyphens", "emu--copy"),
        ("a character outside the alphabet", "emu.copy"),
        ("one past the 255-character bound", past_the_bound.as_str()),
    ];
    for (case, target) in rows {
        expect_error(
            case,
            copy(&router, "emu-target-source", target, &[]).await,
            400,
            "InvalidParameterValue",
        );
    }
    // The bound itself is admitted, which is what makes the row above a bound and not a
    // blanket refusal.
    let at_the_bound = format!("a{}", "b".repeat(254));
    expect_ok(
        "a name at the 255-character bound",
        copy(&router, "emu-target-source", &at_the_bound, &[]).await,
    );

    // A target the region already holds is the modelled already-exists fault, and the
    // group that holds the name keeps its own description.
    let body = expect_error(
        "a target that already exists",
        copy(&router, "emu-target-source", "emu-target-source", &[]).await,
        400,
        "OptionGroupAlreadyExistsFault",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("emu-target-source"),
        "case \"a target that already exists\": expected the message to name the target, got \
         {message:?}"
    );
    let described = describe(&router, "the existing group is untouched", "emu-target-source").await;
    assert_ne!(
        text(&described, "OptionGroupDescription"),
        COPY_DESCRIPTION,
        "case \"a target that already exists\": the refused copy overwrote the group: {described}"
    );

    // Filling the region to its published quota makes the next copy the modelled quota
    // fault: the two groups above plus the fillers make exactly the quota.
    // The source and the copy that took the name at the bound.
    let existing = 2;
    for index in existing..OPTION_GROUP_QUOTA_PER_REGION {
        create_group(
            &router,
            "fill the region to its quota",
            &format!("emu-filler-{index}"),
            "mysql",
        )
        .await;
    }
    expect_error(
        "the region is at its option group quota",
        copy(&router, "emu-target-source", "emu-one-too-many", &[]).await,
        400,
        "OptionGroupQuotaExceededFault",
    );
    expect_error(
        "the region is at its option group quota",
        describe_in(&router, REGION, "emu-one-too-many").await,
        404,
        "OptionGroupNotFoundFault",
    );
}
