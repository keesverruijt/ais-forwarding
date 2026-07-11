"use strict";

// Refresh cadence. A visible countdown discourages manual reloads.
const REFRESH_SECONDS = 15;

const DEFAULT_CENTER = [5.42, 53.18]; // [lng, lat] — Harlingen; overridden by station
const DEFAULT_ZOOM = 9;

// Raster OSM base + the OpenSeaMap seamark overlay (buoys, lights, harbours).
// Both are plain raster tiles, so no API key and local-language labels.
const MAP_STYLE = {
  version: 8,
  sources: {
    osm: {
      type: "raster",
      tiles: [
        "https://a.tile.openstreetmap.org/{z}/{x}/{y}.png",
        "https://b.tile.openstreetmap.org/{z}/{x}/{y}.png",
        "https://c.tile.openstreetmap.org/{z}/{x}/{y}.png",
      ],
      tileSize: 256,
      maxzoom: 19,
      attribution: "&copy; OpenStreetMap contributors",
    },
    seamark: {
      type: "raster",
      tiles: ["https://tiles.openseamap.org/seamark/{z}/{x}/{y}.png"],
      tileSize: 256,
      maxzoom: 18,
      attribution: "&copy; OpenSeaMap contributors",
    },
  },
  layers: [
    { id: "osm", type: "raster", source: "osm" },
    { id: "seamark", type: "raster", source: "seamark" },
  ],
};

let map;
let stationMarker = null;
const markers = new Map(); // mmsi -> maplibregl.Marker
let countdown = REFRESH_SECONDS;
let lastData = null;
let fitted = false; // have we done the one-time fit-to-data yet?

function init() {
  map = new maplibregl.Map({
    container: "map",
    style: MAP_STYLE,
    center: DEFAULT_CENTER,
    zoom: DEFAULT_ZOOM,
    maxZoom: 18,
    attributionControl: { compact: true },
  });

  map.addControl(new maplibregl.NavigationControl({ showCompass: true }), "top-left");
  map.addControl(new maplibregl.ScaleControl({ unit: "nautical" }), "bottom-left");
  map.addControl(new maplibregl.ScaleControl({ unit: "metric" }), "bottom-left");

  map.on("load", () => {
    refresh();
    setInterval(tick, 1000);
  });

  map.on("mousemove", (e) => updateCursor(e.lngLat));
  map.on("moveend", updateCount);

  document.getElementById("search").addEventListener("input", onSearch);
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
    const res = await fetch("api/vessels", { cache: "no-store" });
    if (!res.ok) throw new Error(res.status);
    const data = await res.json();
    lastData = data;
    setStale(false);
    render(data);
  } catch (e) {
    setStale(true);
  }
}

function setStale(stale) {
  document.getElementById("refresh-dot").classList.toggle("stale", stale);
}

function render(data) {
  if (data.station && data.station.label) {
    document.querySelectorAll("[data-title]").forEach((el) => {
      el.textContent = data.station.label;
    });
    document.title = data.station.label + " — Map";
  }

  // Station (receiver) marker.
  if (data.station && data.station.lat != null && data.station.lon != null) {
    const pos = [data.station.lon, data.station.lat];
    if (!stationMarker) {
      stationMarker = new maplibregl.Marker({ element: stationElement() })
        .setLngLat(pos)
        .setPopup(new maplibregl.Popup({ offset: 14 }).setHTML(
          "Receiver: " + esc(data.station.label || "station")
        ))
        .addTo(map);
    } else {
      stationMarker.setLngLat(pos);
    }
  }

  // Rebuild vessel markers.
  for (const m of markers.values()) m.remove();
  markers.clear();
  for (const v of data.vessels) {
    const heading = v.heading != null ? v.heading : v.cog;
    const m = new maplibregl.Marker({
      element: vesselElement(v),
      rotation: heading != null ? heading : 0,
      rotationAlignment: "map",
    })
      .setLngLat([v.lon, v.lat])
      .setPopup(new maplibregl.Popup({ offset: 12 }).setHTML(popupHtml(v)))
      .addTo(map);
    markers.set(v.mmsi, m);
  }

  // On the first load, move to where the data actually is.
  if (!fitted) {
    fitInitialView(data);
    fitted = true;
  }
  updateCount();
}

function fitInitialView(data) {
  if (markers.size >= 2) {
    const b = new maplibregl.LngLatBounds();
    for (const m of markers.values()) b.extend(m.getLngLat());
    map.fitBounds(b, { padding: 60, maxZoom: 13, animate: false });
  } else if (markers.size === 1) {
    map.jumpTo({ center: markers.values().next().value.getLngLat(), zoom: 11 });
  } else if (data.station && data.station.lat != null && data.station.lon != null) {
    map.jumpTo({ center: [data.station.lon, data.station.lat], zoom: DEFAULT_ZOOM });
  }
}

function updateCount() {
  if (!map || !lastData) return;
  const bounds = map.getBounds();
  let visible = 0;
  for (const m of markers.values()) {
    if (bounds.contains(m.getLngLat())) visible += 1;
  }
  document.getElementById("count").textContent = visible + " / " + markers.size;
}

// ---- Vessel / station marker elements -------------------------------------

// AIS ship-type category (tens digit) -> colour, MarineTraffic-ish.
function typeColor(t) {
  if (t >= 60 && t <= 69) return "#2f6bff"; // passenger - blue
  if (t >= 70 && t <= 79) return "#2fa84f"; // cargo - green
  if (t >= 80 && t <= 89) return "#d64545"; // tanker - red
  if (t >= 40 && t <= 49) return "#00b4b4"; // high-speed - teal
  if (t >= 50 && t <= 59) return "#00a0d6"; // tug/pilot/special - cyan
  if (t === 30) return "#b5651d"; // fishing - brown
  if (t === 36 || t === 37) return "#b048c8"; // sailing/pleasure - purple
  return "#8894a3"; // unknown/other - grey
}

function markerElement(svg, size) {
  const el = document.createElement("div");
  el.className = "vessel-icon";
  el.style.width = size + "px";
  el.style.height = size + "px";
  el.innerHTML = svg;
  return el;
}

function vesselElement(v) {
  const color = v.own ? "#111827" : typeColor(v.ship_type);
  const stroke = v.own ? "#f5b301" : "#0b2338";
  const heading = v.heading != null ? v.heading : v.cog;
  let shape;
  if (heading != null) {
    // Points north; the marker's own rotation applies the heading.
    shape =
      '<polygon points="0,-8 5,7 0,4 -5,7" fill="' +
      color +
      '" stroke="' +
      stroke +
      '" stroke-width="1"/>';
  } else {
    shape =
      '<circle r="3.4" fill="' + color + '" stroke="' + stroke + '" stroke-width="1"/>';
  }
  return markerElement(
    '<svg width="20" height="20" viewBox="-10 -10 20 20">' + shape + "</svg>",
    20
  );
}

function stationElement() {
  const svg =
    '<svg width="26" height="26" viewBox="0 0 26 26">' +
    '<path d="M13 4 L18 22 L13 19 L8 22 Z" fill="#2f6bff" stroke="#0b2338" stroke-width="1"/>' +
    '<path d="M7 8 A8 8 0 0 1 19 8" fill="none" stroke="#2fa84f" stroke-width="1.6"/>' +
    '<path d="M7 8 A8 8 0 0 0 19 8" fill="none" stroke="#d64545" stroke-width="1.6"/>' +
    "</svg>";
  return markerElement(svg, 26);
}

function popupHtml(v) {
  const rows = [];
  rows.push("<b>" + esc(v.name || "(unknown)") + "</b>");
  rows.push("MMSI: " + v.mmsi + (v.class ? " &middot; Class " + v.class : ""));
  if (v.callsign) rows.push("Callsign: " + esc(v.callsign));
  if (v.ship_type_text) rows.push("Type: " + esc(v.ship_type_text));
  if (v.sog != null) rows.push("Speed: " + v.sog.toFixed(1) + " kn");
  const hdg = v.heading != null ? v.heading : v.cog;
  if (hdg != null) rows.push("Course: " + Math.round(hdg) + "&deg;");
  if (v.nav_status) rows.push("Status: " + esc(v.nav_status));
  rows.push('<span style="color:#8a97a6">seen ' + v.age + "s ago</span>");
  return rows.join("<br>");
}

// ---- Search ---------------------------------------------------------------

function onSearch(e) {
  const q = e.target.value.trim().toLowerCase();
  if (!q || !lastData) return;
  const hit = lastData.vessels.find(
    (v) =>
      String(v.mmsi).includes(q) ||
      (v.name && v.name.toLowerCase().includes(q))
  );
  if (hit) {
    map.flyTo({ center: [hit.lon, hit.lat], zoom: Math.max(map.getZoom(), 12) });
    const m = markers.get(hit.mmsi);
    if (m && !m.getPopup().isOpen()) m.togglePopup();
  }
}

// ---- Cursor readout -------------------------------------------------------

function updateCursor(lngLat) {
  document.getElementById("cursor").textContent =
    dms(lngLat.lat, true) +
    "  " +
    dms(lngLat.lng, false) +
    "  (" +
    lngLat.lat.toFixed(4) +
    ", " +
    lngLat.lng.toFixed(4) +
    ")";
}

function dms(v, isLat) {
  const hemi = isLat ? (v >= 0 ? "N" : "S") : v >= 0 ? "E" : "W";
  v = Math.abs(v);
  const d = Math.floor(v);
  const mfloat = (v - d) * 60;
  const m = Math.floor(mfloat);
  const s = ((mfloat - m) * 60).toFixed(1);
  return (
    String(d) +
    "° " +
    String(m).padStart(2, "0") +
    "' " +
    String(s).padStart(4, "0") +
    '" ' +
    hemi
  );
}

function esc(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

window.addEventListener("DOMContentLoaded", init);
