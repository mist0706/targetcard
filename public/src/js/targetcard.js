/*
 * The players table: the latest arrivals, name | class | cp.
 *
 * The packet stream gives names and classes for free, and combat power for
 * party members. For anyone else it comes from NC's character API: one lookup
 * per name, cached on disk for six hours by the Rust side, so the first sighting
 * of a player shows a dash for a moment and then stays filled in.
 *
 * No inline script and no inline handlers: the app's CSP allows `script-src
 * 'self'` only, so everything lives in this file.
 */
(() => {
  const POLL_MS = 1000;
  const MAX_LOOKUPS = 2;         // in flight at once, to stay polite to the API
  const FALLBACK_SERVER = "Phernos";

  const invoke = window.__TAURI__?.core?.invoke;
  const rowsHost = () => document.getElementById("tc-rows");
  const numberFormat = new Intl.NumberFormat("en-US");

  const rows = new Map();        // entity_id -> { row, name, cls, cp }
  const scores = new Map();      // lowercase name -> score object
  const misses = new Set();      // names the API had nothing for
  const queued = new Set();      // names already in the queue
  const queue = [];
  let inFlight = 0;
  let lastRecent = [];
  let server = FALLBACK_SERVER;

  // The parser calls it Elementalist; the game and the API call it Spiritmaster.
  function className(player, score) {
    const raw = player.class || score?.class_name || "";
    if (!raw) return "";
    return raw === "Elementalist" ? "Spiritmaster" : raw;
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

      const score = scores.get(player.name.toLowerCase()) || null;
      const cls = className(player, score);
      const dotClass = cls ? cls.toLowerCase().replace(/[^a-z]/g, "") : "";
      parts.dot.className = `dot ${dotClass}`;

      const missed = misses.has(player.name.toLowerCase());
      // No class from the packet stream and nothing from the API: a dash says
      // "asked, nothing there", where an ellipsis would promise an answer.
      parts.label.textContent = cls || (missed ? "—" : "…");

      const power = player.combat_power ?? score?.combat_power ?? null;
      if (power != null) {
        parts.cp.textContent = numberFormat.format(power);
        parts.cp.classList.remove("pending");
      } else {
        parts.cp.textContent = missed ? "—" : "…";
        parts.cp.classList.add("pending");
      }

      fresh.append(parts.row);
    });

    for (const id of [...rows.keys()]) {
      if (!seen.has(id)) rows.delete(id);
    }

    host.replaceChildren(fresh.childNodes.length ? fresh : emptyRow("waiting for players"));
  }

  function enqueue(player) {
    const key = player.name.toLowerCase();
    if (scores.has(key) || misses.has(key) || queued.has(key)) return;
    queued.add(key);
    queue.push(player.name);
    // The in-flight limit lives in pump(), not here: dropping a player from the
    // queue because two lookups are running would hold the rest until the next
    // poll, a whole second of dashes for a table that just filled up.
    pump();
  }

  async function pump() {
    while (inFlight < MAX_LOOKUPS && queue.length) {
      const name = queue.shift();
      const key = name.toLowerCase();
      inFlight += 1;
      try {
        const score = await invoke("targetcard_score", { name, server });
        if (score) scores.set(key, score);
        else misses.add(key);
      } catch (error) {
        console.error("targetcard: score lookup failed", error);
        misses.add(key);
      } finally {
        inFlight -= 1;
        queued.delete(key);
        render();
        pump();
      }
    }
  }

  async function tick() {
    try {
      const snapshot = await invoke("targetcard_snapshot");
      server = snapshot?.server_name || FALLBACK_SERVER;
      lastRecent = snapshot?.recent || [];
      render();

      for (const player of lastRecent) {
        if (player.combat_power == null) enqueue(player);
      }
    } catch (error) {
      console.error("targetcard: snapshot failed", error);
      lastRecent = [];
      render();
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
