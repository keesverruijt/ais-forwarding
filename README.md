# ais-forwarder-rs

This is a program that scratches a personal itch; i'm using small OpenWRT devices
on friend's boats to develop https://github.com/keesverruijt/mayara and in exchange
I create a tracking page for them.

So I need a way to send location data to my server (over VPN) and it needs to
cache data in case their internet link is down.

This uses https://github.com/canboat/canboat to convert from N2K to NMEA0183, not
Signal K which is what I would recommend for users using a slightly bigger device.

The ais-forwarder takes the NMEA0183 AIS stream out of n2kd and then forwards it to 
services like MarineTraffic, and it takes the RMC message, prepends it with the
MMSI or boatname and then forwards that to my tracking page.

This can support any number of AIS and location services.

ais-forwarder-rs, as the name implies, is written in Rust.

## Input sources

The `provider` in `config.ini` can be a TCP client/server, a UDP listener, or a
serial device — so ais-forwarder can act as a drop-in replacement for the
vesselfinder/AISHub "AIS Dispatcher" reading straight off a serial AIS receiver:

```
provider = serial:///dev/ttyUSB0:38400
```

See `config.ini.demo` for all the forms.

## Built-in web UI

With a `[web]` section configured, ais-forwarder serves a small read-only web
interface with two pages, mirroring the AIS Dispatcher layout:

- **Map** — live vessel positions on a MapLibre GL map (OpenStreetMap base with
  the OpenSeaMap seamark overlay, no API key), the receiver's own antenna marker,
  a nautical + metric scale, a ship search box, a cursor lat/lon readout, and a
  15-second auto-refresh with a visible countdown.
- **Status** — input/output byte and message counters (bandwidth over a rolling
  60-second window), CRC errors, per-channel and per-AIS-message-type (1..27)
  breakdowns, and per-destination output counts.

MapLibre GL is vendored into the binary, so the page loads no third-party
scripts; only the map tiles are fetched from OpenStreetMap and OpenSeaMap.
The app is path-agnostic and is meant to run behind a reverse proxy; a hardened
nginx config (TLS, microcaching, rate/connection limits) for hosting it under a
subpath is in [`deploy/`](deploy/).


## To use

- Compile or cross-compile to your device architecture (easy in Rust).
- Deploy on target device
- Run it once, it will complain there is no ini file. 
- Copy config.ini.demo to that location and edit it to your satisfaction.
- Now it will run, and it should remain running no matter what happens to the network.
