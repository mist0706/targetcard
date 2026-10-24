/*
 * The target card's page: poll the backend once a second, show the focused
 * player, look their gear score up once per name.
 *
 * No inline script and no inline handlers: the app's CSP allows `script-src
 * 'self'` only, so everything lives in this file.
 */
(() => {
  const POLL_MS = 1000;
  const FALLBACK_SERVER = "Phernos";

  const invoke = window.__TAURI__?.core?.invoke;
  const el = (id) => document.getElementById(id);

  let pollBusy = false;
  let lookingUp = null; // name currently being resolved, lowercase
  const scores = new Map(); // lowercase name -> { item_level, combat_power, class_name }
  const misses = new Set(); // lowercase names that returned nothing, so we do not retry every second

  function fmtPower(value) {
    if (!value) return "";
    return value >= 1000 ? `${Math.round(value / 100) / 10}k CP` : `${value} CP`;
  }

  function showEmpty() {
    document.body.classList.add("is-empty");
    lookingUp = null;
  }

  function render(player, score) {
    document.body.classList.remove("is-empty");

    const name = el("tc-name");
    name.textContent = player.name;
    name.title = player.name;

    // The card's own class comes from the packet stream; the lookup can fill it
    // in when the stream has not named a class yet.
    const className = player.class || score?.class_name || "";
    el("tc-class").textContent = className;

    // The roster (party members) states level, gear score and combat power
    // outright, so prefer it and skip the web lookup entirely for those.
    const level = player.level ?? score?.level;
    el("tc-level").textContent = level ? `Lv ${level}` : "";

    const gear = player.gear_score ?? score?.item_level;
    const power = player.combat_power ?? score?.combat_power;
    const gs = el("tc-gs");
    if (gear || power) {
      gs.textContent = gear ? `GS ${gear}` : "GS ?";
      gs.classList.remove("is-pending");
      el("tc-cp").textContent = fmtPower(power);
      el("tc-hint").textContent = "";
    } else {
      gs.textContent = "GS ...";
      gs.classList.add("is-pending");
      el("tc-cp").textContent = "";
      el("tc-hint").textContent = lookingUp === player.name.toLowerCase() ? "looking up" : "";
    }
  }

  async function lookup(player, server) {
    const key = player.name.toLowerCase();
    lookingUp = key;
    try {
      const score = await invoke("targetcard_score", { name: player.name, server });
      if (score) {
        scores.set(key, score);
      } else {
        misses.add(key);
      }
    } catch (error) {
      console.error("targetcard: score lookup failed", error);
      misses.add(key);
    } finally {
      if (lookingUp === key) lookingUp = null;
    }
  }

  async function tick() {
    if (pollBusy) return;
    pollBusy = true;
    try {
      const snapshot = await invoke("targetcard_snapshot");
      const server = snapshot?.server_name || FALLBACK_SERVER;
      const focusId = snapshot?.focus_entity_id;
      const player = (snapshot?.players || []).find((p) => p.entity_id === focusId);

      if (!player) {
        showEmpty();
        return;
      }

      const key = player.name.toLowerCase();
      const score = scores.get(key) || null;
      render(player, score);

      if (player.gear_score == null && !score && !misses.has(key) && lookingUp !== key) {
        // Fire and forget: the next tick renders whatever came back.
        lookup(player, server);
        render(player, null);
      }
    } catch (error) {
      console.error("targetcard: snapshot failed", error);
      showEmpty();
    } finally {
      pollBusy = false;
      setTimeout(tick, POLL_MS);
    }
  }

  if (!invoke) {
    console.error("targetcard: __TAURI__ is not available; the card cannot talk to the app");
    return;
  }
  window.addEventListener("DOMContentLoaded", tick);
})();
