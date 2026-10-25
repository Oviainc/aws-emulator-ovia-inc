// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyCacheParameterGroup`.
//!
//! The rule under test: the operation SETS, on the cache parameter group it
//! names, the value of every `ParameterNameValues` pair, each held against the
//! published catalog row of that group's FAMILY — the name has to be one the
//! family publishes, the parameter one a customer may modify, and the value one
//! of the row's `DataType` that the row's `AllowedValues` covers — and the
//! accepted text is stored verbatim, so a later `DescribeCacheParameters`
//! reports exactly it under `Source` `user` while every other parameter still
//! reports the engine's own. A pair the catalog does not take is refused 400
//! `InvalidParameterValue` and the whole request changes nothing; a group the
//! account does not hold is refused 404 `CacheParameterGroupNotFound`; a
//! family's own default group may not be modified; and a value whose
//! `ChangeType` the engine applies only at a restart leaves every cluster using
//! the group reporting `pending-reboot` until the nodes are rebooted.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in [`crate::harness`] and the parameter
//! scanners in [`crate::describe_cache_parameters`], so a change that was not
//! persisted cannot pass: each accepted row is read back through
//! `DescribeCacheParameters` and each refusal is followed by a read proving
//! nothing moved. The tables below range over the axes the rule generalises
//! over — the FAMILY (both engine catalogs, several version lines), the
//! parameter's `DataType`, the FORM of its published value set (an enumeration
//! of words, a flag, a range bounded at both ends, one bounded only at the
//! bottom, a literal beside a range, and no published set at all), the form of
//! the value (an integer, a fraction, a trailing zero, an integer wider than a
//! double's mantissa), the published BOUNDS and one past each of them, the
//! number of pairs in one request, and the parameter's `ChangeType`.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{create_available_cluster, describe_cluster};
use crate::describe_cache_parameters::{create_group, describe_ok, one, reported};
use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// Ask `ModifyCacheParameterGroup` to set `pairs` on `group`.
///
/// `ParameterNameValueList`'s member carries the `xmlName` `ParameterNameValue`
/// (`smithy_query(query="shape", shape="ParameterNameValueList")`), which is
/// what an `awsQuery` client indexes the pairs under.
pub(crate) async fn modify(emulator: &Emulator, group: &str, pairs: &[(&str, &str)]) -> Reply {
    let mut form = params(&[("CacheParameterGroupName", group)]);
    for (index, (name, value)) in pairs.iter().enumerate() {
        let member = index + 1;
        form.push((
            format!("ParameterNameValues.ParameterNameValue.{member}.ParameterName"),
            (*name).to_string(),
        ));
        form.push((
            format!("ParameterNameValues.ParameterNameValue.{member}.ParameterValue"),
            (*value).to_string(),
        ));
    }
    emulator.call("ModifyCacheParameterGroup", &form).await
}

/// [`modify`], asserting the change was accepted and answered with the name the
/// group is STORED under (`CacheParameterGroupNameMessage` carries that alone).
pub(crate) async fn modify_ok(emulator: &Emulator, case: &str, group: &str, pairs: &[(&str, &str)]) {
    let reply = modify(emulator, group, pairs).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: ModifyCacheParameterGroup must answer 200, got {} {}",
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

/// Assert `reply` is the refusal `status` / `code`, and hand back its message.
///
/// `pub(crate)` for the reason the scanners in
/// [`crate::describe_cache_parameters`] are: the suite covering
/// `ResetCacheParameterGroup` drives the same group through the same writer and
/// reads a refusal and a parameter's effective value exactly the same way, and a
/// second copy of these drivers would be a second thing to keep correct.
pub(crate) fn refused<'a>(case: &str, reply: &'a Reply, status: u16, code: &str) -> &'a str {
    assert_eq!(
        reply.status, status,
        "case {case}: expected {status}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case}: expected {code}, got {}",
        reply.body
    );
    reply.error_message()
}

/// What `DescribeCacheParameters` reports for `parameter` of `group`: the value
/// in effect and the source it is reported under.
pub(crate) async fn effective(
    emulator: &Emulator,
    case: &str,
    group: &str,
    parameter: &str,
) -> (String, String) {
    let reply = describe_ok(emulator, group, &[]).await;
    let reported_one = one(&reply, case, parameter);
    (
        reported_one
            .field("ParameterValue")
            .unwrap_or_default()
            .to_string(),
        reported_one.field("Source").unwrap_or_default().to_string(),
    )
}

/// The published `AllowedValues` and `DataType` of `parameter` of `group`, as a
/// read of the group reports them — the catalog a refusal is checked against is
/// the one a client can see.
async fn published(emulator: &Emulator, case: &str, group: &str, parameter: &str) -> (String, String) {
    let reply = describe_ok(emulator, group, &[]).await;
    let reported_one = one(&reply, case, parameter);
    (
        reported_one
            .field("AllowedValues")
            .unwrap_or_default()
            .to_string(),
        reported_one
            .field("DataType")
            .unwrap_or_default()
            .to_string(),
    )
}

/// A value the published set `allowed` covers: its first term — that term's own
/// literal value, or the bottom of its range. The source is the published
/// `Parameter$AllowedValues`, in the forms it is written in (`yes,no`, `1-75`,
/// `1048576-`, `0,20-`).
fn first_allowed(allowed: &str) -> Option<String> {
    let term = allowed.split(',').next()?;
    match term.split_once('-') {
        Some((low, _)) if low.parse::<f64>().is_ok() => Some(low.to_string()),
        _ => Some(term.to_string()),
    }
}

/// `count` parameters of `family` a customer may set, each with a value its
/// published row takes ([`first_allowed`]) — the request bodies the pair-count
/// window is exercised with, taken from the catalog this service publishes
/// rather than written out by hand.
pub(crate) fn settable(family: &str, count: usize) -> Vec<(String, String)> {
    let parameters = state::family_default_parameters(family)
        .unwrap_or_else(|| panic!("case {family}: a published family has a catalog"));
    let chosen: Vec<(String, String)> = parameters
        .values()
        .filter(|parameter| parameter.is_modifiable && !parameter.is_node_type_specific())
        .filter_map(|parameter| {
            let allowed = parameter.allowed_values.as_deref()?;
            Some((parameter.name.clone(), first_allowed(allowed)?))
        })
        .take(count)
        .collect();
    assert_eq!(
        chosen.len(),
        count,
        "case {family}: the family must publish {count} settable parameters, got {}",
        chosen.len()
    );
    chosen
}

/// `pairs` as the borrowed rows [`modify`] takes.
pub(crate) fn borrowed(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

#[tokio::test]
async fn a_value_the_published_catalog_takes_is_set_and_read_back() {
    let emulator = Emulator::new();

    // The axis: the family (both engine catalogs, four version lines), the
    // parameter's data type and the form of its published value set, and the
    // form of the value — including a published bound, a fraction that must
    // keep every digit and an integer wider than a double's mantissa.
    let cases: [(&str, &str, &str); 11] = [
        ("redis7", "maxmemory-policy", "allkeys-lfu"),
        ("redis7", "active-defrag-cycle-max", "1"),
        ("redis7", "active-defrag-threshold-lower", "100"),
        ("redis7", "timeout", "20"),
        ("redis7", "activedefrag", "yes"),
        ("redis6.x", "maxmemory-samples", "64"),
        ("redis2.8", "hll-sparse-max-bytes", "16000"),
        ("redis5.0", "notify-keyspace-events", "gxE"),
        ("valkey8", "repl-backlog-size", "9007199254740993"),
        ("memcached1.6", "chunk_size_growth_factor", "1.0625"),
        ("memcached1.6", "binding_protocol", "ascii"),
    ];
    for (index, (family, parameter, value)) in cases.into_iter().enumerate() {
        let case = format!("{family}/{parameter}={value}");
        let group = format!("set-{index}");
        let untouched = format!("untouched-{index}");
        create_group(&emulator, &group, family).await;
        create_group(&emulator, &untouched, family).await;

        // The engine's own value for the parameter, before anything is set: a
        // row whose value equals it would prove nothing.
        let (default_value, default_source) = effective(&emulator, &case, &group, parameter).await;
        assert_eq!(
            default_source,
            state::PARAMETER_SOURCE_SYSTEM,
            "case {case}: nothing has been set yet, so the source is {}, got {default_source}",
            state::PARAMETER_SOURCE_SYSTEM
        );
        assert_ne!(
            default_value, value,
            "case {case}: the row's value must differ from the engine default, or it proves \
             nothing"
        );

        modify_ok(&emulator, &case, &group, &[(parameter, value)]).await;

        // The value is read back EXACTLY as it was sent: no rounding, no
        // reformatting, no dropped digit.
        let (set_value, set_source) = effective(&emulator, &case, &group, parameter).await;
        assert_eq!(
            set_value, value,
            "case {case}: expected the value that was set verbatim, got {set_value}"
        );
        assert_eq!(
            set_source,
            state::PARAMETER_SOURCE_USER,
            "case {case}: a value set on the group is reported as {}, got {set_source}",
            state::PARAMETER_SOURCE_USER
        );

        // Only that parameter moved: every other one of the group still reports
        // the service's own value.
        let reply = describe_ok(&emulator, &group, &[]).await;
        for other in reported(&reply)
            .iter()
            .filter(|other| other.name != parameter)
        {
            assert_eq!(
                other.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {case}: {} was not set, so its source is {}, got {:?}",
                other.name,
                state::PARAMETER_SOURCE_SYSTEM,
                other.field("Source")
            );
        }

        // The value lives on the GROUP: another group of the same family still
        // reports the engine's own.
        let (neighbour_value, neighbour_source) =
            effective(&emulator, &case, &untouched, parameter).await;
        assert_eq!(
            (neighbour_value.as_str(), neighbour_source.as_str()),
            (default_value.as_str(), state::PARAMETER_SOURCE_SYSTEM),
            "case {case}: another group of the family must be untouched, got \
             {neighbour_value}/{neighbour_source}"
        );

        // Setting the same value again asks for the same state and reports it:
        // the write is idempotent.
        modify_ok(&emulator, &case, &group, &[(parameter, value)]).await;
        let (again, again_source) = effective(&emulator, &case, &group, parameter).await;
        assert_eq!(
            (again.as_str(), again_source.as_str()),
            (value, state::PARAMETER_SOURCE_USER),
            "case {case}: setting the same value twice must leave it set, got {again}/\
             {again_source}"
        );
    }
}

#[tokio::test]
async fn several_parameters_are_set_in_one_request_and_each_is_kept() {
    let emulator = Emulator::new();
    create_group(&emulator, "many", "redis7").await;

    // One request carrying three pairs, then a second carrying one more: the
    // group accumulates the values set on it rather than replacing them.
    let first: [(&str, &str); 3] = [
        ("maxmemory-policy", "noeviction"),
        ("timeout", "300"),
        ("slowlog-max-len", "256"),
    ];
    modify_ok(&emulator, "three pairs", "many", &first).await;
    modify_ok(
        &emulator,
        "a later pair",
        "many",
        &[("reserved-memory-percent", "10")],
    )
    .await;

    for (parameter, value) in first
        .into_iter()
        .chain([("reserved-memory-percent", "10")])
    {
        let (in_effect, source) = effective(&emulator, parameter, "many", parameter).await;
        assert_eq!(
            (in_effect.as_str(), source.as_str()),
            (value, state::PARAMETER_SOURCE_USER),
            "case {parameter}: expected {value} as the customer's, got {in_effect}/{source}"
        );
    }

    // A later request may change a value that is already set.
    modify_ok(
        &emulator,
        "an overwrite",
        "many",
        &[("timeout", "0"), ("slowlog-max-len", "1024")],
    )
    .await;
    for (parameter, value) in [("timeout", "0"), ("slowlog-max-len", "1024")] {
        let (in_effect, source) = effective(&emulator, parameter, "many", parameter).await;
        assert_eq!(
            (in_effect.as_str(), source.as_str()),
            (value, state::PARAMETER_SOURCE_USER),
            "case {parameter}: expected the overwritten {value}, got {in_effect}/{source}"
        );
    }
}

/// What a refusal row expects the refusal to be ABOUT.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// The value is not one of the parameter's `DataType` at all, so the
    /// refusal names that type.
    NotOfType,
    /// The value is of the type but outside the published `AllowedValues`, so
    /// the refusal names that set.
    OutsideAllowedValues,
    /// The pair names no parameter of the family that a customer may set.
    NotSettable,
}

#[tokio::test]
async fn a_pair_the_published_catalog_does_not_take_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    create_group(&emulator, "refusals-redis", "redis7").await;
    create_group(&emulator, "refusals-memcached", "memcached1.6").await;

    // The axis: HOW the pair falls outside the catalog — the wrong type, one
    // past each published bound, outside an enumeration (including by case
    // alone), a parameter the engine or the node type fixes, a parameter of the
    // other engine's catalog, and a name nothing publishes.
    let cases: [(&str, &str, &str, Expect); 19] = [
        ("refusals-redis", "maxmemory-policy", "not-a-policy", Expect::OutsideAllowedValues),
        ("refusals-redis", "maxmemory-policy", "VOLATILE-LRU", Expect::OutsideAllowedValues),
        ("refusals-redis", "activedefrag", "true", Expect::OutsideAllowedValues),
        ("refusals-redis", "active-defrag-cycle-max", "not-a-number", Expect::NotOfType),
        ("refusals-redis", "active-defrag-cycle-max", "1.5", Expect::NotOfType),
        ("refusals-redis", "active-defrag-cycle-max", "", Expect::NotOfType),
        ("refusals-redis", "active-defrag-cycle-max", "0", Expect::OutsideAllowedValues),
        ("refusals-redis", "active-defrag-cycle-max", "76", Expect::OutsideAllowedValues),
        ("refusals-redis", "active-defrag-threshold-lower", "101", Expect::OutsideAllowedValues),
        ("refusals-redis", "timeout", "19", Expect::OutsideAllowedValues),
        ("refusals-redis", "timeout", "-1", Expect::OutsideAllowedValues),
        ("refusals-redis", "repl-backlog-size", "16383", Expect::OutsideAllowedValues),
        ("refusals-redis", "maxclients", "100", Expect::NotSettable),
        ("refusals-redis", "maxmemory", "1048576", Expect::NotSettable),
        ("refusals-redis", "chunk_size", "64", Expect::NotSettable),
        ("refusals-redis", "no-such-parameter", "1", Expect::NotSettable),
        ("refusals-memcached", "chunk_size_growth_factor", "not-a-float", Expect::NotOfType),
        ("refusals-memcached", "cas_disabled", "2", Expect::NotOfType),
        ("refusals-memcached", "slab_automove", "3", Expect::OutsideAllowedValues),
    ];
    for (group, parameter, value, expect) in cases {
        let case = format!("{group}/{parameter}={value:?} ({expect:?})");
        // What the group reports for the parameter BEFORE the refused request,
        // so the read below shows the request changed nothing.
        let before = match expect {
            Expect::NotSettable => None,
            _ => Some(effective(&emulator, &case, group, parameter).await),
        };

        let reply = modify(&emulator, group, &[(parameter, value)]).await;
        let message = refused(&case, &reply, 400, "InvalidParameterValue").to_string();
        match expect {
            Expect::NotOfType => {
                let (_, data_type) = published(&emulator, &case, group, parameter).await;
                assert!(
                    message.contains(&data_type) && message.contains(value),
                    "case {case}: the refusal must name the data type {data_type} the value is \
                     not one of, got {message}"
                );
            }
            Expect::OutsideAllowedValues => {
                let (allowed, _) = published(&emulator, &case, group, parameter).await;
                assert!(
                    !allowed.is_empty(),
                    "case {case}: this row needs a published value set to be outside of"
                );
                assert!(
                    message.contains(&allowed),
                    "case {case}: the refusal must name the published set {allowed}, got {message}"
                );
            }
            // A parameter nothing may set has no published set to be outside
            // of; the refusal only has to name what was asked for.
            Expect::NotSettable => assert!(
                message.contains(parameter),
                "case {case}: the refusal must name the parameter asked for, got {message}"
            ),
        }

        if let Some((value_before, source_before)) = before {
            let after = effective(&emulator, &case, group, parameter).await;
            assert_eq!(
                after,
                (value_before.clone(), source_before.clone()),
                "case {case}: a refused request must leave {value_before}/{source_before} in \
                 effect, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_pair_the_catalog_refuses_leaves_the_whole_request_unapplied() {
    let emulator = Emulator::new();
    create_group(&emulator, "atomic", "redis7").await;

    // A request is one change: a pair the catalog refuses refuses the request,
    // so the pairs beside it are not applied either — whichever side of the bad
    // pair they are on.
    for (case, pairs) in [
        (
            "the bad pair last",
            &[
                ("maxmemory-policy", "allkeys-random"),
                ("timeout", "nonsense"),
            ][..],
        ),
        (
            "the bad pair first",
            &[
                ("timeout", "nonsense"),
                ("maxmemory-policy", "allkeys-random"),
            ][..],
        ),
    ] {
        let reply = modify(&emulator, "atomic", pairs).await;
        refused(case, &reply, 400, "InvalidParameterValue");
        for (parameter, _) in pairs {
            let (_, source) = effective(&emulator, case, "atomic", parameter).await;
            assert_eq!(
                source,
                state::PARAMETER_SOURCE_SYSTEM,
                "case {case}: {parameter} must be left unset, got source {source}"
            );
        }
    }
}

#[tokio::test]
async fn the_documented_pair_window_bounds_one_request() {
    let emulator = Emulator::new();

    // "A maximum of 20 parameters may be modified per request", and "you must
    // supply at least one parameter name and value": the axis is the number of
    // pairs, at each end of that window and one past each.
    let pairs = settable("redis7", state::MAX_PARAMETERS_PER_REQUEST + 1);
    let full = borrowed(&pairs[..state::MAX_PARAMETERS_PER_REQUEST]);
    let over = borrowed(&pairs);

    create_group(&emulator, "window-full", "redis7").await;
    modify_ok(&emulator, "the documented maximum", "window-full", &full).await;
    for (parameter, _) in &full {
        let (_, source) = effective(&emulator, "the documented maximum", "window-full", parameter)
            .await;
        assert_eq!(
            source,
            state::PARAMETER_SOURCE_USER,
            "case the documented maximum: {parameter} must be set, got source {source}"
        );
    }

    create_group(&emulator, "window-over", "redis7").await;
    let refusal = modify(&emulator, "window-over", &over).await;
    refused("one past the documented maximum", &refusal, 400, "InvalidParameterValue");
    for (parameter, _) in &over {
        let (_, source) = effective(
            &emulator,
            "one past the documented maximum",
            "window-over",
            parameter,
        )
        .await;
        assert_eq!(
            source,
            state::PARAMETER_SOURCE_SYSTEM,
            "case one past the documented maximum: {parameter} must be left unset, got source \
             {source}"
        );
    }

    // No pair at all asks for no change. `ParameterNameValues` is `@required`,
    // so the codec may refuse the absent list before the handler sees it;
    // either way the request is refused and nothing is set.
    let empty = modify(&emulator, "window-over", &[]).await;
    assert_eq!(
        empty.status, 400,
        "case no pairs: must be refused 400, got {} {}",
        empty.status, empty.body
    );
    let reply = describe_ok(&emulator, "window-over", &[]).await;
    for parameter in reported(&reply) {
        assert_eq!(
            parameter.field("Source"),
            Some(state::PARAMETER_SOURCE_SYSTEM),
            "case no pairs: {} must be left unset, got {:?}",
            parameter.name,
            parameter.field("Source")
        );
    }
}

#[tokio::test]
async fn a_pair_that_does_not_name_a_parameter_and_a_value_is_refused() {
    let emulator = Emulator::new();
    create_group(&emulator, "pairs", "redis7").await;

    // Both members of `ParameterNameValue` are optional
    // (`smithy_query(query="shape", shape="ParameterNameValue")`), so a pair
    // that carries only one of them reaches the service; neither asks for a
    // change it can make. The axis: which member the pair leaves out.
    let cases: [(&str, &[(&str, &str)]); 4] = [
        (
            "no value",
            &[("ParameterNameValues.ParameterNameValue.1.ParameterName", "timeout")],
        ),
        (
            "no name",
            &[("ParameterNameValues.ParameterNameValue.1.ParameterValue", "300")],
        ),
        (
            "an empty name",
            &[
                ("ParameterNameValues.ParameterNameValue.1.ParameterName", ""),
                ("ParameterNameValues.ParameterNameValue.1.ParameterValue", "300"),
            ],
        ),
        (
            "a good pair beside one with no value",
            &[
                ("ParameterNameValues.ParameterNameValue.1.ParameterName", "timeout"),
                ("ParameterNameValues.ParameterNameValue.1.ParameterValue", "300"),
                ("ParameterNameValues.ParameterNameValue.2.ParameterName", "slowlog-max-len"),
            ],
        ),
    ];
    for (case, extra) in cases {
        let mut form = params(&[("CacheParameterGroupName", "pairs")]);
        form.extend(params(extra));
        let reply = emulator.call("ModifyCacheParameterGroup", &form).await;
        refused(case, &reply, 400, "InvalidParameterValue");
        // Nothing was set, including the good pair of the last row.
        let (_, source) = effective(&emulator, case, "pairs", "timeout").await;
        assert_eq!(
            source,
            state::PARAMETER_SOURCE_SYSTEM,
            "case {case}: a refused request must set nothing, got source {source}"
        );
    }
}

#[tokio::test]
async fn one_parameter_asked_for_twice_is_one_change_or_a_refusal() {
    let emulator = Emulator::new();
    create_group(&emulator, "twice", "redis7").await;

    // The same value twice asks for one change, which the service can make.
    modify_ok(
        &emulator,
        "the same value twice",
        "twice",
        &[("timeout", "300"), ("timeout", "300")],
    )
    .await;
    let (in_effect, source) = effective(&emulator, "the same value twice", "twice", "timeout").await;
    assert_eq!(
        (in_effect.as_str(), source.as_str()),
        ("300", state::PARAMETER_SOURCE_USER),
        "case the same value twice: expected 300 as the customer's, got {in_effect}/{source}"
    );

    // Two different values for one parameter cannot both hold, which is what
    // `InvalidParameterCombination` ("Two or more incompatible parameters were
    // specified") refuses — and the request changes nothing, so the value set
    // above survives it.
    let reply = modify(
        &emulator,
        "twice",
        &[("timeout", "600"), ("timeout", "900")],
    )
    .await;
    refused(
        "two values for one parameter",
        &reply,
        400,
        "InvalidParameterCombination",
    );
    let (kept, kept_source) =
        effective(&emulator, "two values for one parameter", "twice", "timeout").await;
    assert_eq!(
        (kept.as_str(), kept_source.as_str()),
        ("300", state::PARAMETER_SOURCE_USER),
        "case two values for one parameter: the earlier value must survive, got {kept}/\
         {kept_source}"
    );
}

#[tokio::test]
async fn a_group_the_account_cannot_modify_is_refused() {
    let emulator = Emulator::new();
    create_group(&emulator, "holder", "redis7").await;
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
        let reply = modify(&emulator, group, &[("timeout", "300")]).await;
        let message = refused(group, &reply, 404, "CacheParameterGroupNotFound");
        assert!(
            message.contains(group),
            "case {group}: the refusal must name the group asked for, got {message}"
        );
    }

    // Every family's own default group is the service's, not the customer's:
    // its values ARE the engine defaults, so none of its parameters may be set,
    // and a refused request leaves it reporting them all.
    for family in state::PARAMETER_GROUP_FAMILIES {
        let group = format!("default.{family}");
        let reply = modify(&emulator, &group, &[("timeout", "300")]).await;
        let message = refused(&group, &reply, 400, "InvalidParameterValue");
        assert!(
            message.contains(&group),
            "case {group}: the refusal must name the group asked for, got {message}"
        );
        let described = describe_ok(&emulator, &group, &[]).await;
        for parameter in reported(&described) {
            assert_eq!(
                parameter.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {group}: {} must still report the engine's own value, got {:?}",
                parameter.name,
                parameter.field("Source")
            );
        }
    }

    // A `CacheParameterGroupName` "is stored as a lowercase string", so a
    // custom group is modified however the request spells it, and the one
    // stored group is what changes.
    modify_ok(&emulator, "another case", "HOLDER", &[("timeout", "300")]).await;
    let (in_effect, source) = effective(&emulator, "another case", "holder", "timeout").await;
    assert_eq!(
        (in_effect.as_str(), source.as_str()),
        ("300", state::PARAMETER_SOURCE_USER),
        "case another case: the stored group must carry the change, got {in_effect}/{source}"
    );
}

#[tokio::test]
async fn the_change_is_recorded_as_an_event_of_the_group() {
    let emulator = Emulator::new();
    create_group(&emulator, "logged", "redis7").await;
    modify_ok(&emulator, "an event", "logged", &[("timeout", "300")]).await;

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
    // The create records one event and the change another, both under the
    // group's own source.
    let messages = reply.all("Event/Message");
    assert!(
        messages.len() >= 2,
        "a changed group must have recorded an event beside its create, got {messages:?}"
    );
    for identifier in reply.all("Event/SourceIdentifier") {
        assert_eq!(
            identifier, "logged",
            "every event selected must belong to the group, got {identifier}"
        );
    }
}

#[tokio::test]
async fn a_change_the_engine_applies_only_at_a_restart_waits_for_a_reboot() {
    let emulator = Emulator::new();

    // `Parameter$ChangeType` is the axis: a parameter the engine applies at
    // once leaves the clusters using the group in sync, and one it applies only
    // at a restart leaves them owed a reboot. The published catalogs put the
    // two change types on different engines — every Redis OSS parameter a
    // customer may modify is applied immediately, and every settable Memcached
    // one needs a restart — so each row runs on the engine that has it.
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
            ("NumCacheNodes", "3"),
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

    // Before anything is changed both clusters run their group's values.
    for cluster in ["immediate-cluster", "reboot-cluster"] {
        let (apply_status, ids) = status(&emulator, cluster, cluster).await;
        assert_eq!(
            (apply_status.as_str(), ids.as_slice()),
            (state::PARAMETER_APPLY_STATUS, &[][..]),
            "case {cluster}: a cluster whose group is untouched is {}, got {apply_status} \
             {ids:?}",
            state::PARAMETER_APPLY_STATUS
        );
    }

    // A value the engine applies at once changes nothing about the cluster's
    // association.
    modify_ok(
        &emulator,
        "an immediate change",
        "immediate-group",
        &[("maxmemory-policy", "allkeys-lru")],
    )
    .await;
    let (apply_status, ids) = status(&emulator, "an immediate change", "immediate-cluster").await;
    assert_eq!(
        (apply_status.as_str(), ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case an immediate change: expected {}, got {apply_status} {ids:?}",
        state::PARAMETER_APPLY_STATUS
    );

    // A value the engine picks up only when a node restarts leaves every node
    // of the cluster owed a reboot.
    modify_ok(
        &emulator,
        "a deferred change",
        "reboot-group",
        &[("binding_protocol", "ascii")],
    )
    .await;
    let (apply_status, ids) = status(&emulator, "a deferred change", "reboot-cluster").await;
    assert_eq!(
        apply_status,
        state::PARAMETER_APPLY_STATUS_PENDING_REBOOT,
        "case a deferred change: expected {}, got {apply_status}",
        state::PARAMETER_APPLY_STATUS_PENDING_REBOOT
    );
    assert_eq!(
        ids,
        vec![
            "0001".to_string(),
            "0002".to_string(),
            "0003".to_string()
        ],
        "case a deferred change: every node of the cluster must need a reboot, got {ids:?}"
    );
    // The cluster that uses the OTHER group is unaffected: the change is the
    // group's, and only the clusters associated with it are owed anything.
    let (other_status, other_ids) =
        status(&emulator, "a deferred change elsewhere", "immediate-cluster").await;
    assert_eq!(
        (other_status.as_str(), other_ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case a deferred change elsewhere: expected {}, got {other_status} {other_ids:?}",
        state::PARAMETER_APPLY_STATUS
    );

    // "This operation applies any modified cache parameter groups to the
    // cluster": a reboot of SOME of the nodes applies the group to those nodes
    // alone, so the rest are still owed one.
    let rebooted = emulator
        .call(
            "RebootCacheCluster",
            &params(&[
                ("CacheClusterId", "reboot-cluster"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0001"),
                ("CacheNodeIdsToReboot.CacheNodeId.2", "0002"),
            ]),
        )
        .await;
    assert_eq!(
        rebooted.status, 200,
        "RebootCacheCluster must answer 200, got {} {}",
        rebooted.status, rebooted.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let (apply_status, ids) = status(&emulator, "a partial reboot", "reboot-cluster").await;
    assert_eq!(
        (apply_status.as_str(), ids.as_slice()),
        (
            state::PARAMETER_APPLY_STATUS_PENDING_REBOOT,
            &["0003".to_string()][..]
        ),
        "case a partial reboot: only the node that was not rebooted is owed one, got \
         {apply_status} {ids:?}"
    );

    // Once every node has been restarted the cluster runs the group's values.
    let rebooted = emulator
        .call(
            "RebootCacheCluster",
            &params(&[
                ("CacheClusterId", "reboot-cluster"),
                ("CacheNodeIdsToReboot.CacheNodeId.1", "0003"),
            ]),
        )
        .await;
    assert_eq!(
        rebooted.status, 200,
        "the second RebootCacheCluster must answer 200, got {} {}",
        rebooted.status, rebooted.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let (apply_status, ids) = status(&emulator, "a full reboot", "reboot-cluster").await;
    assert_eq!(
        (apply_status.as_str(), ids.as_slice()),
        (state::PARAMETER_APPLY_STATUS, &[][..]),
        "case a full reboot: expected {}, got {apply_status} {ids:?}",
        state::PARAMETER_APPLY_STATUS
    );
}
