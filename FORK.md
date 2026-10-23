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
2. `src-tauri/src/targetcard/snapshot.rs` — a `TargetSnapshot` and the
   `targetcard_snapshot` command.
3. `src-tauri/src/targetcard/score*.rs` — gear score lookup and cache. The
   parser-side parse function is pure; the `reqwest` client is gated behind the
   `desktop` feature.
4. `public/targetcard.html` plus `public/src/js/targetcard.*` — a small
   always-on-top card window.
5. Single-line hooks in `src-tauri/src/lib.rs`, `src-tauri/src/app.rs`,
   `src-tauri/src/combat/data_storage.rs` (`get_actor_jobs`) and
   `src-tauri/tauri.conf.json` (`productName` and window title, nothing else).
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
