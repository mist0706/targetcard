# targetcard: a fork of the A2Tools DPS meter

Base: `taengu/A2Tools-DPS-Meter` at commit `2d75523` (v2.0.76, 2026-10-23).

Upstream is GPL-3.0. This fork is therefore GPL-3.0 as well, and this file is the
record of what differs so the difference stays reviewable and mergeable.

## What this fork changes

The meter answers "how much damage is everyone doing". This fork answers "who is
this player, and how geared are they":

1. `src-tauri/src/targetcard/mod.rs` — pure derivation of player cards from the
   existing `DataStorage` snapshots: `PlayerCard { entity_id, name, class,
   is_self }`, `build_cards()`, `focus_card()`. No I/O, no UI, and deliberately
   inside the parser core so the wasm purity check keeps covering it.
2. `src-tauri/src/targetcard/snapshot.rs` — `TargetSnapshot` and the `targetcard_snapshot`
   command. Player set = authoritative `44/45 36` identity records (union) confirmed
   player actors, so a nearby player who has not fought still gets a card and a mob
   never does.
3. `src-tauri/src/targetcard/score.rs` — pure URL building, the EU server-id table,
   and `parse_score()` for NC's `/api/character/info`. Gear score is the `statList`
   entry with `type == "ItemLevel"`; the display name is localised, so matching on
   `name` misses it. Combat power is `profile.combatPower`.
   `score_service.rs` (desktop only) adds the `reqwest` client, a six-hour on-disk
   cache, and `targetcard_score`.
4. `public/targetcard.html` plus `public/src/js/targetcard.{js,css}` — a small
   always-on-top card window, 260x104, dragging via `data-tauri-drag-region`,
   polling the snapshot once a second and looking each name up once.
5. `app.rs` hooks: the two commands in `invoke_handler`, `ScoreServiceState` in
   managed state, and `"targetcard"` added to `TOOL_WINDOWS` so the card hides with
   the meter, keeps above the game while the game is in front, and remembers where
   it was dragged.
6. `package.json` name, so a built binary is not mistaken for an official release.

`identifier` in `tauri.conf.json` is deliberately left at `com.a2tools.dps-meter`:
it decides where config and history live, and changing it would orphan the data of
an existing install.

## What this fork does not do

- It does not upload anything to a2tools.app. Build with
  `npm run build:offline` (`--no-default-features --features desktop`): the
  `online` feature, and with it uploads, presence and the updater, links nothing.
- It does not modify the game client, its memory, or its files. It reads this
  machine's own network traffic, the same way upstream does.
- It does not present itself as A2Tools. Upstream is credited above; issues with
  this fork belong here, not upstream.

## Character data and third parties

Gear score lookups go to `aion-api.bnshive.com`, the backend behind
`aion2.bnshive.com`. That endpoint currently serves TW/CN servers and rejects
global ones such as `Phernos`, so on global the card shows a name and class but no
number. Only the character name and server are sent, one request per name per six
hours, cached locally. Nothing else about any player leaves the machine.

## Terms of service

Passive packet capture does not touch the client, but NCSOFT's terms of service
likely prohibit third-party tooling regardless, and account action is how that gets
enforced. That is the user's call to make, made once, with eyes open.

## Rebase notes

Keep the patch additive: new `targetcard` module, new page, new JS, and one-line
hooks in `lib.rs`, `app.rs`, `data_storage.rs`, `tauri.conf.json`. After every
upstream rebase run `cargo test --no-default-features targetcard` and replay one
recorded capture through the card before trusting the player list.
