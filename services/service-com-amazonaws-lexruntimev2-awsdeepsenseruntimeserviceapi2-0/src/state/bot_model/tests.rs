// SPDX-License-Identifier: CC0-1.0

//! The sentence this module hands an operation for a `botId`/`botAliasId` pair
//! that resolves to no alias of that bot — the one refusal of this service whose
//! message has variable parts, so what it renders is worth pinning on its own.
//! The resolution itself is driven end to end over HTTP in
//! `tests/all/bot_addressing.rs`, against a bot the control plane built.

use super::*;

/// RULE: the sentence names the two ids the request addressed the alias by, in
/// that order, and the text after them is fixed — so it is the same sentence
/// whatever the ids, and never the sentence a locale with no built model gets.
#[test]
fn an_unresolved_pair_is_stated_as_the_pair_the_request_carried() {
    // (case, botId, botAliasId) — the rows differ along the character classes
    // and the case of the two ids, which the modelled `Id` (ten `[0-9a-zA-Z]`)
    // and `BotAliasId` (`^(\bTSTALIASID\b|[0-9a-zA-Z]+)$`) admit.
    let rows: [(&str, &str, &str); 4] = [
        ("a minted bot id and the test alias", "BOTIDBOTID", "TSTALIASID"),
        ("an alias id of no alias", "BOTIDBOTID", "XXXXXXXXXX"),
        ("ids of digits", "0123456789", "9876543210"),
        ("lower-case ids", "zzzzzzzzzz", "liveailas1"),
    ];

    for (case, bot_id, bot_alias_id) in rows {
        let actual = bot_alias_not_valid_message(bot_id, bot_alias_id);
        let expected =
            format!("{bot_id}/{bot_alias_id} is not a valid BotId/BotAliasId combination.");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert_ne!(
            actual, BOT_LOCALE_NOT_VALID_MESSAGE,
            "case {case:?}: an unresolved pair must not be stated as a locale with no built \
             model, got {actual:?}"
        );
    }
}
