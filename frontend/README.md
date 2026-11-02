# DPDS frontend

React + TypeScript + Vite, react-bootstrap, Chart.js; look and layout follow the P4TG GUI. It talks to the
`dpds-controller` REST API under `/api` (pages: Dashboard, Links, Interfaces, Host hints, Settings).

## Development

```sh
npm ci
npm run dev        # http://localhost:5173, /api is proxied to http://127.0.0.1:8100 (vite.config.ts)
npm test           # vitest against a mocked API
npm run typecheck
```

Start a controller first (`dpds-controller serve`, default bind `0.0.0.0:8100`). Node >= 18.18 works; the
Docker build uses Node 22.

## Build

```sh
npm run build      # tsc --noEmit && vite build -> dist/
```

## Docker

`frontend/Dockerfile` builds `dist/` with `node:22.23.4-alpine` and serves it with
`nginxinc/nginx-unprivileged:1.30.6-alpine` (nginx runs as uid 101, not root). `frontend/nginx.conf` is installed as an
nginx template that the image renders with `envsubst` at start. It falls back to `index.html`, serves `index.html` with
`Cache-Control: no-cache` and `/assets/` (hashed file names) with a one-year cache, gzips js/css/json and proxies `/api/`
to the controller. The root `docker-compose.yml` runs it with `network_mode: host`:

```sh
docker compose up -d --build          # from the repository root
DPDS_FRONTEND_BIND=127.0.0.1 DPDS_FRONTEND_PORT=9000 DPDS_CONTROLLER=http://127.0.0.1:8100 docker compose up -d
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `DPDS_FRONTEND_BIND` | `0.0.0.0` | Address nginx listens on. The frontend port exposes the **whole controller API** (start/stop, links, settings), so use `127.0.0.1` when only this host should reach it (and bind the controller to `127.0.0.1` too). An IPv6 address needs brackets: `[::1]`. |
| `DPDS_FRONTEND_PORT` | `8080` | Port nginx listens on. Must be 1024 or higher: nginx does not run as root. |
| `DPDS_CONTROLLER` | `http://127.0.0.1:8100` | Controller base URL as `http://host:port` **without a path**: nginx forwards `/api/...` unchanged, so a path would be prepended to every request and break them. |

nginx forwards the browser's `Host` header to the controller (`proxy_set_header Host $host;`). The controller's
DNS-rebinding guard checks it, so a host name other than an IP address, `localhost` or the machine's host name must be
allowed on the controller with `--allowed-host` (`DPDS_ALLOWED_HOST`), or the API answers 421/403.

Not verified here: the image build and the nginx configuration were not run (no Docker in the development container);
the tests only check the files as text.
