// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ResetCacheParameterGroup`.
//!
//! The rule under test: the operation puts the parameters it names back to the
//! engine's own default on the cache parameter group it names, by REMOVING the
//! values that group holds for them — every name held against the published
//! catalog of that group's FAMILY, so a name the family publishes none of, or
//! one no customer may change, refuses the whole request and resets nothing —
//! and exactly one of `ResetAllParameters` and `ParameterNameValues` says what
//! is reset, so a request where both do, or neither does, is refused 400
//! `InvalidParameterCombination`. A reset parameter is reported by a later
//! `DescribeCacheParameters` under `Source` `system` with the engine's own
//! value, every value the request did not name stays the customer's, a group
//! the account does not hold is refused 404 `CacheParameterGroupNotFound`, a
//! family's own default group may not be reset, and a parameter whose
//! `ChangeType` the engine applies only at a restart leaves every cluster using
//! the group owed a reboot.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in [`crate::harness`], the parameter
//! scanners in [`crate::describe_cache_parameters`] and the writer drivers in
//! [`crate::modify_cache_parameter_group`], so a reset that was not persisted
//! cannot pass: each value is set through `ModifyCacheParameterGroup`, reset
//! through this operation and read back through `DescribeCacheParameters`, and
//! each refusal is followed by a read proving nothing moved. The tables below
//! range over the axes the rule generalises over — the FAMILY (both engine
//! catalogs, several version lines), the parameter's `DataType` and the form of
//! its value, which of the two members selects what is reset, the NUMBER of
//! names in one request (at the documented maximum and one past it), the way a
//! name misses the catalog, the way a group name misses, and the parameter's
//! `ChangeType`.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{create_available_cluster, describe_cluster};
use crate::describe_cache_parameters::{create_group, describe_ok, reported};
use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};
use crate::modify_cache_parameter_group::{
    borrowed, effective, modify_ok, refused, settable,
};

/// Ask `ResetCacheParameterGroup` to reset `names` on `group`, with whatever
/// else `extra` names.
///
/// `ParameterNameValueList`'s member carries the `xmlName` `ParameterNameValue`
/// (`smithy_query(query="shape", shape="ParameterNameValueList")`), which is
/// what an `awsQuery` client indexes the names under. Each pair also carries a
/// `ParameterValue`, which a reset does not read: the parameter goes back to the
/// engine's own value whatever the request says it is worth.
async fn reset_with(
    emulator: &Emulator,
    group: &str,
    names: &[&str],
    extra: &[(&str, &str)],
) -> Reply {
    let mut form = params(&[("CacheParameterGroupName", group)]);
    form.extend(params(extra));
    for (index, name) in names.iter().enumerate() {
        let member = index + 1;
        form.push((
            format!("ParameterNameValues.ParameterNameValue.{member}.ParameterName"),
            (*name).to_string(),
        ));
        form.push((
            format!("ParameterNameValues.ParameterNameValue.{member}.ParameterValue"),
            "ignored-by-a-reset".to_string(),
        ));
    }
    emulator.call("ResetCacheParameterGroup", &form).await
}

/// [`reset_with`] naming only the parameters to reset.
async fn reset(emulator: &Emulator, group: &str, names: &[&str]) -> Reply {
    reset_with(emulator, group, names, &[]).await
}

/// Ask for the whole group to be reset (`ResetAllParameters` true, no names).
async fn reset_all(emulator: &Emulator, group: &str) -> Reply {
    reset_with(emulator, group, &[], &[("ResetAllParameters", "true")]).await
}

/// Assert a reset was accepted and answered with the name the group is STORED
/// under (`CacheParameterGroupNameMessage` carries that alone).
fn reset_accepted(case: &str, group: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case}: ResetCacheParameterGroup must answer 200, got {} {}",
        reply.status, reply.body
    );
    let stored = group.to_ascii_lowercase();
    assert_eq!(
        reply.text("CacheParameterGroupName"),
        Some(stored.as_str()),
        "case {case}: expected the stored group name {stored}, got {}",
        reply.body
    );
}

/// [`reset`], asserting it was accepted.
async fn reset_ok(emulator: &Emulator, case: &str, group: &str, names: &[&str]) {
    let reply = reset(emulator, group, names).await;
    reset_accepted(case, group, &reply);
}

/// Assert every parameter `group` reports is the engine's own value.
async fn all_at_engine_default(emulator: &Emulator, case: &str, group: &str) {
    let reply = describe_ok(emulator, group, &[]).await;
    for parameter in reported(&reply) {
        assert_eq!(
            parameter.field("Source"),
            Some(state::PARAMETER_SOURCE_SYSTEM),
            "case {case}: {} must report the engine's own value, got {:?} / {:?}",
            parameter.name,
            parameter.field("Source"),
            parameter.field("ParameterValue")
        );
    }
}

#[tokio::test]
async fn a_named_parameter_goes_back_to_the_engine_default() {
    let emulator = Emulator::new();

    // The axis: the family (both engine catalogs, four version lines), the
    // parameter's data type and the form of its published value set, and the
    // form of the value that is being reset away — a word, a flag, a bound, a
    // fraction, an integer wider than a double's mantissa.
    let cases: [(&str, &str, &str); 8] = [
        ("redis7", "maxmemory-policy", "allkeys-lfu"),
        ("redis7", "timeout", "20"),
        ("redis7", "activedefrag", "yes"),
        ("redis6.x", "maxmemory-samples", "64"),
        ("redis2.8", "hll-sparse-max-bytes", "16000"),
        ("valkey8", "repl-backlog-size", "9007199254740993"),
        ("memcached1.6", "chunk_size_growth_factor", "1.0625"),
        ("memcached1.6", "binding_protocol", "ascii"),
    ];
    for (index, (family, parameter, value)) in cases.into_iter().enumerate() {
        let case = format!("{family}/{parameter}={value}");
        let group = format!("reset-{index}");
        let neighbour = format!("neighbour-{index}");
        create_group(&emulator, &group, family).await;
        create_group(&emulator, &neighbour, family).await;

        // The engine's own value, before anything is set: what the reset has to
        // put back.
        let (default_value, default_source) = effective(&emulator, &case, &group, parameter).await;
        assert_eq!(
            default_source,
            state::PARAMETER_SOURCE_SYSTEM,
            "case {case}: nothing has been set yet, so the source is {}, got {default_source}",
            state::PARAMETER_SOURCE_SYSTEM
        );
        assert_ne!(
            default_value, value,
            "case {case}: the row's value must differ from the engine default, or the reset \
             proves nothing"
        );

        // Set the value on BOTH groups, so the reset below shows it is the one
        // group that was reset.
        for holder in [&group, &neighbour] {
            modify_ok(&emulator, &case, holder, &[(parameter, value)]).await;
        }

        reset_ok(&emulator, &case, &group, &[parameter]).await;

        let after = effective(&emulator, &case, &group, parameter).await;
        assert_eq!(
            after,
            (default_value.clone(), default_source.clone()),
            "case {case}: a reset parameter must report the engine's own \
             {default_value}/{default_source}, got {after:?}"
        );

        // Every other parameter of the group was already the engine's own and
        // still is.
        all_at_engine_default(&emulator, &case, &group).await;

        // The reset is the GROUP's: the other group of the same family still
        // holds the value that was set on it.
        let elsewhere = effective(&emulator, &case, &neighbour, parameter).await;
        assert_eq!(
            elsewhere,
            (value.to_string(), state::PARAMETER_SOURCE_USER.to_string()),
            "case {case}: another group of the family must keep its value, got {elsewhere:?}"
        );

        // Resetting a parameter the group holds no value for asks for the state
        // it is already in: the request is accepted and changes nothing.
        reset_ok(&emulator, &case, &group, &[parameter]).await;
        let again = effective(&emulator, &case, &group, parameter).await;
        assert_eq!(
            again,
            (default_value.clone(), default_source.clone()),
            "case {case}: resetting twice must leave the engine's own value, got {again:?}"
        );
    }
}

#[tokio::test]
async fn only_the_named_parameters_are_reset() {
    let emulator = Emulator::new();
    create_group(&emulator, "selective", "redis7").await;

    let set: [(&str, &str); 3] = [
        ("maxmemory-policy", "noeviction"),
        ("timeout", "300"),
        ("slowlog-max-len", "256"),
    ];
    modify_ok(&emulator, "three set", "selective", &set).await;

    // One name of the three: that parameter goes back to the engine's own and
    // the other two still carry what was set on them.
    reset_ok(&emulator, "one of three", "selective", &["timeout"]).await;
    let (reset_value, reset_source) =
        effective(&emulator, "one of three", "selective", "timeout").await;
    assert_eq!(
        reset_source,
        state::PARAMETER_SOURCE_SYSTEM,
        "case one of three: timeout must report the engine's own value, got \
         {reset_value}/{reset_source}"
    );
    for (parameter, value) in [set[0], set[2]] {
        let kept = effective(&emulator, "one of three", "selective", parameter).await;
        assert_eq!(
            kept,
            (value.to_string(), state::PARAMETER_SOURCE_USER.to_string()),
            "case one of three: {parameter} was not named, so it keeps {value}, got {kept:?}"
        );
    }

    // Two names in one request, one of them a parameter that is no longer set:
    // both end up at the engine's own value.
    reset_ok(
        &emulator,
        "two names",
        "selective",
        &["maxmemory-policy", "timeout"],
    )
    .await;
    for parameter in ["maxmemory-policy", "timeout"] {
        let (_, source) = effective(&emulator, "two names", "selective", parameter).await;
        assert_eq!(
            source,
            state::PARAMETER_SOURCE_SYSTEM,
            "case two names: {parameter} must report the engine's own value, got {source}"
        );
    }
    // The name asked for twice in one request asks for the one change it
    // already asked for.
    modify_ok(&emulator, "a repeat", "selective", &[("timeout", "600")]).await;
    reset_ok(
        &emulator,
        "a repeat",
        "selective",
        &["timeout", "timeout"],
    )
    .await;
    let (_, source) = effective(&emulator, "a repeat", "selective", "timeout").await;
    assert_eq!(
        source,
        state::PARAMETER_SOURCE_SYSTEM,
        "case a repeat: timeout must report the engine's own value, got {source}"
    );
    let kept = effective(&emulator, "a repeat", "selective", "slowlog-max-len").await;
    assert_eq!(
        kept,
        ("256".to_string(), state::PARAMETER_SOURCE_USER.to_string()),
        "case a repeat: the parameter never named must keep its value, got {kept:?}"
    );
}

#[tokio::test]
async fn reset_all_parameters_puts_every_value_back() {
    let emulator = Emulator::new();

    // The axis: the family behind the group, and how many values it holds —
    // several, and none at all.
    for (family, set) in [
        (
            "redis7",
            &[
                ("maxmemory-policy", "allkeys-random"),
                ("timeout", "300"),
                ("slowlog-max-len", "1024"),
            ][..],
        ),
        (
            "memcached1.6",
            &[("binding_protocol", "ascii"), ("cas_disabled", "1")][..],
        ),
    ] {
        let group = format!("all-{family}");
        let neighbour = format!("all-neighbour-{family}");
        create_group(&emulator, &group, family).await;
        create_group(&emulator, &neighbour, family).await;
        for holder in [&group, &neighbour] {
            modify_ok(&emulator, family, holder, set).await;
        }

        let reply = reset_all(&emulator, &group).await;
        reset_accepted(family, &group, &reply);
        all_at_engine_default(&emulator, family, &group).await;

        // The other group of the family kept every value set on it: resetting
        // the whole group resets ONE group.
        for (parameter, value) in set {
            let kept = effective(&emulator, family, &neighbour, parameter).await;
            assert_eq!(
                kept,
                (
                    (*value).to_string(),
                    state::PARAMETER_SOURCE_USER.to_string()
                ),
                "case {family}: the other group must keep {parameter}={value}, got {kept:?}"
            );
        }

        // A group that holds no value at all is already in the state the
        // request asks for, so it is accepted and reports the engine's own
        // values still.
        let again = reset_all(&emulator, &group).await;
        reset_accepted(family, &group, &again);
        all_at_engine_default(&emulator, family, &group).await;
    }
}

#[tokio::test]
async fn exactly_one_member_must_say_what_is_reset() {
    let emulator = Emulator::new();
    create_group(&emulator, "members", "redis7").await;
    modify_ok(&emulator, "a value to keep", "members", &[("timeout", "300")]).await;

    // "If ResetAllParameters is true, do not use ParameterNameValues. If
    // ResetAllParameters is false, you must specify the name of at least one
    // parameter to reset": the axis is which of the two members says what to
    // reset — both, neither (the flag absent, and the flag explicitly false).
    let both = reset_with(
        &emulator,
        "members",
        &["timeout"],
        &[("ResetAllParameters", "true")],
    )
    .await;
    refused("both members", &both, 400, "InvalidParameterCombination");

    for (case, extra) in [
        ("neither member", &[][..]),
        ("the flag false", &[("ResetAllParameters", "false")][..]),
    ] {
        let reply = reset_with(&emulator, "members", &[], extra).await;
        refused(case, &reply, 400, "InvalidParameterCombination");
    }

    // Each refusal left the value that was set in place.
    let kept = effective(&emulator, "a refused request", "members", "timeout").await;
    assert_eq!(
        kept,
        ("300".to_string(), state::PARAMETER_SOURCE_USER.to_string()),
        "case a refused request: the value set must survive every refusal, got {kept:?}"
    );

    // The flag explicitly false BESIDE a name is the documented combination
    // that asks for a reset of that name.
    let accepted = reset_with(
        &emulator,
        "members",
        &["timeout"],
        &[("ResetAllParameters", "false")],
    )
    .await;
    reset_accepted("the flag false beside a name", "members", &accepted);
    let (_, source) = effective(
        &emulator,
        "the flag false beside a name",
        "members",
        "timeout",
    )
    .await;
    assert_eq!(
        source,
        state::PARAMETER_SOURCE_SYSTEM,
        "case the flag false beside a name: timeout must be reset, got {source}"
    );
}

#[tokio::test]
async fn a_name_the_published_catalog_does_not_carry_is_refused_and_resets_nothing() {
    let emulator = Emulator::new();
    create_group(&emulator, "catalog-redis", "redis7").await;
    create_group(&emulator, "catalog-memcached", "memcached1.6").await;
    modify_ok(
        &emulator,
        "a value to keep",
        "catalog-redis",
        &[("timeout", "300")],
    )
    .await;
    modify_ok(
        &emulator,
        "a value to keep",
        "catalog-memcached",
        &[("binding_protocol", "ascii")],
    )
    .await;

    // The axis: HOW the name misses the group's published catalog — a name
    // nothing publishes, a parameter of the other engine's catalog, one the
    // engine fixes (`IsModifiable: false`), one whose value is per cache node
    // type, a name that differs from a published one by case, and the empty
    // name — and WHERE the bad name sits in the batch.
    let cases: [(&str, &str, &[&str]); 8] = [
        ("catalog-redis", "timeout", &["no-such-parameter"]),
        ("catalog-redis", "timeout", &["chunk_size"]),
        ("catalog-redis", "timeout", &["maxclients"]),
        ("catalog-redis", "timeout", &["maxmemory"]),
        ("catalog-redis", "timeout", &["TIMEOUT"]),
        ("catalog-redis", "timeout", &["timeout", "no-such-parameter"]),
        ("catalog-redis", "timeout", &["no-such-parameter", "timeout"]),
        (
            "catalog-memcached",
            "binding_protocol",
            &["binding_protocol", "maxmemory-policy"],
        ),
    ];
    for (group, held, names) in cases {
        let case = format!("{group} {names:?}");
        let before = effective(&emulator, &case, group, held).await;

        let reply = reset(&emulator, group, names).await;
        let message = refused(&case, &reply, 400, "InvalidParameterValue").to_string();
        let culprit = names
            .iter()
            .find(|name| **name != held)
            .unwrap_or_else(|| panic!("case {case}: a row names one bad parameter"));
        assert!(
            message.contains(culprit),
            "case {case}: the refusal must name {culprit}, got {message}"
        );

        // The request is one change: the name beside the bad one was not reset
        // either, so the value set on the group survives.
        let after = effective(&emulator, &case, group, held).await;
        assert_eq!(
            after, before,
            "case {case}: a refused request must leave {before:?} in effect, got {after:?}"
        );
    }

    // A pair that names no parameter at all asks for no change this operation
    // can make, and the list it is in is not empty, so it is not the
    // combination refusal either.
    for (case, extra) in [
        (
            "no name",
            &[(
                "ParameterNameValues.ParameterNameValue.1.ParameterValue",
                "300",
            )][..],
        ),
        (
            "an empty name",
            &[(
                "ParameterNameValues.ParameterNameValue.1.ParameterName",
                "",
            )][..],
        ),
    ] {
        let mut form = params(&[("CacheParameterGroupName", "catalog-redis")]);
        form.extend(params(extra));
        let reply = emulator.call("ResetCacheParameterGroup", &form).await;
        refused(case, &reply, 400, "InvalidParameterValue");
        let kept = effective(&emulator, case, "catalog-redis", "timeout").await;
        assert_eq!(
            kept,
            ("300".to_string(), state::PARAMETER_SOURCE_USER.to_string()),
            "case {case}: a refused request must reset nothing, got {kept:?}"
        );
    }
}

#[tokio::test]
async fn the_documented_name_window_bounds_one_request() {
    let emulator = Emulator::new();
    create_group(&emulator, "window", "redis7").await;

    // "A maximum of 20 parameters may be modified per request", the window the
    // list this operation shares with the writer documents: the axis is the
    // number of names, at the documented maximum and one past it. The
    // parameters come from the published catalog rather than a hand-written
    // list.
    let pairs = settable("redis7", state::MAX_PARAMETERS_PER_REQUEST + 1);
    let all = borrowed(&pairs);
    let names: Vec<&str> = all.iter().map(|(name, _)| *name).collect();

    // Setting them takes two requests, for the same documented reason.
    modify_ok(
        &emulator,
        "the values to reset",
        "window",
        &all[..state::MAX_PARAMETERS_PER_REQUEST],
    )
    .await;
    modify_ok(
        &emulator,
        "the value past the window",
        "window",
        &all[state::MAX_PARAMETERS_PER_REQUEST..],
    )
    .await;

    // One name past the documented maximum: refused, and nothing is reset.
    let over = reset(&emulator, "window", &names).await;
    refused(
        "one past the documented maximum",
        &over,
        400,
        "InvalidParameterValue",
    );
    for (parameter, value) in &all {
        let kept = effective(
            &emulator,
            "one past the documented maximum",
            "window",
            parameter,
        )
        .await;
        assert_eq!(
            kept,
            (
                (*value).to_string(),
                state::PARAMETER_SOURCE_USER.to_string()
            ),
            "case one past the documented maximum: {parameter} must keep {value}, got {kept:?}"
        );
    }

    // The documented maximum itself is accepted, and every name in it is reset.
    reset_ok(
        &emulator,
        "the documented maximum",
        "window",
        &names[..state::MAX_PARAMETERS_PER_REQUEST],
    )
    .await;
    for (parameter, _) in &all[..state::MAX_PARAMETERS_PER_REQUEST] {
        let (_, source) = effective(&emulator, "the documented maximum", "window", parameter).await;
        assert_eq!(
            source,
            state::PARAMETER_SOURCE_SYSTEM,
            "case the documented maximum: {parameter} must be reset, got {source}"
        );
    }
    let (last_parameter, last_value) = &all[state::MAX_PARAMETERS_PER_REQUEST];
    let kept = effective(
        &emulator,
        "the documented maximum",
        "window",
        last_parameter,
    )
    .await;
    assert_eq!(
        kept,
        (
            (*last_value).to_string(),
            state::PARAMETER_SOURCE_USER.to_string()
        ),
        "case the documented maximum: the name left out must keep {last_value}, got {kept:?}"
    );
}

#[tokio::test]
async fn a_group_the_account_cannot_reset_is_refused() {
    let emulator = Emulator::new();
    create_group(&emulator, "holder", "redis7").await;
    modify_ok(&emulator, "a value to reset", "holder", &[("timeout", "300")]).await;
    // A group that existed and was deleted is a name the account no longer
    // holds, so it is refused exactly like one that never existed.
    create_group(&emulator, "gone", "redis7").await;
    let removed = emulator
        .call(
            "DeleteCacheParameterGroup",
            &params(&[("CacheParameterGroupName", "gone")]),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "DeleteCacheParameterGroup must answer 200, got {} {}",
        removed.status, removed.body
    );

    // The axis: the way the name misses — never created, created and deleted, a
    // `default.` name of a family the service does not publish, the bare
    // prefix, and a name that differs from a stored one by more than case.
    for group in [
        "no-such-group",
        "gone",
        "default.redis99",
        "default.",
        "holder-2",
    ] {
        let named = reset(&emulator, group, &["timeout"]).await;
        let message = refused(group, &named, 404, "CacheParameterGroupNotFound").to_string();
        assert!(
            message.contains(group),
            "case {group}: the refusal must name the group asked for, got {message}"
        );
        // The whole-group form resolves the same name the same way.
        let whole = reset_all(&emulator, group).await;
        refused(group, &whole, 404, "CacheParameterGroupNotFound");
    }

    // The group a request is about is resolved BEFORE its list is measured, so
    // a name the account does not hold is answered with the modelled 404
    // whatever the list carries — here one name past the documented maximum.
    let pairs = settable("redis7", state::MAX_PARAMETERS_PER_REQUEST + 1);
    let over = borrowed(&pairs);
    let names: Vec<&str> = over.iter().map(|(name, _)| *name).collect();
    let oversized = reset(&emulator, "no-such-group", &names).await;
    refused(
        "an oversized list on a group the account does not hold",
        &oversized,
        404,
        "CacheParameterGroupNotFound",
    );

    // Every family's own default group is the service's, not the customer's:
    // its values ARE the engine defaults, so none of its parameters can be put
    // back to one, in either form of the request. The refusal states the rule
    // rather than the group, which is how the service words it.
    for family in state::PARAMETER_GROUP_FAMILIES {
        let group = format!("default.{family}");
        let named = reset(&emulator, &group, &["maxmemory-policy"]).await;
        let message = refused(&group, &named, 400, "InvalidParameterValue").to_string();
        assert_eq!(
            message, "Cannot modify a default parameter group.",
            "case {group}: expected the service's default-group sentence, got {message}"
        );
        let whole = reset_all(&emulator, &group).await;
        refused(&group, &whole, 400, "InvalidParameterValue");
        all_at_engine_default(&emulator, &group, &group).await;
    }

    // A `CacheParameterGroupName` "is stored as a lowercase string", so a
    // custom group is reset however the request spells it, and the one stored
    // group is what changes.
    reset_ok(&emulator, "another case", "HOLDER", &["timeout"]).await;
    let (_, source) = effective(&emulator, "another case", "holder", "timeout").await;
    assert_eq!(
        source,
        state::PARAMETER_SOURCE_SYSTEM,
        "case another case: the stored group must carry the reset, got {source}"
    );
}

#[tokio::test]
async fn the_reset_is_recorded_as_an_event_of_the_group() {
    let emulator = Emulator::new();
    create_group(&emulator, "logged", "redis7").await;
    modify_ok(&emulator, "an event", "logged", &[("timeout", "300")]).await;
    reset_ok(&emulator, "an event", "logged", &["timeout"]).await;
    let whole = reset_all(&emulator, "logged").await;
    reset_accepted("an event", "logged", &whole);

    let reply = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceIdentifier", "logged"),
                ("SourceType", "cache-parameter-group"),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "DescribeEvents must answer 200, got {} {}",
        reply.status, reply.body
    );
    // The create records one event, the change another and each reset one more.
    let messages = reply.all("Event/Message");
    assert!(
        messages.len() >= 4,
        "each reset must have recorded an event beside the create and the change, got \
         {messages:?}"
    );
    for identifier in reply.all("Event/SourceIdentifier") {
        assert_eq!(
            identifier, "logged",
            "every event selected must belong to the group, got {identifier}"
        );
    }
}

#[tokio::test]
async fn a_reset_the_engine_applies_only_at_a_restart_waits_for_a_reboot() {
    let emulator = Emulator::new();

    // `Parameter$ChangeType` is the axis: putting back a value the engine
    // applies at once leaves the clusters using the group in sync, and putting
    // back one it applies only at a restart leaves them owed a reboot. The
    // published catalogs put the two change types on different engines — every
    // Redis OSS parameter a customer may modify is applied immediately, and
    // every settable Memcached one needs a restart — so each row runs on the
    // engine that has it.
    create_group(&emulator, "immediate-group", "redis7").await;
    create_group(&emulator, "reboot-group", "memcached1.6").await;
    create_available_cluster(
        &emulator,
        "immediate-cluster",
        &[("CacheParameterGroupName", "immediate-group")],
    )
    .await;
    create_available_cluster(
        &emulator,
        "reboot-cluster",
        &[
            ("Engine", "memcached"),
            ("NumCacheNodes", "2"),
            ("CacheParameterGroupName", "reboot-group"),
        ],
    )
    .await;

    /// The apply status and the node ids a describe reports for a cluster's
    /// parameter group.
    async fn status(emulator: &Emulator, case: &str, cluster: &str) -> (String, Vec<String>) {
        let reply = describe_cluster(emulator, cluster).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: DescribeCacheClusters must answer 200, got {} {}",
            reply.status, reply.body
        );
        (
            reply
                .text("CacheParameterGroup/ParameterApplyStatus")
                .unwrap_or_default()
                .to_string(),
            reply
                .all("CacheNodeIdsToReboot/CacheNodeId")
                .into_iter()
                .map(str::to_string)
                .collect(),
        )
    }

    /// Reboot every node of `cluster`, so the next assertion starts from a
    /// cluster that runs its group's values.
    async fn reboot(emulator: &Emulator, cluster: &str, nodes: &[&str]) {
        let mut form = params(&[("CacheClusterId", cluster)]);
        for (index, node) in nodes.iter().enumerate() {
            form.push((
                format!("CacheNodeIdsToReboot.CacheNodeId.{}", index + 1),
                (*node).to_string(),
            ));
        }
        let reply = emulator.call("RebootCacheCluster", &form).await;
        assert_eq!(
            reply.status, 200,
            "RebootCacheCluster must answer 200, got {} {}",
            reply.status, reply.body
        );
        std::thread::sleep(PAST_TRANSITION);
    }

    // A value the engine applies at once: setting it and putting it back leave
    // the cluster running the group's values throughout.
    modify_ok(
        &emulator,
        "an immediate value",
        "immediate-group",
        &[("maxmemory-policy", "allkeys-lru")],
    )
    .await;
    reset_ok(
        &emulator,
        "an immediate reset",
        "immediate-group",
        &["maxmemory-policy"],
    )
    .await;
    let (apply_status, ids) = status(&emulator, "an immediate reset", "immediate-cluster").await;
    assert_eq!(
        (apply_status.as_str(), ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case an immediate reset: expected {}, got {apply_status} {ids:?}",
        state::PARAMETER_APPLY_STATUS
    );

    // A value the engine picks up only when a node restarts: set it, reboot the
    // nodes so the cluster runs it, then put it back — the nodes are owed a
    // reboot again, because what they are running is no longer what the group
    // says.
    modify_ok(
        &emulator,
        "a deferred value",
        "reboot-group",
        &[("binding_protocol", "ascii")],
    )
    .await;
    reboot(&emulator, "reboot-cluster", &["0001", "0002"]).await;
    let (settled, settled_ids) = status(&emulator, "a rebooted cluster", "reboot-cluster").await;
    assert_eq!(
        (settled.as_str(), settled_ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case a rebooted cluster: expected {}, got {settled} {settled_ids:?}",
        state::PARAMETER_APPLY_STATUS
    );

    reset_ok(
        &emulator,
        "a deferred reset",
        "reboot-group",
        &["binding_protocol"],
    )
    .await;
    let (apply_status, ids) = status(&emulator, "a deferred reset", "reboot-cluster").await;
    assert_eq!(
        (apply_status.as_str(), ids.as_slice()),
        (
            state::PARAMETER_APPLY_STATUS_PENDING_REBOOT,
            &["0001".to_string(), "0002".to_string()][..]
        ),
        "case a deferred reset: every node of the cluster must need a reboot, got \
         {apply_status} {ids:?}"
    );
    // The cluster that uses the OTHER group is unaffected: the reset is the
    // group's.
    let (other_status, other_ids) =
        status(&emulator, "a deferred reset elsewhere", "immediate-cluster").await;
    assert_eq!(
        (other_status.as_str(), other_ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case a deferred reset elsewhere: expected {}, got {other_status} {other_ids:?}",
        state::PARAMETER_APPLY_STATUS
    );

    // Resetting a parameter the group holds no value for changes nothing, so it
    // owes the clusters nothing either.
    reboot(&emulator, "reboot-cluster", &["0001", "0002"]).await;
    reset_ok(
        &emulator,
        "a reset that changes nothing",
        "reboot-group",
        &["binding_protocol"],
    )
    .await;
    let (apply_status, ids) =
        status(&emulator, "a reset that changes nothing", "reboot-cluster").await;
    assert_eq!(
        (apply_status.as_str(), ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case a reset that changes nothing: expected {}, got {apply_status} {ids:?}",
        state::PARAMETER_APPLY_STATUS
    );
}
