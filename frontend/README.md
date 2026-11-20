# DPDS frontend

React + TypeScript + Vite web UI for the `dpds-controller` REST API (`/api`). The
controller embeds the built `dist/` and serves it on its own port (default 8100);
`dpds.sh install` and `update` build it with `npm ci` and `npm run build`.

```sh
npm ci
npm run dev        # http://localhost:5173
npm test           # vitest against a mocked API
npm run typecheck
npm run build      # tsc --noEmit && vite build -> dist/
```

The dev server proxies `/api` to a running controller at `http://127.0.0.1:8100`.
Set `DPDS_CONTROLLER=http://host:port` to use another one. A host name other than
an IP, `localhost` or the machine's name must be allowed on the controller with
`--allowed-host`. Node >= 26 is required.
