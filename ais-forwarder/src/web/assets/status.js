"use strict";

const REFRESH_SECONDS = 15;
let countdown = REFRESH_SECONDS;

function init() {
  buildGrid();
  refresh();
  setInterval(tick, 1000);
}

function tick() {
  countdown -= 1;
  if (countdown <= 0) {
    refresh();
  } else {
    document.getElementById("countdown").textContent = countdown;
  }
}

async function refresh() {
  countdown = REFRESH_SECONDS;
  document.getElementById("countdown").textContent = countdown;
  try {
    const res = await fetch("api/stats", { cache: "no-store" });
    if (!res.ok) throw new Error(res.status);
    render(await res.json());
    setStale(false);
  } catch (e) {
    setStale(true);
  }
}

function setStale(stale) {
  document.getElementById("refresh-dot").classList.toggle("stale", stale);
}

function render(d) {
  if (d.title) {
    document.querySelectorAll("[data-title]").forEach((el) => {
      el.textContent = d.title;
    });
    document.title = d.title + " — Status";
  }

  const i = d.input || {};
  set("in-vdm", num(i.vdm_messages));
  set("in-nonvdm", num(i.non_vdm_messages));
  set("in-crc", num(i.crc_errors));
  set("in-dup", num(i.duplicates));
  set("in-a", num(i.channel_a));
  set("in-b", num(i.channel_b));
  set("in-bytes", bytes(i.bytes));
  set("in-bw", rate(i.bandwidth));

  const o = d.output || {};
  set("out-ratio", (o.in_out_ratio || 0).toFixed(2));
  set("out-bytes", bytes(o.bytes));
  set("out-bw", rate(o.bandwidth));

  // Per-destination output counts.
  const dests = document.getElementById("dests");
  dests.innerHTML = "";
  for (const row of o.per_dest || []) {
    const el = document.createElement("div");
    el.className = "dest-row";
    el.innerHTML = "<span>" + esc(row.dest) + "</span><span>" + num(row.count) + "</span>";
    dests.appendChild(el);
  }

  // Message-type grid (indices 0..26 => MSG 1..27), plus INVALID in slot 28.
  const types = d.by_message_type || [];
  for (let n = 1; n <= 27; n++) {
    setType("type-" + n, types[n - 1] || 0);
  }
  setType("type-invalid", d.invalid || 0);
}

// Lay the 28 cells out column-major (1,8,15,22 / 2,9,16,23 / ...) to match the
// reference dispatcher's layout.
function buildGrid() {
  const grid = document.getElementById("typegrid");
  const labels = [];
  for (let n = 1; n <= 27; n++) labels.push({ id: "type-" + n, text: "MSG " + n });
  labels.push({ id: "type-invalid", text: "INVALID" });

  // 7 rows x 4 columns, column-major.
  const rows = 7;
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < 4; c++) {
      const idx = c * rows + r;
      if (idx >= labels.length) continue;
      const cell = document.createElement("div");
      cell.className = "type-cell zero";
      cell.id = "cell-" + labels[idx].id;
      cell.innerHTML =
        '<div class="type-label">' +
        labels[idx].text +
        '</div><div class="type-value" id="' +
        labels[idx].id +
        '">0</div>';
      grid.appendChild(cell);
    }
  }
}

function setType(id, value) {
  const el = document.getElementById(id);
  if (!el) return;
  el.textContent = num(value);
  const cell = document.getElementById("cell-" + id);
  if (cell) cell.classList.toggle("zero", !value);
}

function set(id, text) {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

function num(n) {
  return (n || 0).toLocaleString("en-US");
}

function bytes(n) {
  n = n || 0;
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(2) + " M";
  if (n >= 1024) return (n / 1024).toFixed(2) + " K";
  return n + " B";
}

function rate(n) {
  n = n || 0;
  if (n >= 1024) return (n / 1024).toFixed(2) + " K/s";
  return n.toFixed(2) + " B/s";
}

function esc(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

window.addEventListener("DOMContentLoaded", init);
