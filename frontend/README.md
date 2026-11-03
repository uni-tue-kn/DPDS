# DPDS frontend

React + TypeScript + Vite, react-bootstrap, Chart.js; look and layout follow the P4TG GUI. It talks to the
`dpds-controller` REST API under `/api` (pages: Dashboard, Links, Interfaces, Host hints, Settings).

In production there is nothing to run here: CI builds `dist/` and uploads it as a release asset, `dpds.sh install`
downloads the asset that matches this directory and the controller embeds it and serves it on its own port at
`http://<host>:<controller port>/`.

## Development

```sh
npm ci
npm run dev        # http://localhost:5173, /api is proxied to http://127.0.0.1:8100 (vite.config.ts)
npm test           # vitest against a mocked API
npm run typecheck
```

Start a controller first (`dpds-controller serve`, default bind `0.0.0.0:8100`). The dev server proxies `/api` to
`DPDS_CONTROLLER`, default `http://127.0.0.1:8100`; set it to reach a controller on another host or port:

```sh
DPDS_CONTROLLER=http://10.0.0.5:9000 npm run dev
```

Use `scheme://host:port` without a path. The controller checks the `Host` header against DNS rebinding, so a
host name other than an IP address, `localhost` or the machine's host name must be allowed on the controller with
`--allowed-host` (`DPDS_ALLOWED_HOST`). Node >= 18.18 works; CI builds with Node 22.

## Build

```sh
npm run build      # tsc --noEmit && vite build -> dist/
```

`dist/` goes to `/` with the hashed files under `/assets/`. To run the controller with your own build, point the
controller build at it: `DPDS_FRONTEND_DIST=frontend/dist ./dpds.sh install` (from the repository root, no download then), or let
`cargo build -p dpds-controller` pick up `frontend/dist` by itself.
