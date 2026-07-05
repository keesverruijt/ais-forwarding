<link href="https://cdn.jsdelivr.net/npm/maplibre-gl@5.8.0/dist/maplibre-gl.css" rel="stylesheet" />
<script src="https://cdn.jsdelivr.net/npm/maplibre-gl@5.8.0/dist/maplibre-gl.js"></script>
<style>
  /* Map needs an explicit height when embedded in a page; the
     fullscreen button (top-right) expands it on demand. */
  #map { width: 100%; height: 500px; background: #0a0a14; }

  /* Legend */
  .map-legend {
    background: rgba(255, 255, 255, 0.92);
    backdrop-filter: blur(4px);
    -webkit-backdrop-filter: blur(4px);
    padding: 8px 10px;
    border-radius: 6px;
    box-shadow: 0 1px 5px rgba(0, 0, 0, 0.25);
    font: 12px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    max-width: 280px;
  }
  .legenda-row {
    display: flex; align-items: center; gap: 6px;
    padding: 2px 4px; border-radius: 3px; cursor: pointer;
    -webkit-user-select: none; user-select: none; transition: background 0.15s;
  }
  .legenda-row:hover { background: rgba(0, 0, 0, 0.06); }
  .legenda-row.active { background: rgba(0, 100, 200, 0.1); }
  .legenda-swatch { width: 28px; height: 5px; border-radius: 3px; flex-shrink: 0; }
  .legenda-year { font-weight: 600; min-width: 32px; }
  .legenda-stat { color: #666; white-space: nowrap; }
  .legenda-hint { color: #999; font-size: 10px; padding: 2px 4px 0; }
  #speed-scale { display: none; padding: 4px; margin-top: 4px; background: rgba(0, 0, 0, 0.03); border-radius: 4px; }
  #speed-scale .title { font-size: 10px; color: #666; margin-bottom: 2px; }
  #speed-scale .bar { display: flex; height: 8px; border-radius: 4px; overflow: hidden; }
  #speed-scale .labels { display: flex; font-size: 9px; color: #888; margin-top: 1px; }
  #speed-scale .labels > span { text-align: center; }

  /* Top bar (title + latest-fix info) */
  .map-topbar {
    background: rgba(255, 255, 255, 0.92);
    backdrop-filter: blur(4px); -webkit-backdrop-filter: blur(4px);
    border-radius: 6px; box-shadow: 0 1px 5px rgba(0, 0, 0, 0.25);
    display: flex; flex-direction: column; gap: 1px; padding: 3px 4px 4px 8px;
    min-width: 170px; max-width: 240px;
  }
  .map-topbar-title { display: flex; align-items: center; gap: 2px; }
  .map-title { flex: 1; font: 12px/1 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; color: #333; white-space: nowrap; padding-right: 4px; }
  .map-info { font: 11px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; color: #555; padding: 1px 2px 0; }
  .map-info .age { font-weight: 600; color: #333; }
  .map-info .pos { font-variant-numeric: tabular-nums; }
  .map-btn { background: transparent; border: none; border-radius: 3px; width: 26px; height: 26px; cursor: pointer; display: flex; align-items: center; justify-content: center; font-size: 16px; line-height: 1; color: #333; padding: 0; }
  .map-btn:hover { background: rgba(0, 0, 0, 0.08); }

  .wind-barb { line-height: 0; pointer-events: none; }
</style>
<div id="map"></div>
<script>
  const mmsi = "<?php echo $mmsi;?>";
  const shipName = "<?php echo addslashes($name); ?>";
  const base_url = "https://merrimac.nl/wp-content/uploads/map/";


  // Barbs are drawn at every zoom; per-pixel thinning (below) keeps their
  // on-screen density constant, so there's no need for a zoom threshold.
  const WIND_MIN_PX = 50; // minimum on-screen spacing between wind barbs (px)
  const YEAR_RX = /_([^_]*)_/;

  const map = new maplibregl.Map({
    container: "map",
    center: [-90, 15],
    zoom: 1.5,
    style: {
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
          attribution:
            'Map data &copy; <a href="https://www.osm.org">OpenStreetMap</a>',
        },
      },
      layers: [{ id: "osm", type: "raster", source: "osm" }],
    },
  });

  map.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), "top-right");
  map.addControl(new maplibregl.FullscreenControl(), "top-right");
  map.addControl(new maplibregl.ScaleControl({ unit: "nautical" }), "bottom-right");

  // --- Top bar control (title + refresh + latest-fix info) ---
  let infoDiv;
  class TopBarControl {
    onAdd(m) {
      this._map = m;
      const bar = document.createElement("div");
      bar.className = "map-topbar maplibregl-ctrl";

      const titleRow = document.createElement("div");
      titleRow.className = "map-topbar-title";
      const title = document.createElement("span");
      title.className = "map-title";
      title.textContent = shipName + " / " + mmsi;
      const refreshBtn = document.createElement("button");
      refreshBtn.className = "map-btn";
      refreshBtn.type = "button";
      refreshBtn.title = "Refresh";
      refreshBtn.innerHTML = "↻";
      refreshBtn.addEventListener("click", () => window.location.reload());
      titleRow.appendChild(title);
      titleRow.appendChild(refreshBtn);

      infoDiv = document.createElement("div");
      infoDiv.className = "map-info";
      infoDiv.style.display = "none";

      bar.appendChild(titleRow);
      bar.appendChild(infoDiv);
      this._container = bar;
      return bar;
    }
    onRemove() { this._container.parentNode.removeChild(this._container); }
  }
  map.addControl(new TopBarControl(), "top-left");
  // Keep the "updated … ago" line ticking without a page reload.
  setInterval(updateLatestInfo, 30000);

  // --- Latest-fix info (newest track point across all loaded years) --------
  function latestFix() {
    let best = null;
    for (const y of Object.keys(yearState)) {
      for (const p of yearState[y].track) {
        if (!p.time) continue;
        if (!best || p.time > best.time) best = p;
      }
    }
    return best;
  }

  function formatAge(ms) {
    const s = Math.max(0, Math.round(ms / 1000));
    if (s < 60) return s + " s ago";
    const m = Math.round(s / 60);
    if (m < 60) return m + " min ago";
    const h = Math.floor(m / 60);
    if (h < 24) return h + " h " + (m % 60) + " min ago";
    const d = Math.floor(h / 24);
    return d + " d " + (h % 24) + " h ago";
  }

  function formatCoord(v, pos, neg) {
    const hemi = v >= 0 ? pos : neg;
    v = Math.abs(v);
    const deg = Math.floor(v);
    const min = ((v - deg) * 60).toFixed(1);
    return deg + "°" + min + "′ " + hemi;
  }

  function updateLatestInfo() {
    if (!infoDiv) return;
    const p = latestFix();
    if (!p || !p.time) { infoDiv.style.display = "none"; return; }
    infoDiv.style.display = "block";

    const lon = ((((p.lon + 180) % 360) + 360) % 360) - 180; // normalise unwrapped lon to [-180,180)
    let html = '<div class="age">Updated ' + formatAge(Date.now() - p.time.getTime()) + "</div>";
    html += '<div class="pos">' + formatCoord(p.lat, "N", "S") + ", " + formatCoord(lon, "E", "W") + "</div>";

    const parts = [];
    if (p.sog != null) parts.push(p.sog.toFixed(1) + " kn");
    if (p.tws != null) {
      let w = "wind " + p.tws.toFixed(0) + " kn";
      if (p.twd != null) w += " @ " + Math.round(((p.twd || 0) + (p.cog || 0)) % 360) + "°";
      parts.push(w);
    }
    if (parts.length) html += "<div>" + parts.join(" · ") + "</div>";
    infoDiv.innerHTML = html;
  }

  // --- Legend control ---
  let legendDiv;
  class LegendControl {
    onAdd() {
      legendDiv = document.createElement("div");
      legendDiv.className = "map-legend maplibregl-ctrl";
      this._container = legendDiv;
      return legendDiv;
    }
    onRemove() { this._container.parentNode.removeChild(this._container); }
  }
  map.addControl(new LegendControl(), "bottom-left");

  const yearState = {}; // year -> { color, file, track, markers }
  const popup = new maplibregl.Popup({ closeButton: false, closeOnClick: false });

  map.on("load", () => {
    map.setProjection({ type: "globe" }); // 3D globe when zoomed out, flat when zoomed in
    // Graticule (curves naturally on the globe)
    map.addSource("graticule", { type: "geojson", data: emptyFC() });
    map.addLayer({
      id: "graticule",
      type: "line",
      source: "graticule",
      paint: { "line-color": "rgba(120,120,120,0.5)", "line-width": 0.7 },
    });
    updateGraticule();
    map.on("moveend", updateGraticule);
    map.on("moveend", updateDetailLayers);

    // Hover tooltip: the speed line is a single gradient feature with no
    // per-point data, so when the cursor is over it we look up the nearest
    // actual track fix (which still carries sog/tws/time) and show that.
    map.on("mousemove", (e) => {
      const years = Object.keys(yearState).filter(
        (y) => map.getLayer("speed-" + y) && map.getLayoutProperty("speed-" + y, "visibility") === "visible"
      );
      if (!years.length) { hideTip(); return; }
      // Cheap gate: only search when actually hovering the rendered line.
      if (!map.queryRenderedFeatures(e.point, { layers: years.map((y) => "speed-" + y) }).length) {
        hideTip();
        return;
      }
      const p = nearestFix(years, e.point);
      if (!p) { hideTip(); return; }

      let tip = p.sog != null ? p.sog.toFixed(1) + " kn" : "";
      if (p.tws != null) tip += " | wind " + p.tws.toFixed(0) + " kn";
      if (p.time) tip += " | " + p.time.toISOString().substring(0, 16);
      if (!tip) { hideTip(); return; }

      map.getCanvas().style.cursor = "crosshair";
      popup.setLngLat([p.lon, p.lat]).setHTML(tip).addTo(map);
    });

    // Load the year list
    fetch(base_url + mmsi + "_tracks.json")
      .then((r) => r.json())
      .then(processFiles)
      .catch((err) => console.error("Error fetching JSON:", err));
  });

  function emptyFC() { return { type: "FeatureCollection", features: [] }; }

  function hideTip() {
    popup.remove();
    map.getCanvas().style.cursor = "";
  }

  // Nearest track fix (in screen pixels) among the given years, limited to the
  // current viewport so we never scan the whole world's worth of points.
  function nearestFix(years, pixel) {
    let best = null, bestD = Infinity;
    // Pixel-space search (see refreshWindBarbs): geographic bounds.contains()
    // would drop antimeridian-unwrapped points whose lon falls outside
    // [-180,180], so the hover popup died exactly where the boat now is.
    const cont = map.getContainer();
    const vw = cont.clientWidth, vh = cont.clientHeight;
    for (const y of years) {
      for (const p of yearState[y].track) {
        const pt = map.project([p.lon, p.lat]);
        if (pt.x < 0 || pt.x > vw || pt.y < 0 || pt.y > vh) continue;
        if (isOccluded(pt, p)) continue;
        const d = (pt.x - pixel.x) ** 2 + (pt.y - pixel.y) ** 2;
        if (d < bestD) { bestD = d; best = p; }
      }
    }
    return best;
  }

  // True when a track point is hidden behind the globe: project() still returns
  // an on-screen pixel for far-side points, but unproject() of that pixel gives
  // the NEAR-side location, so a point that doesn't round-trip is occluded.
  // (On the flat/Mercator projection every visible point round-trips, so this
  // simply never fires there.)
  function isOccluded(pt, p) {
    const ll = map.unproject(pt);
    if (Math.abs(ll.lat - p.lat) > 1) return true;
    const dlon = Math.abs(((((ll.lng - p.lon) % 360) + 540) % 360) - 180);
    return dlon > 1;
  }

  function processFiles(data) {
    const start_hue = 360, end_hue = 90;
    const step_hue = (start_hue - end_hue) / data.length;
    let hue = start_hue, last_file;

    for (const file of data) {
      const color = RgbToHex(HslToRgb(hue / 360, 1, 0.6));
      const year = YEAR_RX.exec(file)[1];

      const row = document.createElement("div");
      row.className = "legenda-row";
      row.id = "legenda_" + year;
      row.dataset.file = file;
      row.dataset.color = color;

      const swatch = document.createElement("div");
      swatch.className = "legenda-swatch";
      swatch.style.background = color;

      const yearEl = document.createElement("span");
      yearEl.className = "legenda-year";
      yearEl.textContent = year;

      const stats = document.createElement("span");
      stats.className = "legenda-stat";
      stats.id = "stats_" + year;

      row.appendChild(swatch);
      row.appendChild(yearEl);
      row.appendChild(stats);
      row.addEventListener("click", () => loadMapForYear(row.dataset.file));
      legendDiv.appendChild(row);

      last_file = file;
      hue -= step_hue;
    }

    const scale = document.createElement("div");
    scale.id = "speed-scale";
    scale.innerHTML =
      '<div class="title">Speed (knots)</div>' +
      '<div class="bar">' +
      `<div style="flex:5;background:${speedColor(2)}"></div>` +
      `<div style="flex:2;background:${speedColor(6)}"></div>` +
      `<div style="flex:2;background:${speedColor(8)}"></div>` +
      `<div style="flex:2;background:${speedColor(10)}"></div>` +
      "</div>" +
      '<div class="labels">' +
      '<span style="flex:5">0</span><span style="flex:2">5</span>' +
      '<span style="flex:2">7</span><span style="flex:2">9+</span>' +
      "</div>";
    legendDiv.appendChild(scale);

    if (data.length > 1) {
      const hint = document.createElement("div");
      hint.className = "legenda-hint";
      hint.textContent = "Click a year to show/hide its track.";
      legendDiv.appendChild(hint);
    }

    loadMapForYear(last_file);
  }

  function loadMapForYear(file) {
    const year = YEAR_RX.exec(file)[1];
    const row = document.getElementById("legenda_" + year);

    if (yearState[year]) {
      removeYear(year);
      row.classList.remove("active");
      updateDetailLayers();
      return;
    }

    const color = row.dataset.color;
    fetch(base_url + file)
      .then((res) => res.text())
      .then((gpxText) => {
        const track = parseGPX(gpxText);
        if (track.length === 0) return;

        // Basic track: one LineString. lineMetrics:true lets the speed layer
        // tint it by line-progress.
        map.addSource("basic-src-" + year, {
          type: "geojson",
          lineMetrics: true,
          data: {
            type: "Feature",
            geometry: { type: "LineString", coordinates: track.map((p) => [p.lon, p.lat]) },
          },
        });
        map.addLayer({
          id: "basic-" + year,
          type: "line",
          source: "basic-src-" + year,
          layout: { "line-join": "round", "line-cap": "round" },
          paint: { "line-color": color, "line-width": 3, "line-opacity": 0.8 },
        });

        // Speed-coloured track: the SAME line tinted by speed via line-gradient.
        // One feature, so it stays crisp at every zoom — unlike thousands of
        // per-segment features, which collapse below tile resolution zoomed out.
        map.addLayer({
          id: "speed-" + year,
          type: "line",
          source: "basic-src-" + year,
          layout: { visibility: "none", "line-join": "round", "line-cap": "round" },
          paint: { "line-gradient": speedGradient(track), "line-width": 4, "line-opacity": 0.9 },
        });

        yearState[year] = { color, file, track, markers: [] };
        row.classList.add("active");

        const statsEl = document.getElementById("stats_" + year);
        if (!statsEl.textContent) {
          statsEl.textContent =
            totalDistance(track).toFixed(0) + " nm, max " + maxSpeed(track).toFixed(1) + " kn";
        }

        const b = new maplibregl.LngLatBounds();
        track.forEach((p) => b.extend([p.lon, p.lat]));
        map.fitBounds(b, { padding: 60, duration: 800 });

        updateDetailLayers();
      });
  }

  function removeYear(year) {
    const st = yearState[year];
    if (!st) return;
    clearMarkers(st);
    for (const id of ["speed-" + year, "basic-" + year]) {
      if (map.getLayer(id)) map.removeLayer(id);
    }
    if (map.getSource("basic-src-" + year)) map.removeSource("basic-src-" + year);
    delete yearState[year];
  }

  function clearMarkers(st) {
    st.markers.forEach((m) => m.remove());
    st.markers = [];
  }

  function updateDetailLayers() {
    const years = Object.keys(yearState);
    const singleYear = years.length === 1;

    for (const year of years) {
      const st = yearState[year];
      const row = document.getElementById("legenda_" + year);
      const swatch = row.querySelector(".legenda-swatch");

      if (singleYear) {
        map.setLayoutProperty("speed-" + year, "visibility", "visible");
        map.setPaintProperty("basic-" + year, "line-opacity", 0.2);
        swatch.style.background = `linear-gradient(to right, ${speedColor(2)} 0%, ${speedColor(2)} 45%, ${speedColor(6)} 45%, ${speedColor(6)} 63%, ${speedColor(8)} 63%, ${speedColor(8)} 82%, ${speedColor(10)} 82%)`;
      } else {
        map.setLayoutProperty("speed-" + year, "visibility", "none");
        map.setPaintProperty("basic-" + year, "line-opacity", 0.8);
        swatch.style.background = st.color;
      }

      refreshWindBarbs(year);
    }

    document.querySelectorAll(".legenda-row:not(.active)").forEach((row) => {
      const swatch = row.querySelector(".legenda-swatch");
      if (swatch) swatch.style.background = row.dataset.color;
    });

    const scaleEl = document.getElementById("speed-scale");
    if (scaleEl) scaleEl.style.display = singleYear ? "block" : "none";

    updateLatestInfo();
  }

  function refreshWindBarbs(year) {
    const st = yearState[year];
    clearMarkers(st);

    // Cull by on-screen pixel position rather than geographic bounds. The track
    // is longitude-unwrapped across the antimeridian, so its lon values can fall
    // outside [-180,180] (the boat's current fix sits at lon ~-180.8). MapLibre's
    // LngLatBounds.contains() rejects those, which made every barb near the date
    // line vanish. project() resolves each point to its true on-globe pixel
    // regardless of wrapping, so a viewport test is antimeridian-safe.
    const cont = map.getContainer();
    const vw = cont.clientWidth, vh = cont.clientHeight;
    const MARGIN = 40; // keep barbs that sit just off the visible edges
    let lastPt = null;

    for (const p of st.track) {
      if (p.twd == null || p.tws == null) continue;

      const pt = map.project([p.lon, p.lat]);
      if (pt.x < -MARGIN || pt.x > vw + MARGIN || pt.y < -MARGIN || pt.y > vh + MARGIN) continue;
      // On the globe, points on the far side still project into the viewport.
      // Reject them: a pixel that doesn't round-trip back to this location is
      // occluded (unproject returns the near-side point under that pixel).
      if (isOccluded(pt, p)) continue;

      // Thin to a constant on-screen spacing.
      if (lastPt && Math.hypot(pt.x - lastPt.x, pt.y - lastPt.y) < WIND_MIN_PX) continue;
      lastPt = pt;

      // TWD is relative to boat heading; add COG for true north on the map
      const rotation = ((p.twd || 0) + (p.cog || 0)) % 360;

      const el = document.createElement("div");
      el.className = "wind-barb";
      el.innerHTML = windBarbSVG(p.tws, rotation);

      const marker = new maplibregl.Marker({ element: el, anchor: "bottom" })
        .setLngLat([p.lon, p.lat])
        .addTo(map);
      st.markers.push(marker);
    }
  }

  // --- Graticule -----------------------------------------------------------
  function gridSpacing(z) {
    if (z < 2) return 30;
    if (z < 4) return 10;
    if (z < 6) return 5;
    if (z < 8) return 1;
    if (z < 10) return 0.5;
    if (z < 12) return 0.25;
    return 0.1;
  }

  function updateGraticule() {
    const src = map.getSource("graticule");
    if (src) src.setData(buildGraticule());
  }

  function buildGraticule() {
    let b;
    try { b = map.getBounds(); } catch (e) { return emptyFC(); }
    const z = map.getZoom();
    const s = gridSpacing(z);
    let west = b.getWest(), east = b.getEast();
    let south = Math.max(-85, b.getSouth()), north = Math.min(85, b.getNorth());
    if (east < west) east += 360;
    if (east - west > 360) { west = -180; east = 180; }
    if (north <= south) return emptyFC();

    const lonStep = Math.min(s, (east - west) / 120) || s;
    const latStep = Math.min(s, (north - south) / 120) || s;
    const features = [];

    for (let lat = Math.ceil(south / s) * s; lat <= north; lat += s) {
      const coords = [];
      for (let lon = west; lon < east; lon += lonStep) coords.push([lon, lat]);
      coords.push([east, lat]);
      features.push({ type: "Feature", geometry: { type: "LineString", coordinates: coords } });
    }
    for (let lon = Math.ceil(west / s) * s; lon <= east; lon += s) {
      const coords = [];
      for (let lat = south; lat < north; lat += latStep) coords.push([lon, lat]);
      coords.push([lon, north]);
      features.push({ type: "Feature", geometry: { type: "LineString", coordinates: coords } });
    }
    return { type: "FeatureCollection", features };
  }

  // --- Data layer (ported verbatim from the Leaflet snippet) ---------------
  function speedColor(knots) {
    if (knots == null) return "#888";
    if (knots < 5) return `hsl(220, 80%, ${55 - knots}%)`;
    if (knots < 7) return "hsl(120, 80%, 40%)";
    if (knots < 9) return "hsl(50, 100%, 45%)";
    return "hsl(0, 100%, 45%)";
  }

  // Build a line-gradient paint expression that tints a single track line by
  // speed. Stops are positioned by cumulative Web-Mercator distance so they
  // align with MapLibre's line-progress (0..1). Downsampled to ~400 stops.
  function speedGradient(track) {
    function merc(lon, lat) {
      const s = Math.sin((lat * Math.PI) / 180);
      return [(lon + 180) / 360, 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)];
    }
    const n = track.length;
    const cum = new Array(n);
    cum[0] = 0;
    for (let k = 1; k < n; k++) {
      const a = merc(track[k - 1].lon, track[k - 1].lat);
      const b = merc(track[k].lon, track[k].lat);
      cum[k] = cum[k - 1] + Math.hypot(b[0] - a[0], b[1] - a[1]);
    }
    const total = cum[n - 1] || 1;
    const stride = Math.max(1, Math.floor(n / 400));
    const stops = [];
    let last = -1;
    for (let k = 0; k < n; k += stride) {
      let prog = k === 0 ? 0 : cum[k] / total;
      if (prog <= last) prog = last + 1e-6;
      if (prog >= 1) break;
      const sog = track[k].sog != null ? track[k].sog : k > 0 ? track[k - 1].sog : null;
      stops.push(prog, speedColor(sog));
      last = prog;
    }
    stops.push(1, speedColor(track[n - 1].sog != null ? track[n - 1].sog : null));
    return ["interpolate", ["linear"], ["line-progress"], ...stops];
  }

  function parseGPX(gpxText) {
    const parser = new DOMParser();
    const doc = parser.parseFromString(gpxText, "text/xml");
    const trkpts = doc.querySelectorAll("trkpt");
    const points = [];

    for (const pt of trkpts) {
      const lat = parseFloat(pt.getAttribute("lat"));
      const lon = parseFloat(pt.getAttribute("lon"));
      const timeEl = pt.querySelector("time");
      const time = timeEl ? new Date(timeEl.textContent) : null;

      let sog = null, cog = null, tws = null, twd = null;
      const ext = pt.querySelector("extensions");
      if (ext) {
        for (const child of ext.children) {
          const tag = child.localName;
          const val = parseFloat(child.textContent);
          if (tag === "sog") sog = val;
          else if (tag === "cog") cog = val;
          else if (tag === "tws") tws = val;
          else if (tag === "twd") twd = val;
        }
      }
      points.push({ lat, lon, time, sog, cog, tws, twd });
    }

    unwrapLongitudes(points);
    return points;
  }

  // Keep longitudes continuous across the antimeridian so a circumnavigating
  // track stays one unbroken line. (Less critical on a globe than in Mercator,
  // but harmless and keeps geometry well-behaved.)
  function unwrapLongitudes(points) {
    if (points.length === 0) return;
    let prev = points[0].lon;
    for (let i = 1; i < points.length; i++) {
      let lon = points[i].lon;
      while (lon - prev > 180) lon -= 360;
      while (lon - prev < -180) lon += 360;
      points[i].lon = lon;
      prev = lon;
    }
  }

  function windBarbSVG(knots, rotation) {
    const W = 20, H = 30;
    const cx = W / 2;
    const staffTop = 3, staffBot = H;
    const barbLen = 7, halfLen = 4, barbGap = 3;

    let paths = [];
    paths.push(`M${cx},${staffTop} L${cx},${staffBot}`);
    let remaining = Math.round(knots / 5) * 5;
    let y = staffTop;

    while (remaining >= 50) {
      paths.push(`M${cx},${y} L${cx + barbLen},${y + barbGap / 2} L${cx},${y + barbGap} Z`);
      y += barbGap + 1; remaining -= 50;
    }
    while (remaining >= 10) {
      paths.push(`M${cx},${y} L${cx + barbLen},${y - 2}`);
      y += barbGap; remaining -= 10;
    }
    if (remaining >= 5) paths.push(`M${cx},${y} L${cx + halfLen},${y - 1}`);

    return (
      `<svg xmlns="https://www.w3.org/2000/svg" width="${W}" height="${H}" ` +
      `style="transform:rotate(${rotation}deg);transform-origin:center bottom">` +
      `<path d="${paths.join(" ")}" stroke="#0055aa" stroke-width="1.5" ` +
      `fill="#0055aa" stroke-linecap="round"/></svg>`
    );
  }

  function haversineNm(lat1, lon1, lat2, lon2) {
    const R = 3440.065;
    const dLat = ((lat2 - lat1) * Math.PI) / 180;
    const dLon = ((lon2 - lon1) * Math.PI) / 180;
    const a =
      Math.sin(dLat / 2) ** 2 +
      Math.cos((lat1 * Math.PI) / 180) * Math.cos((lat2 * Math.PI) / 180) * Math.sin(dLon / 2) ** 2;
    return 2 * R * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
  }

  function totalDistance(trackData) {
    let total = 0;
    for (let i = 1; i < trackData.length; i++) {
      total += haversineNm(trackData[i - 1].lat, trackData[i - 1].lon, trackData[i].lat, trackData[i].lon);
    }
    return total;
  }

  function maxSpeed(trackData) {
    let max = 0;
    for (const p of trackData) if (p.sog != null && p.sog > max) max = p.sog;
    return max;
  }

  function HslToRgb(h, s, l) {
    var r, g, b;
    if (s == 0) { r = g = b = l; }
    else {
      function hue2rgb(p, q, t) {
        if (t < 0) t += 1;
        if (t > 1) t -= 1;
        if (t < 1 / 6) return p + (q - p) * 6 * t;
        if (t < 1 / 2) return q;
        if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
        return p;
      }
      var q = l < 0.5 ? l * (1 + s) : l + s - l * s;
      var p = 2 * l - q;
      r = hue2rgb(p, q, h + 1 / 3);
      g = hue2rgb(p, q, h);
      b = hue2rgb(p, q, h - 1 / 3);
    }
    return [Math.trunc(r * 255), Math.trunc(g * 255), Math.trunc(b * 255)];
  }

  function RgbToHex(rgb) {
    return "#" + ((1 << 24) + (rgb[0] << 16) + (rgb[1] << 8) + rgb[2]).toString(16).slice(1);
  }
</script>
