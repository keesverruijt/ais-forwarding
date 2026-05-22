#!/usr/bin/env python3
"""Convert NMEA RMC (+MWV wind) file to GPX with speed and wind extensions.

Replaces gpsbabel for the ais-forwarder pipeline.

Usage: rmc2gpx.py input.rmc output.gpx
"""

import sys
import re
import xml.etree.ElementTree as ET
from datetime import datetime, date, timezone

NS = "http://www.topografix.com/GPX/1/1"
NS_EXT = "http://merrimac.nl/gpx/ext/1"


def parse_rmc(line):
    """Parse $xxRMC sentence, return dict or None."""
    m = re.match(r"^\$..RMC,", line)
    if not m:
        return None
    fields = line.split("*")[0].split(",")
    if len(fields) < 10 or fields[2] != "A":
        return None

    time_str = fields[1]
    date_str = fields[9]
    try:
        dt = datetime.strptime(date_str + time_str[:6], "%d%m%y%H%M%S").replace(
            tzinfo=timezone.utc
        )
    except ValueError:
        return None

    lat = _parse_latlon(fields[3], fields[4])
    lon = _parse_latlon(fields[5], fields[6])
    if lat is None or lon is None:
        return None

    sog = _parse_float(fields[7])
    cog = _parse_float(fields[8])

    result = {"time": dt, "lat": lat, "lon": lon, "sog": sog, "cog": cog}

    # Extra fields after standard RMC: twd, tws (appended by ais-forwarder)
    if len(fields) > 13 and fields[13]:
        result["twd"] = _parse_float(fields[13])
    if len(fields) > 14 and fields[14]:
        result["tws"] = _parse_float(fields[14])

    return result


def parse_mwv(line):
    """Parse $xxMWV true wind sentence, return dict or None."""
    m = re.match(r"^\$..MWV,", line)
    if not m:
        return None
    fields = line.split("*")[0].split(",")
    if len(fields) < 6 or fields[2] != "T" or fields[5] != "A":
        return None
    angle = _parse_float(fields[1])
    speed = _parse_float(fields[3])
    if angle is None or speed is None:
        return None
    units = fields[4]
    if units == "K":
        speed /= 1.852
    elif units == "M":
        speed *= 1.94384
    elif units != "N":
        return None
    return {"twd": angle, "tws": speed}


def _parse_latlon(value, hemisphere):
    if not value or not hemisphere:
        return None
    try:
        # Format: DDMM.MMMMM (lat) or DDDMM.MMMMM (lon)
        # Some GPS omit leading zeros, so MM.MMMMM is valid (degrees=0)
        dot = value.index(".")
        deg_end = max(0, dot - 2)
        degrees = float(value[:deg_end]) if deg_end > 0 else 0.0
        minutes = float(value[deg_end:])
        result = degrees + minutes / 60.0
        if hemisphere in ("S", "W"):
            result = -result
        return result
    except (ValueError, IndexError):
        return None


def _parse_float(s):
    if not s:
        return None
    try:
        return float(s)
    except ValueError:
        return None


def fix_stale_dates(trackpoints):
    """Fix backward date jumps caused by GPS restarts with stale dates.

    Walks the list in file order tracking the max date seen. When a record's
    date is earlier, it belongs to a "backward group". The first record after
    the group that has a date >= max supplies the correct date for the group.
    Time-of-day is preserved.
    """
    if not trackpoints:
        return

    max_date = trackpoints[0]["time"].date()
    i = 0
    while i < len(trackpoints):
        tp_date = trackpoints[i]["time"].date()
        if tp_date >= max_date:
            max_date = tp_date
            i += 1
        else:
            # Start of backward group
            group_start = i
            while i < len(trackpoints) and trackpoints[i]["time"].date() < max_date:
                i += 1
            # i is now the first non-backward entry, or end of list
            fix_date = trackpoints[i]["time"].date() if i < len(trackpoints) else max_date
            count = i - group_start
            print(
                f"Fixing {count} records with stale date "
                f"{trackpoints[group_start]['time'].date()} -> {fix_date}",
                file=sys.stderr,
            )
            for j in range(group_start, i):
                old = trackpoints[j]["time"]
                trackpoints[j]["time"] = datetime.combine(
                    fix_date, old.time(), tzinfo=timezone.utc
                )


def convert(input_path, output_path):
    ET.register_namespace("", NS)
    ET.register_namespace("ext", NS_EXT)

    trackpoints = []

    with open(input_path) as f:
        pending_rmc = None
        for raw_line in f:
            line = raw_line.strip()
            # Strip the persistence key prefix (e.g. "00000001@244123456")
            if "@" in line and "$" in line:
                line = line[line.index("$") :]

            rmc = parse_rmc(line)
            if rmc:
                if pending_rmc:
                    trackpoints.append(pending_rmc)
                pending_rmc = rmc
                continue

            mwv = parse_mwv(line)
            if mwv and pending_rmc:
                pending_rmc.update(mwv)
                continue

        if pending_rmc:
            trackpoints.append(pending_rmc)

    # Fix stale GPS dates: when the date jumps backward, use the next
    # forward date for the backward group (GPS restarts with old date).
    fix_stale_dates(trackpoints)

    # Build GPX
    gpx = ET.Element(
        "gpx",
        version="1.1",
        creator="rmc2gpx",
        xmlns=NS,
    )
    metadata = ET.SubElement(gpx, "metadata")
    ET.SubElement(metadata, "time").text = datetime.now(timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )

    if trackpoints:
        lats = [tp["lat"] for tp in trackpoints]
        lons = [tp["lon"] for tp in trackpoints]
        ET.SubElement(
            metadata,
            "bounds",
            minlat=f"{min(lats):.9f}",
            minlon=f"{min(lons):.9f}",
            maxlat=f"{max(lats):.9f}",
            maxlon=f"{max(lons):.9f}",
        )

    trk = ET.SubElement(gpx, "trk")
    trkseg = ET.SubElement(trk, "trkseg")

    for tp in trackpoints:
        trkpt = ET.SubElement(
            trkseg,
            "trkpt",
            lat=f"{tp['lat']:.9f}",
            lon=f"{tp['lon']:.9f}",
        )
        ET.SubElement(trkpt, "time").text = tp["time"].strftime("%Y-%m-%dT%H:%M:%SZ")

        has_ext = (
            tp.get("sog") is not None
            or tp.get("cog") is not None
            or tp.get("tws") is not None
        )
        if has_ext:
            extensions = ET.SubElement(trkpt, "extensions")
            if tp.get("sog") is not None:
                ET.SubElement(extensions, f"{{{NS_EXT}}}sog").text = f"{tp['sog']:.1f}"
            if tp.get("cog") is not None:
                ET.SubElement(extensions, f"{{{NS_EXT}}}cog").text = f"{tp['cog']:.1f}"
            if tp.get("tws") is not None:
                ET.SubElement(extensions, f"{{{NS_EXT}}}tws").text = f"{tp['tws']:.1f}"
            if tp.get("twd") is not None:
                ET.SubElement(extensions, f"{{{NS_EXT}}}twd").text = f"{tp['twd']:.1f}"

    ET.indent(gpx)
    tree = ET.ElementTree(gpx)
    tree.write(output_path, encoding="unicode", xml_declaration=True)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} input.rmc output.gpx", file=sys.stderr)
        sys.exit(1)
    convert(sys.argv[1], sys.argv[2])
