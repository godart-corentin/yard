# Yard Web status page

Yard Web is an optional private status page for the projects already configured in Yard.
It does not introduce a second project inventory: `/etc/yard/projects/*.toml` remains the source of truth.

For every project, Yard Web reads:

- the project name from the TOML filename;
- `deployment.health_url` for the HTTP health check;
- `/var/lib/yard/<project>.json` for the currently deployed release.

Projects without `deployment.health_url` still appear, but their health is reported as `Unknown`.

## Install

Yard Web follows the same deployment model as KilnR Web: a dedicated Docker container behind the existing Caddy reverse proxy.
No host port is published.

From a Yard source checkout:

```bash
sudo ./install-web.sh status.example.com
```

The installer:

1. discovers the running Caddy container and its mounted Caddyfile;
2. reuses Caddy's existing Compose default network when possible;
3. asks for a Basic Auth password (username defaults to `yard`);
4. stores only Caddy's password hash in the Caddyfile;
5. mounts `/etc/yard/projects` and `/var/lib/yard` read-only into `yard-web`;
6. builds and starts the `yard-web` container;
7. validates and reloads Caddy.

Override the username with:

```bash
sudo YARD_WEB_USER=corentin ./install-web.sh status.example.com
```

If automatic Caddy network discovery is ambiguous, choose an existing network already attached to Caddy:

```bash
sudo YARD_PROXY_NETWORK=caddy-proxy ./install-web.sh status.example.com
```

The generated Compose project lives at:

```text
/opt/yard/docker-compose.yml
```

Caddy configuration and previous Yard Web Compose files are backed up under:

```text
/opt/yard/backups/
```

## Status API

The container exposes the following endpoints only to its Docker network:

```text
GET /healthz
GET /api/status
```

`/api/status` reports the overall state and, per project:

- current health (`Operational`, `Down`, or `Unknown`);
- health URL;
- HTTP status when available;
- request latency;
- current Yard release tag/revision and deployment timestamp.

The browser refreshes the status automatically every 30 seconds. Health responses are cached briefly by the server to avoid duplicate checks.

## Security

Basic Auth is enforced by Caddy, not by the application container.

`yard-web`:

- publishes no host port;
- mounts Yard configuration and state read-only;
- runs with a read-only root filesystem;
- drops Linux capabilities;
- enables `no-new-privileges`;
- has conservative process, memory, and CPU limits;
- never reads application `.env` files or secrets.

Only `deployment.health_url` and Yard's deployment metadata are exposed to the web UI.

## Remove

```bash
sudo ./uninstall-web.sh
```

This removes the Yard Caddy block and the `yard-web` Compose project. It does not modify `/etc/yard/projects` or `/var/lib/yard`.
