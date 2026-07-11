# Deploying the ais-forwarder web UI

The `ais-forwarder` binary embeds a small read-only web server (Map + Status
pages). It is meant to sit behind an nginx reverse proxy. This directory holds
the nginx config for the public deployment at **https://canbo.at/ais-harlingen/**.

## Topology

```
browser ──TLS──> nginx (canbo.at, shared host) ──tunnel──> ais-forwarder :8080
                   │                                          (0.0.0.0:8080)
                   ├─ microcache /api/*  (5s)
                   └─ cache static shell (1h)
```

- The app binds `0.0.0.0:8080` (set in `config.ini`); the reverse proxy is on a
  different host and reaches it over your tunnel/VPN.
- The app is **path-agnostic**: nginx strips the `/ais-harlingen/` prefix, so the
  app serves `/`, `/api/vessels`, `/api/stats`. The HTML uses
  `<base href="/ais-harlingen/">` so assets resolve under the subpath. Remount it
  elsewhere by changing only `nginx.conf`.

## nginx setup

`nginx.conf` is split into two parts (see the comments in the file):

1. **PART A** → `/etc/nginx/conf.d/ais-harlingen.conf` — rate/conn zones, the
   cache path, and the `ais_backend` upstream. All names are `ais_`-prefixed so
   they don't collide with the other domains on the host.
2. **PART B** → paste the `location` blocks into the existing
   `server { server_name canbo.at; ... }`.

Then:

```sh
# create the cache dir (owner = nginx user)
install -d -o www-data -g www-data /var/cache/nginx/ais-harlingen
# point the upstream at the app host
$EDITOR /etc/nginx/conf.d/ais-harlingen.conf   # server 10.0.67.7:8080;
nginx -t && systemctl reload nginx
```

## Why it resists DoS

The dashboard is read-only and every client polls the *same* data, so the
primary defense is the **microcache**: `/api/*` is cached 5s with
`proxy_cache_lock`, so N concurrent clients collapse to ~1 upstream request per
interval — the backend load is independent of client count. Rate limits
(`5r/s` API, `30r/s` static), a per-IP connection cap (20), a 1k body limit and
short timeouts protect nginx itself against slow/abusive clients. The map only
refreshes every 15s (with an on-page countdown) so well-behaved clients are
gentle to begin with.

nginx can't stop a volumetric L3/L4 flood that saturates the uplink — that needs
an upstream scrubber. Consider `fail2ban` on the access log to ban repeat-429
IPs, and `net.ipv4.tcp_syncookies=1`.
