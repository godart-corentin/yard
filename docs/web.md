# Yard Web

Yard Web is an optional private web interface for the projects already configured in Yard.
It shows their status and runs the same actions as the CLI. It does not introduce a second
project inventory: `/etc/yard/projects/*.toml` remains the source of truth.

The wire contract (endpoints, job model, executor protocol) is in [web-jobs.md](web-jobs.md).
This page explains what Web can do, how it is built and what it means for security.

## What Web can do

Reads (no side effect):

- project health, deployed release, host metrics and service probes (`/api/status`);
- bounded one-shot application logs, recorded restore points, the restore journal and the image
  inventory, plus the dry run of an image prune (`/api/read`);
- live application logs, streamed (`/api/logs/stream`);
- the history and live output of jobs (`/api/jobs`).

Actions, run as **jobs** on the host by the Yard CLI (`POST /api/jobs`):

| Web action | Equivalent CLI command |
| --- | --- |
| `deploy` | `yard deploy <project>` |
| `backup` | `yard backup <project>` |
| `restore` (a recorded restore point, explicit confirmation) | `yard restore <project> <revision> --yes` |
| `images-prune` (explicit confirmation, always all eligible candidates) | `yard images --prune --yes` |
| `host-collect` | `yard host` |

There is one running job per project; image prune and host collection exclude every other job.
Each job records who started it (the Basic Auth user), its state, its exit code and its full output.
Jobs cannot be cancelled in this version. Everything an authenticated user can see, they can also
act on: there are no roles.

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
configured limit. With one-second timestamps, a beat in the same second as
service startup is ignored too: the worker must emit periodically for a later
beat to count. Waiting remains bounded by the configured timeout; a timeout
fails deployment and triggers application-image rollback. Manual `yard restore`
bypasses these gates for emergency recovery;
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
The CLI (`yard status <project>` or `yard host`) collects CPU, load, RAM, physical disks, Docker usage and Yard container states on the host and writes an atomic, versioned snapshot. Web reads this file through its read-only state mount: it has no Docker socket, host disk mounts, or host-monitoring permissions. Schedule `yard host` externally if continuous refresh is wanted; Yard installs no timer.

## Architecture

```text
browser
  | HTTPS + Basic Auth
Caddy        checks the password, then overwrites X-Yard-User and X-Yard-Proxy-Token
  | private Docker network, no published port
yard-web     read-only mounts, no Docker socket, unprivileged, read-only root filesystem
  |-- reads  /etc/yard/projects, /var/lib/yard (including jobs/)   (read-only bind mounts)
  |-- reads  /run/yard-web-read/read.sock  --> yard-web-read.service (yard read-server)
  `-- writes /run/yard-web-ops/ops.sock    --> yard-web-ops.service  (yard ops-server)
```

- **Caddy** is the only authentication. It is also the trusted proxy: it tells Web who the user is.
- **yard-web** (Rust, axum) serves the React build, the status and read APIs, the jobs API and the
  Server-Sent Events streams. It never runs a command itself.
- **Read executor** (`yard-web-read.service`): bounded, allowlisted reads (`images`, `images-prune-plan`,
  `logs`, `logs-follow`, `restore-points`, `restore-log`). Sandboxed (read-only filesystem, no
  capabilities, private network), runs as root only to read mode-0600 state and reach Docker.
- **Ops executor** (`yard-web-ops.service`, `yard ops-server`): starts the five job actions above. It
  accepts a closed set of actions and builds each command line from a fixed template, so the request
  cannot choose a command or free-form arguments. See "Security" for its privileges.
- **Job storage**: the ops executor writes `/var/lib/yard/jobs/<id>.json` (state, atomically replaced)
  and `<id>.log` (append-only output, capped at 16 MiB) and removes terminal jobs after 90 days. Web
  reads these files through the state mount it already has, so job history and output stay readable
  while the executor is down. Web never writes there.

Each executor socket is a separate directory with its own group (`yard-web-read`, `yard-web-ops`);
the Web container user joins both groups, and nothing else on the host does by default.

## Install

Yard Web is a dedicated Docker container behind the existing Caddy reverse proxy. The application
server is a standalone Rust binary and no host port is published.

Install or update Yard first. The main installer copies the reproducible Yard Web build context to
`/usr/local/share/yard/web-src` and the ops executor files to `/usr/local/share/yard/ops-src`, so the
web deployment does not depend on keeping a source checkout:

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
3. asks for a Basic Auth password (username from `YARD_WEB_USER` or a prompt);
4. creates, once, a random proxy token in `/opt/yard/web.env` (root, mode 0600) and reuses it on
   re-installs and updates; the token is never printed;
5. writes the Yard block in the Caddyfile: Basic Auth (password hash only) and, in `reverse_proxy`,
   `header_up X-Yard-User {http.auth.user.id}` and `header_up X-Yard-Proxy-Token <token>`;
6. installs and starts `yard-web-read.service` and `yard-web-ops.service`, and waits for their sockets;
7. mounts `/etc/yard/projects`, `/var/lib/yard` and both socket directories **read-only** into `yard-web`;
8. builds the React frontend (Node) and the Rust server in a multi-stage Docker build and starts the
   container as an unprivileged user (65532) with `YARD_WEB_PROXY_TOKEN` taken from `web.env`;
9. validates and reloads Caddy.

Override the username with `sudo YARD_WEB_USER=corentin ./install-web.sh status.example.com`. If the
Caddy network is ambiguous, choose one with `YARD_PROXY_NETWORK=caddy-proxy`.

Files created:

```text
/opt/yard/docker-compose.yml          Compose project (contains no secret)
/opt/yard/web.env                     root:root 0600, YARD_WEB_PROXY_TOKEN
/opt/yard/backups/<timestamp>/        previous Caddyfile (holds the token and password hash) and Compose file, root only
/etc/systemd/system/yard-web-read.service
/etc/systemd/system/yard-web-ops.service
```

Re-running `install-web.sh` after an upgrade is how unit changes are applied; `update.sh` only
restarts the units.

### Proxy identity

Yard Web serves every `/api/*` route (status, reads, job list and output, the SSE log stream and mutations)
only when the request carries the proxy token; `/healthz` and the static UI files stay open. Caddy adds it, and the user
name, to every proxied request. `header_up <name> <value>` **replaces** any value the client sent for
that header (it only appends with a `+` prefix, which the installer does not use), so a browser cannot
choose its own identity or supply the token. Web compares the token in constant time, validates the user
name as `[A-Za-z0-9._@-]{1,64}`, and never echoes either value. A missing or wrong token on `/api/*` is
`403 forbidden`. With no `YARD_WEB_PROXY_TOKEN` configured (for example a plain `docker run`), the gate is off so
the read-only dashboard keeps working (`GET /api/*` answers), while every mutation is still refused with `403`.

The token protects against **other containers on the shared Caddy network** talking to `yard-web:8088`
directly. Be aware where it lives: in clear in the Caddyfile (and so in Caddy's loaded configuration, which
the Caddy admin API serves; it listens on localhost inside the Caddy container by default, do not publish
or expose it), in `/opt/yard/web.env`, and in the environment of the `yard-web` container, which anyone able
to run `docker inspect` can read. Keep the Caddyfile unreadable by other users (`chmod o-rw`; the installer warns
when it is not). To rotate: delete `/opt/yard/web.env` and run `install-web.sh` again.

## Update

Update the Yard CLI, the installed web source and any running web container together:

```bash
./update.sh
```

If `yard-web` is running, the updater restarts `yard-web-read.service`, restarts `yard-web-ops.service`
**unless jobs are running** (it says so; restart it later, a restart ends running jobs and marks them
failed), and rebuilds the image from the installed Rust server and frontend source. If `/opt/yard/web.env`
does not exist yet (Web installed before Web actions), it tells you to run `install-web.sh`.

## Remove

```bash
sudo ./uninstall-web.sh
```

This removes the Yard Caddy block, the `yard-web` Compose project, `/opt/yard/web.env` and both
executor units. It does not modify `/etc/yard/projects` or `/var/lib/yard` (job history and logs stay).

## HTTP API

The container exposes these endpoints only to its Docker network (see [web-jobs.md](web-jobs.md)):

```text
GET  /healthz
GET  /api/status
GET  /api/read?op=images | images-prune-plan | logs | restore-points | restore-log
GET  /api/logs/stream?project=hello-api&service=api&tail=200     (SSE)
GET  /api/jobs                       GET /api/jobs/{id}           GET /api/jobs/{id}/output   (SSE)
POST /api/jobs                       {action, project?, revision?, confirm?}
```

`/api/status` reports the overall state and, per project:

- current health (`Operational`, `Down`, or `Unknown`), health URL, HTTP status and request latency;
- `services`: sanitized CLI measurements by Compose service (health state, HTTP latency or heartbeat age);
  without a fresh snapshot they are Unknown;
- the current Yard release (tag, revision, deployment time, per-service images) and `pending_release`
  when activation was interrupted or a rollback could not be verified;
- backup status (local and off-site attempts).

The `host` field contains `status`, `age_seconds`, and a sanitized `snapshot` with host metrics, applied
thresholds and the statuses computed by the CLI. Web does not recompute warnings. The host view becomes
`Unknown` when the snapshot is missing, unreadable, invalid, from an unknown version, or older than 300
seconds (`YARD_WEB_HOST_MAX_AGE_SECONDS`). A configured project with no Compose containers yet is
`Unknown`; only a stopped container is `Critical`. The project name `host` is reserved.

Reads show the actual bounded CLI output or an explicit unavailable state. `400` means refused, `503`
the executor could not answer, `502` a command failure, timeout, output limit or invalid reply. The UI
shows only Web's generic error messages, never backend text. One-shot logs are limited to 1000 lines,
128 KiB and 10 seconds; `since` only accepts short Docker duration/timestamp characters, and an unknown
service is refused against the project's configured Compose services. URL-only monitors have an HTTP
status but no reads and no actions.

Job requests are refused (`400`) for an unknown project, a URL monitor, a missing or unrecorded restore
point, or a missing `confirm: true` on `restore` and `images-prune`; a busy lock is `409` with the running
job id; an unreachable executor is `503`. The browser refreshes status automatically every 30 seconds.

## Security

What did **not** change: Basic Auth is enforced by Caddy, not by the application container, and the
`yard-web` container itself is still unprivileged and read-only: no published port, read-only
Yard mounts, read-only root filesystem, all capabilities dropped, `no-new-privileges`, process, memory and
CPU limits, no Docker socket, no application `.env` files. Only known public fields of the project
state and host snapshot are exposed.

What changed: Web can now start deployments, backups, restores and image prunes. Compromising a Basic Auth
account, or the Caddy host, now means **running those actions as root on the host**, where before it meant
reading status. Concretely:

- **Authorization is Basic Auth alone, with no roles.** Every authenticated user may run every action, on every
  project. Give the password only to people you would give `sudo yard` to. There is no rate limit besides
  the per-project and global job locks.
- **The ops executor is root and its hardening is relaxed** compared with the reader, because deploys cannot
  work without it. It runs as root (Docker socket, Git, project backup commands, writes `/var/lib/yard`),
  keeps network access (Git fetches, image builds, off-site backups), uses `ProtectSystem=full` instead of
  `strict` with no `ProtectHome` (repositories, Compose directories and backup targets are wherever the
  manifests say), and keeps four capabilities (`CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`, `CAP_FOWNER`,
  `CAP_CHOWN`). `NoNewPrivileges`, namespace, syscall-architecture and kernel-protection restrictions remain.
  Project manifests are privileged configuration (they name the commands that run): keep
  `/etc/yard/projects` writable by trusted administrators only, since Web will run what they say.
- **What a request can choose is small.** The action (closed list), a project that exists in
  `/etc/yard/projects` with a full manifest, and for restore a revision that is a recorded restore point.
  Commands are built from fixed templates, with no shell and a cleared environment (`PATH`, `HOME`, the
  `YARD_*` settings). Web validates everything, then the executor validates it again. Requests are limited
  to 4 KiB and unknown fields are rejected.
- **Cross-site requests are blocked.** A mutation needs the `X-Yard-Request: 1` header (a browser cannot add it
  cross-origin without a CORS preflight, and Web sends no CORS headers), `Sec-Fetch-Site` must be
  `same-origin` when present, and `Origin` must match the host. Browsers send Basic Auth credentials automatically,
  which is why these checks matter.
- **The proxy token** (see above) stops containers on the Caddy network from forging an identity or bypassing
  the Basic Auth. Every `/api/*` route, GET and SSE included, requires it, so such a container cannot read
  status, logs or job output either. Only `/healthz` and the static UI are open. Without a configured token
  reads are open (read-only mode) and mutations are refused.
- **Sockets**: the ops socket is mode 0660 in a 0750 directory owned by the dedicated `yard-web-ops` group. Only
  members of that group (the Web container) can start jobs. Anyone who can join that group, or compromise the
  `yard-web` process, can ask the executor for the same five actions, and nothing else.
- **Bounded jobs.** Each runs in its own process group with a wall-clock timeout (deploy and restore 30 min,
  backup 60 min, prune 10 min, host collection 2 min): SIGTERM, then SIGKILL after 10 s; the group is killed
  when the job ends. Output is capped at 16 MiB. At most 32 job output streams and 4 live log streams are open
  at once (1 or 2 hours each). Stopping the executor ends running jobs.
- **Job output is visible and stored in clear.** `deploy`, `backup` and `restore` print whatever the CLI and the
  commands it runs print, and `/var/lib/yard/jobs/*.log` is mode 0644 in a 0755 directory so that the read-only
  state mount works. Every authenticated user, the `yard-web` container, and every local user on the host
  can read it for 90 days. If a build or backup command may print a secret, fix the command to stay quiet; Yard does not
  redact output.

## Hardening checklist

- Serve Yard Web only over HTTPS with a strong Basic Auth password, ideally on a private network or VPN.
- Keep `/opt/yard` and the Caddyfile root-readable only; do not expose the Caddy admin endpoint.
- Do not put untrusted containers on the Caddy network.
- Keep manifests (`/etc/yard/projects`) writable by administrators only.
- Review `journalctl -u yard-web-ops.service`; every job also records its actor in `/var/lib/yard/jobs/`.
