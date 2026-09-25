# Yard Web status page

Yard Web is an optional private status page for the projects already configured in Yard.
It does not introduce a second project inventory: `/etc/yard/projects/*.toml` remains the source of truth.

For every project, Yard Web reads:

- the project name from the TOML filename;
- `deployment.health_url` for the HTTP health check;
- `/var/lib/yard/<project>.json` for the currently deployed release.
- `/var/lib/yard/host.json` for host metrics and their precomputed statuses.

Projects without `deployment.health_url` still appear, but their health is reported as `Unknown`.
For an external service that Yard does not deploy, a manifest containing only
`[deployment]` and `health_url` is an URL monitor. `yard status` checks the URL
and reports its HTTP result without requiring Git, Compose, or a release state.
Deployment commands still require a full project manifest.
For per-service health, declare `[service_health.<compose service>]` with either
`type = "http"`, `url = "https://..."` and `timeout_ms = 2000`, or
`type = "heartbeat"`, an absolute in-container `path` and `max_age_seconds`.
See `examples/hello-api-worker.toml`. A worker periodically touches the file
inside its own container (for example after a successful processing cycle).
The CLI reads its mtime using `docker compose exec -T <service> stat -c %Y -- <path>`;
the worker needs only `touch` and `stat` in the container. A missing, unreadable,
future-dated or stopped source can never be Healthy. Keep the file on the
container's ephemeral filesystem, not a persistent volume. No Docker socket or
worker filesystem is exposed to Web. No broker, agent or new dependency is needed.
Probes are informational by default (`deployment_gate = false`). Set
`deployment_gate = true` on either probe type to block a new release until all
gated services are Healthy, after Compose startup and image verification.
Unknown, Degraded and Unhealthy do not pass; a missing heartbeat is retried
until the configured timeout, not failed immediately. Set positive
`[deployment] gate_attempts` (default 30) and `gate_interval_seconds` (default 2)
to control the attempts and spacing. A heartbeat predating completion of the
new release's Compose startup never counts, even if its age is within the
configured limit. Timeout fails deployment and triggers application-image
rollback. Manual `yard restore` bypasses these gates for emergency recovery;
the separate `deployment.health_url` check is unchanged. Web only displays
the existing probe states; it does not run the deployment gate.
An undeclared service probe is explicitly Unknown in the CLI and host snapshot;
the dashboard hides its unconfigured probe row.

At collection time HTTP latency above `YARD_HTTP_WARN_MS` (default 500) is
Degraded; above `YARD_HTTP_CRIT_MS` (default 2000), Unhealthy. A heartbeat older
than its `max_age_seconds` is Degraded; older than that limit times
`YARD_HEARTBEAT_CRIT_MULTIPLIER` (default 2, minimum 2), Unhealthy. Invalid
threshold overrides use defaults. HTTP errors, non-2xx responses and stopped
containers are Unhealthy. Schedule `yard host` often enough for HTTP freshness
(for the default five-minute snapshot limit, every two minutes works):

```cron
*/2 * * * * /usr/local/bin/yard host >/dev/null 2>&1
```

The Web reader recomputes heartbeat age on every API request from its timestamp.
Once the host snapshot exceeds `YARD_WEB_HOST_MAX_AGE_SECONDS`, probes become
Unknown until the CLI collects again. A previously saved version-1 snapshot
without a `services` field remains readable.
The CLI (`yard status <project>` or `yard host`) collects CPU, load, RAM, physical disks, Docker usage and Yard container states on the host and writes an atomic, versioned snapshot. Web only reads this file through its existing read-only state mount: it has no Docker socket, host disk mounts, or host-monitoring permissions. Schedule `yard host` externally if continuous refresh is wanted; Yard installs no timer.

## Install

Yard Web follows the same deployment model as Kilnr Web: a dedicated Docker container behind the existing Caddy reverse proxy.
The application server is a standalone Rust binary and no host port is published.

Install or update Yard first. The main installer copies the reproducible Yard Web build context to `/usr/local/share/yard/web-src`, so the web deployment does not depend on keeping a source checkout:

```bash
./install.sh
```

Then, from the Yard source checkout:

```bash
sudo ./install-web.sh status.example.com
```

The installer:

1. discovers the running Caddy container and its mounted Caddyfile;
2. reuses Caddy's existing Compose default network when possible;
3. asks for a Basic Auth password (username defaults to `yard`);
4. stores only Caddy's password hash in the Caddyfile;
5. mounts `/etc/yard/projects` and `/var/lib/yard` read-only into `yard-web`;
6. builds the Rust `yard-web` binary from the installed web source;
7. starts the `yard-web` container as an unprivileged user;
8. validates and reloads Caddy.

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
- `services`: sanitized CLI measurements by Compose service (health state,
  HTTP latency in milliseconds or current heartbeat age in seconds). Project
  state derives from these service measurements when service probes are
  configured; the legacy `deployment.health_url` behavior remains for old
  manifests. Without a fresh snapshot, service measurements are Unknown;
- current Yard release tag/revision, deployment timestamp, and `release.services` (each service name and image reference, useful for checking exactly which image is deployed);
- `pending_release` when activation is interrupted or a rollback cannot be verified (including its status and per-service images), so the dashboard can warn that the recorded release and Docker state may differ.

The `host` field contains `status`, `age_seconds`, and a sanitized `snapshot` (version 1) with the host metrics, applied thresholds, and statuses computed by the CLI. Web does not recompute warnings. A configured project with no Compose containers yet has `containers_status: "unknown"` and the message "No containers for a configured project"; only a stopped container is `Critical`. The host view becomes `Unknown` when the snapshot is missing, unreadable, invalid, from an unknown version, or older than 300 seconds; stale age is still shown. Override with `YARD_WEB_HOST_MAX_AGE_SECONDS`. The rest of `/api/status` continues to work. Project name `host` is reserved for the snapshot filename.

The dashboard's **Host** section shows only CPU, RAM, physical disks and the measurement age. Its badge reflects the most severe CPU/RAM/disk status (or `Unknown` if the snapshot is unavailable); stopped containers, load and Docker usage do not affect it. The **Services** section lists each container beneath its matching project card, with the Compose service name, state and status. A configured migration service that exited with code 0 appears as `Completed` and does not degrade the project. Other stopped containers degrade their service even when the HTTP check succeeds; a failed HTTP check leaves the service `Down`. The Services badge and summary counts reflect these displayed service states. Without a fresh host snapshot, container states cannot be shown and project badges reflect HTTP health alone. The CLI still displays Load, Docker usage and containers, and `/api/status` still exposes all of them in the snapshot; they are simply not displayed in Host.

The browser refreshes the status automatically every 30 seconds. Health responses are cached briefly by the server to avoid duplicate checks.

## Update

Update the Yard CLI, installed web source and any running web container together:

```bash
./update.sh
```

If `yard-web` is running, the updater rebuilds its image from the newly installed Rust server and frontend source.

## Security

Basic Auth is enforced by Caddy, not by the application container.

`yard-web`:

- runs a native Rust HTTP server with no Python application runtime;
- publishes no host port;
- mounts Yard configuration and state read-only;
- runs with a read-only root filesystem;
- drops Linux capabilities;
- enables `no-new-privileges`;
- has conservative process, memory, and CPU limits;
- never reads application `.env` files or secrets;
- reads only known public fields from the host snapshot (no arbitrary JSON forwarding).

Only `deployment.health_url`, Yard's deployment metadata and sanitized service/host snapshot fields are exposed to the web UI; application environment files are never read.

## Remove

```bash
sudo ./uninstall-web.sh
```

This removes the Yard Caddy block and the `yard-web` Compose project. It does not modify `/etc/yard/projects` or `/var/lib/yard`.
