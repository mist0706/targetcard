/*
 * The players table: the latest arrivals, name | class | cp.
 *
 * The packet stream gives names and classes for free, and combat power for
 * party members. For anyone else it comes from NC's character API: one lookup
 * per name, cached on disk for six hours by the Rust side.
 *
 * Three states, and they mean different things:
 *   …  a lookup is running, or one is due
 *   —  we asked and got nothing (no such character, or the request failed)
 *   72,140  the number
 *
 * No inline script and no inline handlers: the app's CSP allows `script-src
 * 'self'` only, so everything lives in this file.
 */
(() => {
  const POLL_MS = 1000;
  const MAX_LOOKUPS = 2;              // in flight at once, to stay polite
  const FALLBACK_SERVER = "Phernos";

  // A lookup that never settled used to block the queue for the rest of the
  // session: two of them, and every later name waited behind them forever,
  // which is what "stays grey and never updates" looked like. The timeout makes
  // that impossible, and the two retry windows stop a name being written off
  // for good after one bad minute.
  const LOOKUP_TIMEOUT_MS = window.__TC_LOOKUP_TIMEOUT ?? 20_000;
  const RETRY_FAILED_MS = 60_000;        // network or timeout: again in a minute
  const RETRY_EMPTY_MS = 30 * 60_000;     // no such character: again in half an hour
  // A name several players share, with no class to settle it yet. The class
  // arrives once the fight starts, so ask again soon rather than in half an hour.
  const RETRY_AMBIGUOUS_MS = 120_000;

  const invoke = window.__TAURI__?.core?.invoke;
  const rowsHost = () => document.getElementById("tc-rows");
  const numberFormat = new Intl.NumberFormat("en-US");

  const rows = new Map();        // entity_id -> { row, name, dot, label, cp }
  const scores = new Map();      // lowercase name -> score object
  const retryAt = new Map();     // lowercase name -> when another attempt is allowed
  const queued = new Set();      // names in the queue or in flight
  const queue = [];
  let inFlight = 0;
  let pumping = false;
  let lastRecent = [];
  // In the Abyss the table shows hostiles only. That needs our own faction, which
  // costs one lookup of our own name and is then remembered for the session.
  let inAbyss = false;
  let inArena = false;
  let selfRace = null;
  let selfLevel = null;
  let askingSelf = false;
  let server = FALLBACK_SERVER;

  // The parser calls it Elementalist; the game and the API call it Spiritmaster.
  function className(player, score) {
    const raw = player.class || score?.class_name || "";
    if (!raw) return "";
    return raw === "Elementalist" ? "Spiritmaster" : raw;
  }

  function withTimeout(promise, ms) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`no answer after ${ms} ms`)), ms);
      promise.then(
        (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        (error) => {
          clearTimeout(timer);
          reject(error);
        },
      );
    });
  }

  function emptyRow(text) {
    const div = document.createElement("div");
    div.className = "empty";
    div.setAttribute("data-tauri-drag-region", "");
    div.textContent = text;
    return div;
  }

  function makeRow() {
    const row = document.createElement("div");
    row.className = "row";
    const name = document.createElement("span");
    name.className = "name";
    const cls = document.createElement("span");
    cls.className = "class";
    const dot = document.createElement("i");
    dot.className = "dot";
    const label = document.createElement("span");
    label.className = "clabel";
    cls.append(dot, label);
    const cp = document.createElement("span");
    cp.className = "cp";
    row.append(name, cls, cp);
    return { row, name, dot, label, cp };
  }

  function render() {
    const host = rowsHost();
    const fresh = document.createDocumentFragment();
    const seen = new Set();
    const now = Date.now();

    lastRecent.forEach((player, index) => {
      seen.add(player.entity_id);
      let parts = rows.get(player.entity_id);
      if (!parts) {
        parts = makeRow();
        rows.set(player.entity_id, parts);
      }

      parts.name.textContent = player.name;
      parts.name.title = player.name;
      parts.row.classList.toggle("fresh", index === 0);

      const key = player.name.toLowerCase();
      const score = scores.get(key) || null;

      // Abyss: keep hostiles, drop our own side. A row whose faction is not known
      // yet stays visible rather than blinking out and back; it disappears as soon
      // as the answer says it was a teammate.
      const race = score?.race ?? null;
      if (inAbyss && selfRace != null && race != null && race === selfRace) {
        seen.add(player.entity_id);
        return;
      }
      const cls = className(player, score);
      // A dash says "asked, nothing there". An ellipsis promises an answer, so
      // it is only shown while a lookup is running or due.
      const waiting = (retryAt.get(key) ?? 0) <= now;

      const dotClass = cls ? cls.toLowerCase().replace(/[^a-z]/g, "") : "";
      parts.dot.className = `dot ${dotClass}`;
      parts.label.className = `clabel ${dotClass}`;
      parts.label.textContent = cls || (waiting ? "…" : "—");

      const power = player.combat_power ?? score?.combat_power ?? null;
      if (power != null) {
        parts.cp.textContent = numberFormat.format(power);
        parts.cp.classList.remove("pending");
      } else {
        parts.cp.textContent = waiting ? "…" : "—";
        parts.cp.classList.add("pending");
      }

      fresh.append(parts.row);
    });

    for (const id of [...rows.keys()]) {
      if (!seen.has(id)) rows.delete(id);
    }

    const nothing = inAbyss && selfRace != null ? "no hostiles in range" : "waiting for players";
    host.replaceChildren(fresh.childNodes.length ? fresh : emptyRow(nothing));
  }

  // The class from the packet stream for whichever row this name belongs to, if
  // one is on screen. Not the API's answer: that is what we are trying to get.
  function classOf(name) {
    const key = name.toLowerCase();
    const row = lastRecent.find((player) => player.name.toLowerCase() === key);
    return row?.class || null;
  }

  function enqueue(player) {
    const key = player.name.toLowerCase();
    if (scores.has(key) || queued.has(key)) return;
    if ((retryAt.get(key) ?? 0) > Date.now()) return; // still cooling down
    queued.add(key);
    queue.push(player.name);
    pump();
  }

  async function pump() {
    if (pumping) return; // the running loop picks up anything added meanwhile
    pumping = true;
    try {
      while (inFlight < MAX_LOOKUPS && queue.length) {
        const name = queue.shift();
        const key = name.toLowerCase();
        inFlight += 1;
        try {
          // The class the game showed travels with the request: names are not
          // unique (five level 45 players called Tails across EU), so when the
          // name is shared the class is what tells them apart.
          const reply = await withTimeout(
            invoke("targetcard_score", {
              name,
              server,
              class: classOf(name),
              floor: levelFloor(),
              arena: inArena,
              knownServer: player.server_id ?? null,
            }),
            LOOKUP_TIMEOUT_MS,
          );
          if (reply?.score) {
            scores.set(key, reply.score);
            retryAt.delete(key);
          } else if (reply?.reason === "ambiguous") {
            retryAt.set(key, Date.now() + RETRY_AMBIGUOUS_MS);
          } else {
            // A definite answer: nobody by that name that we can find.
            retryAt.set(key, Date.now() + RETRY_EMPTY_MS);
          }
        } catch (error) {
          console.error("targetcard: lookup failed for", name, error);
          retryAt.set(key, Date.now() + RETRY_FAILED_MS);
        } finally {
          inFlight -= 1;
          queued.delete(key);
          render();
        }
      }
    } finally {
      pumping = false;
    }
  }

  /// Learn our own faction and level from our own character sheet. One request,
  /// once per session. The faction is what the Abyss filter compares against; the
  /// level is what stops a low-level namesake on our server from answering for
  /// somebody we are actually fighting.
  async function askSelf(name, serverName) {
    if ((selfRace != null && selfLevel != null) || askingSelf || !name) return;
    askingSelf = true;
    try {
      const reply = await withTimeout(
        invoke("targetcard_score", {
          name,
          server: serverName || server,
          class: null,
          floor: null,
          arena: false,
        }),
        LOOKUP_TIMEOUT_MS,
      );
      const race = reply?.score?.race ?? null;
      const level = reply?.score?.level ?? null;
      if (race != null) selfRace = race;
      if (level != null) selfLevel = level;
      if (race != null || level != null) {
        console.log(
          "targetcard: we are",
          race === 1 ? "Elyos" : race === 2 ? "Asmodian" : "of unknown faction",
          level ? `at level ${level}` : "at an unknown level",
        );
        render();
      }
    } catch (error) {
      console.error("targetcard: could not read our own character", error);
    } finally {
      askingSelf = false;
    }
  }

  /// The lowest level worth considering for the people in this table. Level 45 is
  /// where PvP happens, and a name shared with an alt twenty levels below us is not
  /// the person on screen.
  function levelFloor() {
    if (selfLevel == null || selfLevel < 30) return null;
    return selfLevel - 5;
  }

  async function tick() {
    try {
      const snapshot = await withTimeout(invoke("targetcard_snapshot"), LOOKUP_TIMEOUT_MS);
      server = snapshot?.server_name || FALLBACK_SERVER;
      lastRecent = snapshot?.recent || [];
      inAbyss = !!snapshot?.in_abyss;
      inArena = !!snapshot?.in_arena;
      render();
      askSelf(snapshot?.local_name, snapshot?.server_name);

      for (const player of lastRecent) {
        if (player.combat_power == null) enqueue(player);
      }
    } catch (error) {
      console.error("targetcard: snapshot failed", error);
    } finally {
      setTimeout(tick, POLL_MS);
    }
  }

  if (!invoke) {
    console.error("targetcard: __TAURI__ is not available; the table cannot talk to the app");
    return;
  }
  window.addEventListener("DOMContentLoaded", tick);
})();
