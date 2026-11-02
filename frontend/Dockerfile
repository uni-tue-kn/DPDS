# Build stage: node 22 (the frontend also builds with node >= 18.18).
FROM node:22.23.4-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# Runtime stage: nginx serves dist/ and proxies /api/ to the controller on the host. It runs as the
# unprivileged user of the image (uid 101), so the port must be >= 1024 (the image default is 8080).
# The image renders /etc/nginx/templates/*.template into /etc/nginx/conf.d with envsubst at start (only
# variables that are set in the environment, so nginx's own $uri and $host stay untouched).
FROM nginxinc/nginx-unprivileged:1.30.6-alpine
# DPDS_FRONTEND_BIND: address nginx listens on (127.0.0.1 keeps the whole API off the network).
# DPDS_CONTROLLER: scheme, host and port of the controller, WITHOUT a path (http://host:port).
ENV DPDS_FRONTEND_BIND=0.0.0.0 \
    DPDS_FRONTEND_PORT=8080 \
    DPDS_CONTROLLER=http://127.0.0.1:8100
COPY nginx.conf /etc/nginx/templates/default.conf.template
COPY --from=build /app/dist /usr/share/nginx/html
