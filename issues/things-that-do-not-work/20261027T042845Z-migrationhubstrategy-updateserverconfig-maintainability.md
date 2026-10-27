Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# The strategyOption acceptance rule is implemented twice, once in UpdateServerConfig and once in UpdateApplicationComponentConfig

_Filed: 2026-10-27T04:28:44Z_
_Operation: UpdateServerConfig | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule for what a `strategyOption` the service accepts and stores looks like — the three enum-membership checks (`Strategy`, `TargetDestination`, `TransformationToolName`) plus the "an option carrying only `isPreferred` names no strategy, so there is nothing to store" decision — is written out twice in this crate, in the two config-update handlers, with identical bodies and identical doc comments. The two copies can drift: adding a fourth member to `StrategyOption`, or changing which values are admitted, has to be done in both places or the server preference and the component preference start accepting different options for the same modelled shape. The crate's own convention is to state such a rule once in `state.rs` (`check_enum`, `check_group_names`, `filter_pair`, `sort_descending`, `strategy_listings`, `recommendation_of_option` are all shared there) and to let each handler only lift the resulting `ValidationException` into its own error channel. The fix is to move the shared body beside `crate::state::check_enum` and have both handlers call it; every response and every existing test stays unchanged.

## Evidence

Copy 1 — `src/ops/update_server_config.rs:140-159`:

    /// The strategy option this request stores against the server, or `None` when it names
    /// none to store.
    ///
    /// `isPreferred` alone selects no strategy, no destination and no tool, so an option
    /// carrying nothing else is nothing to store ...
    fn preference_of(
        option: Option<StrategyOption>,
    ) -> Result<Option<StrategyOption>, UpdateServerConfigError> {
        let Some(option) = option else {
            return Ok(None);
        };
        check_enum(option.strategy.as_deref(), STRATEGY_VALUES)?;
        check_enum(option.target_destination.as_deref(), TARGET_DESTINATION_VALUES)?;
        check_enum(option.tool_name.as_deref(), TRANSFORMATION_TOOL_NAME_VALUES)?;
        let names_a_strategy = option.strategy.is_some()
            || option.target_destination.is_some()
            || option.tool_name.is_some();
        Ok(names_a_strategy.then_some(option))
    }

Copy 2 — `src/ops/update_application_component_config.rs:183-202`, same body under the same doc:

    fn preference_of(
        option: Option<StrategyOption>,
    ) -> Result<Option<StrategyOption>, UpdateApplicationComponentConfigError> {
        let Some(option) = option else {
            return Ok(None);
        };
        check_enum(option.strategy.as_deref(), STRATEGY_VALUES)?;
        check_enum(option.target_destination.as_deref(), TARGET_DESTINATION_VALUES)?;
        check_enum(option.tool_name.as_deref(), TRANSFORMATION_TOOL_NAME_VALUES)?;
        let names_a_strategy = option.strategy.is_some()
            || option.target_destination.is_some()
            || option.tool_name.is_some();
        Ok(names_a_strategy.then_some(option))
    }

The crate already holds the membership test itself once — `src/state.rs:2447` `pub fn check_enum(value: Option<&str>, declared: &[&str]) -> Result<(), ValidationException>` — and each handler's local `check_enum` wrapper says so: "The membership test itself is the service's [`crate::state::check_enum`] ... this wrapper only lifts its `ValidationException` into this operation's error channel" (`src/ops/update_server_config.rs:128-138`, `src/ops/update_application_component_config.rs:168-181`).

Tests that pin the behaviour both copies must keep: `tests/all/server_config.rs:406-416` (`an_update_naming_no_strategy_option_changes_nothing`, rows "a strategyOption carrying only isPreferred" / "... only a false isPreferred") and `tests/all/server_config.rs:736` (`a_value_no_enumeration_declares_is_refused`); `tests/all/application_component_config.rs:476` (case "an option carrying only isPreferred", with the stated rule at lines 318-323) and `tests/all/application_component_config.rs:1088` (`a_value_this_service_does_not_accept_is_refused_and_stores_nothing`).

## Steps to Reproduce

1. Open `src/ops/update_server_config.rs` and read `preference_of` at lines 140-159.
2. Open `src/ops/update_application_component_config.rs` and read `preference_of` at lines 183-202: the body and the doc are the same rule a second time.
3. Note that `src/state.rs:2447` already hosts the shared `check_enum` the two copies call, which is where this crate states a rule once for every handler.
4. Run the existing suites `tests/all/server_config.rs` and `tests/all/application_component_config.rs`: they pass against either copy, so folding the two into one shared helper must leave both green and every response byte-identical.
