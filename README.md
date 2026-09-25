# Yard

**A small, boring deployment CLI for self-hosted Docker Compose projects.**

Yard gives a single-server homelab a consistent operational interface without introducing a full container orchestrator.

```bash
yard list
yard status hello-api
yard host
yard deploy hello-api
yard restore-points hello-api
yard restore hello-api release:abc123def456 --yes
yard restore-log hello-api
yard logs hello-api
yard backup hello-api
```

Yard sits on top of tools you already trust — Git, Docker Compose, HTTP health checks, and existing backup commands — and turns common deployment operations into predictable, repeatable commands.

An optional private **Yard Web** dashboard can expose the current health and deployed release of the projects already configured in Yard. It uses the same project manifests; there is no second status-page configuration to maintain.

## Why Yard?

A homelab often starts with a few Compose files and eventually grows its own collection of shell snippets:

```text
git pull
build the image
run migrations
restart the service
check /health
remember the previous version
run the backup script
```

That works, but each application ends up being operated differently. Yard provides a small common layer instead.

```text
                 ┌─────────────┐
                 │    Yard     │
                 └──────┬──────┘
                        │
             project TOML manifest
                        │
        ┌───────────────┼────────────────┐
        │               │                │
       Git        Docker Compose      backups
        │               │                │
        └────── deploy / rollback ───────┘
                        │
                   health check
                        │
                  optional Web UI
```

## Philosophy

Yard aims to be:

- **small** — a CLI with an optional status UI, not a platform;
- **explicit** — deployment behavior lives in readable project manifests;
- **boring** — use standard Linux and Docker primitives instead of inventing new infrastructure;
- **safe by default** — backups, health checks, immutable image tags, and deliberate rollback behavior;
- **project-agnostic** — application-specific details belong in configuration, not hard-coded into Yard;
- **single-host friendly** — designed first for self-hosted servers and homelabs.

Yard is **not Kubernetes**, a scheduler, a service mesh, a secret manager, or a highly-available control plane.

## Requirements

Yard targets Linux hosts with:

- Git;
- Docker with the `docker compose` plugin;
- either Rust/Cargo or Docker when installing Yard from source.

If Cargo is available, `install.sh` builds Yard directly. Otherwise it uses the official Rust Docker image to build the CLI, so Rust does not need to be installed permanently on the host.

The installed Yard CLI is a standalone native binary. Yard Web is also a native Rust server built inside Docker; no Python application runtime is required.

## Installation

Clone the repository and install Yard:

```bash
git clone https://github.com/godart-corentin/yard.git
cd yard
./install.sh
```

The installer creates or installs:

```text
/usr/local/bin/yard
/usr/local/share/yard/web-src/
/etc/yard/projects/
/var/lib/yard/
```

The source checkout can also be updated later with:

```bash
./update.sh
```

## Project manifests

Projects are defined as TOML files in:

```text
/etc/yard/projects/*.toml
```

For a service monitored by URL but not deployed by Yard, a manifest may contain
only `[deployment]` and `health_url`. `yard status` checks that URL and reports
the HTTP result; no Git repository, Compose file, or release state is needed.
Deployment commands require the full manifest below.

A generic example is included at [`examples/hello-api.toml`](examples/hello-api.toml):

```toml
repo = "/srv/hello-api"
branch = "main"
remote = "origin"

[compose]
directory = "/srv/hello-api/deploy"
file = "docker-compose.yml"
env_file = ".env"
service = "api"

[image]
name = "hello-api"
tag_env = "HELLO_API_IMAGE_TAG"

[deployment]
migration_service = "migrate"
health_url = "https://api.example.com/health"
health_attempts = 30
health_interval_seconds = 2

[backup]
command = ["/usr/local/sbin/hello-api-backup"]
# Optional: execute only after the local command succeeds (no shell is used).
# The destination is descriptive metadata; Yard cannot infer it from the command.
offsite_command = ["/usr/local/sbin/hello-api-offsite-copy"]
offsite_destination = "remote:hello-api"
```

The corresponding Compose service should use the configured tag environment variable, for example:

```yaml
services:
  api:
    image: hello-api:${HELLO_API_IMAGE_TAG:-local}
    build:
      context: ..
```

This is how Yard builds immutable application images tagged with the Git revision, then switches Compose to the selected release.

For a release spanning multiple Compose services, use `services = ["api", "worker"]` instead of `service = "api"` in `[compose]` (see [`examples/hello-api-worker.toml`](examples/hello-api-worker.toml)). The historical `service` key remains supported and is treated as a one-item list; specifying both keys is an error. The list must not be empty and names must be distinct, nonempty Compose service identifiers. This is an **application-only allowlist**: never include a database, data store, or migration service. Each listed service needs its own `image:` reference incorporating the shared `[image].tag_env` (for example `hello-api:${HELLO_API_IMAGE_TAG:-local}` and `hello-worker:${HELLO_API_IMAGE_TAG:-local}`), plus a `build:` section. Yard resolves the actual image references via `docker compose config`, so `[image].name` remains required for historical manifests but does not force a single image name across services.

Secrets do **not** belong in Yard manifests. Keep them in the application's own protected environment or secret files.

Yard records the latest local backup attempt and the separate off-site copy attempt in the project state. `yard status` and the Web dashboard show the outcome, start time, duration and configured destination. If `backup.directory` or `backup.offsite_destination` is omitted, the destination is unknown; Yard does not infer a file, size or file count from the command or existing files. Without a recorded run, both views say so explicitly. An off-site failure is recorded separately and makes the backup command fail (and stops deploy before activation), while the local attempt remains a success. An unsuccessful local backup skips the off-site command. Restore/rollback never invokes either backup command. Existing manifests need no changes.

If a stored backup attempt has invalid fields, `yard status` rejects the state file; the Web API returns a sanitized `{"result":"invalid"}` record and the dashboard displays “Invalid backup record” instead of claiming no backup was recorded.

## Commands

```bash
# Discover configured projects
yard list

# Inspect Git, deployment state, Compose containers and host metrics
yard status hello-api

# Short overview of all projects (alerts for unhealthy services or image drift)
yard status

# Inspect only the host and refresh the Web snapshot
yard host

# List protected and reclaimable images; preview, then explicitly confirm removal
yard images
yard images --prune
yard images --prune --yes

# Deploy the configured branch
yard deploy hello-api

# Inspect recorded releases and backup attempts, then restore a named application image
yard restore-points hello-api
yard restore hello-api release:abc123def456 --yes
yard restore-log hello-api

# Follow application logs
yard logs hello-api

# Show the last 50 lines without following
yard logs hello-api --tail 50 --no-follow

# Follow only the api service's logs from the last two hours
yard logs hello-api --service api --since 2h --follow

# Show only the worker service's logs since a specific timestamp
yard logs hello-api --service worker --since 2026-09-24T08:00:00Z --no-follow

# Run the configured backup command
yard backup hello-api
```

`yard status` (without a project) prints one summary line per configured project, including recorded releases, deployment and backup timestamps, service probes, checkout disk usage and alerts for stopped/missing containers or application image drift. A configured migration service that exited successfully is expected. Data services are monitored for stopped containers but their images are not compared with application releases. `yard status <project>` keeps its existing Git, release, backup, Compose and host sections and adds detailed probe measurements and timestamps. Disk (repo) counts allocated bytes in the local checkout only: Docker volumes, backup files outside the checkout and remote copies are not attributed to a project. An unreadable state is reported as an alert, never as a missing deployment.

Image cleanup is CLI-only and never scheduled. It protects images recorded for the current and previous release of every configured project, and skips images used by running containers. Only 12-character Git-revision tags in repositories recorded by those releases are candidates; unrelated Docker images are never selected. The byte estimate is the sum of image sizes (shared layers can reduce actual savings). A project with missing or invalid release state, an interrupted deployment, or a failed Docker inspection blocks the entire prune rather than guessing what is safe. Legacy releases without recorded service images require a new deployment before pruning is available.

`logs` follows by default (`--follow` is explicit; `--no-follow` exits after printing). Without `--service`, it includes all services configured for the project. `--service` must exactly match a configured Compose service; an unknown name reports the valid services. `--since` accepts a duration or timestamp understood by Docker Compose and is passed through unchanged.

For development and tests, the system directories can be overridden:

```bash
yard --projects-dir ./projects --state-dir ./state list
```

or with `YARD_PROJECTS_DIR` and `YARD_STATE_DIR`.

### Host monitoring

`yard status <project>` ends with a `HOST` section; `yard host` shows the same section without a project. It reports CPU utilization and load, RAM, physical filesystem usage, Docker images/containers/volumes usage, and the state of configured Yard Compose containers. For example:

```text
HOST
  CPU          45.0% [Normal]
  Load         1.00 / 2.00 / 3.00 [Normal]
  RAM          1.0 GiB / 2.0 GiB [Warning]
  Disk         /: 8.2 GiB / 10.0 GiB [Warning]
  Docker       images 2GB, containers 1MB, volumes 3GB [Normal]
  Container    hello-api / api: exited [Critical]
```

By default, disk usage strictly above 80% is `Warning`, strictly above 90% is `Critical`, RAM usage strictly above 85% is `Warning`, and a stopped container is `Critical`. At the exact threshold the lower level still applies. A configured project with no Compose containers yet is `Unknown` ("No containers for a configured project"), not `Critical`; this status is preserved in the CLI, snapshot and Web view. Override thresholds with `YARD_DISK_WARN_PERCENT`, `YARD_DISK_CRIT_PERCENT`, and `YARD_MEM_PRESSURE_PERCENT` (integers 0–100); invalid values use defaults. A missing measurement is `Unknown`, not a failed command. Physical mounts come from Linux mountinfo; container overlay filesystems are not treated as host disks.

Each CLI collection atomically replaces `host.json` in the state directory (`--state-dir` / `YARD_STATE_DIR`), including when collections overlap. If saving fails, the CLI reports the error on stderr but still displays the host status. The project name `host` is reserved to avoid colliding with this snapshot. The file has a version, collection timestamp, applied thresholds, metrics and computed statuses, but no application environment or secrets. Web only reads this snapshot; it never measures the host itself. A regular external scheduler can run `yard host` to keep the Web view current; Yard installs no timer or service.

## Yard Web

Yard Web is an optional private dashboard for the projects already registered in Yard.

It reads:

- the project inventory from `/etc/yard/projects/*.toml`;
- `deployment.health_url` for the current HTTP health check;
- `/var/lib/yard/<project>.json` for the deployed release metadata.
- `/var/lib/yard/host.json` for the last CLI-produced host snapshot, when available.

A project without `deployment.health_url` still appears, but its health is reported as `Unknown`.

The dashboard shows projects as a responsive card grid with current health, latency, HTTP status, last check, and deployed release information when available.
It also displays host metrics and their precomputed states with the measurement age. After five minutes without a new snapshot, or if the file is unreadable or incompatible, host data is explicitly unavailable while project health remains functional. Set `YARD_WEB_HOST_MAX_AGE_SECONDS` to override the five-minute freshness limit.

### Install Yard Web

From a Yard source checkout:

```bash
sudo ./install-web.sh status.example.com
```

`install-web.sh` prefers the `web/` source next to the script and falls back to `/usr/local/share/yard/web-src` when needed. The Rust web server is compiled by the Dockerfile, so Cargo is not required on the host for this step.

The installer:

1. discovers the running Caddy container and its mounted Caddyfile;
2. reuses an existing Docker network already attached to Caddy;
3. asks for a Basic Auth username and password;
4. stores only Caddy's password hash in the Caddyfile;
5. mounts Yard project manifests and deployment state read-only;
6. builds and starts the `yard-web` container without publishing a host port;
7. validates and reloads Caddy;
8. waits for `yard-web` to become healthy.

If Caddy network discovery is ambiguous, choose one explicitly:

```bash
sudo YARD_PROXY_NETWORK=caddy-proxy ./install-web.sh status.example.com
```

The generated Compose project lives at:

```text
/opt/yard/docker-compose.yml
```

Caddy and previous Yard Web configuration are backed up under:

```text
/opt/yard/backups/
```

Yard Web exposes these endpoints inside its Docker network:

```text
GET /healthz
GET /api/status
```

The browser refreshes automatically, while the server briefly caches health results to avoid duplicate checks.

More detail is available in [`docs/web.md`](docs/web.md).

## Deployment model

`yard deploy <project>` follows this lifecycle:

1. refuse to deploy if tracked local Git changes exist;
2. switch to the configured branch;
3. fetch and fast-forward from the configured Git remote;
4. run the project's backup command when configured;
5. derive an immutable image tag from the updated Git commit SHA;
6. resolve and build every configured Compose service using that same tag (one Git commit for the whole release);
7. run the migration service when configured;
8. persist the new image tag in the project's Compose `.env` file;
9. start each listed application service (`--no-deps`), leaving persistent dependencies untouched;
10. wait for the configured HTTP health check and verify the running service images;
11. when any `[service_health.<service>]` probe has `deployment_gate = true`, wait for every gated probe to become Healthy;
12. atomically record the active release (revision, service/image references, timestamp and status) and the previous release under `/var/lib/yard`.

Build or migration failures leave the active release unchanged and report the running containers. If activation, the health check or a deployment gate fails, Yard names the blocking services (the configured HTTP check is associated with the first service), reports actual Compose containers, and attempts to restore the previous application images for *all* services. It never records a partially activated release as active. A pending release marker is written before changing the Compose tag and remains `activating` throughout gate checks; it is cleared only after a verified activation or restoration. `yard status` highlights pending work and image mismatches. After an interrupted/failed restoration, inspect `yard status` and `yard restore-points <project>`, then explicitly choose the recorded application release with `yard restore <project> release:<tag> --yes` before attempting another deploy.

Service probes remain informational unless `deployment_gate = true` (default `false`). A gated HTTP or heartbeat probe must be Healthy; Unknown, Degraded and Unhealthy never count. A missing heartbeat is retried rather than rejected immediately. `[deployment] gate_attempts = 30` and `gate_interval_seconds = 2` are the defaults (30 attempts, one immediately and the rest two seconds apart); both must be positive. A heartbeat predating completion of the new release's Compose startup cannot validate it, even if it is otherwise fresh. Because timestamps have one-second precision, beats from the same second as service startup are ignored too: the worker must emit periodically so a later beat can pass. Gate waiting remains bounded by the configured timeout. A timed-out gate fails deployment and triggers the same application-image rollback. Manual `yard restore` does not enforce gates, so a recovery cannot be blocked by a missing worker heartbeat. The legacy `deployment.health_url` check remains independent.

Compose `.env` files that are symbolic links are refused by `yard deploy` and `yard restore` (including the `rollback` alias). Replace the link with a regular file at the configured `[compose].directory` / `[compose].env_file` path before retrying; keep its permissions and contents protected. Yard also refuses to reuse the old predictable temporary file: for `env_file = ".env"` in `/srv/hello-api/deploy`, it is `/srv/hello-api/deploy/..env.yard.tmp` (two leading dots). A leftover from an earlier Yard version, or any other planted file or link at that path, must be inspected and removed **manually**; Yard never deletes a pre-existing path. For example:

```bash
ls -ld -- /srv/hello-api/deploy/..env.yard.tmp
# After checking it is not needed, remove the path itself (not a link target):
rm -- /srv/hello-api/deploy/..env.yard.tmp
```

After a refusal, read the exact path and reason in the error, inspect `yard status <project>` and the path, then correct the `.env` link or remove the confirmed obsolete temporary file. A refusal detected before activation leaves no new `pending` marker; retry `yard deploy <project>` once the obstruction is gone. If `yard status` already reports a `pending` release from an interrupted run, remove the obstruction first, explicitly choose the recorded active image with `yard restore <project> release:<tag> --yes`, check status, and only then retry deploy. Do not edit the state JSON by hand.

Database rollback is deliberately separate. Yard never restores a database automatically just because an application image was rolled back.

Yard treats long-lived dependencies such as databases as already-provisioned infrastructure. A migration service may start the dependencies it needs, but release activation itself uses `docker compose up --no-deps` so a routine application deploy does not unexpectedly recreate PostgreSQL, Redis, or other persistent services.

## Explicit application recovery

`yard restore-points <project>` lists the recorded current, previous and pending releases, plus the latest local and off-site backup attempts, with timestamps, age, outcome and destination. Malformed individual records are marked `unreadable`, not `absent`. Backup records are metadata, **not restorable database targets**. Only recorded releases are listed; Yard does not claim to enumerate all historical images.

Select a release by the displayed `release:<tag>` identifier (or a Git revision when unambiguous), then explicitly confirm:

```bash
yard restore-points hello-api
yard restore hello-api release:abc123def456 --yes
yard restore-log hello-api
```

`yard rollback` is an alias with the **same required target and `--yes`**; neither command has an implicit “previous” action. A revision not recorded in Yard state is resolved through Git, but its already-built images must exist locally. No rebuild, migration or automatic data restore is performed. Before either activation or failure recovery, Yard checks that every recorded release service matches the current application-only Compose allowlist exactly and is not the configured migration service. A changed service list requires manual inspection and a corrected application manifest; Yard will not start an old service that is no longer authorized. The manifest and Compose file are trusted operator configuration: do not designate data services as applications. Before changing the image tag or project state, Yard stores timestamped private copies of the state JSON and Compose `.env` in the state directory (`<project>.restore-<timestamp>.json` and `.env`, mode `0600`). It writes a pending marker before activation and verifies running services and health before recording the target as current. On activation failure it tries to reactivate the original image; if recovery also fails the pending marker remains and the error names the snapshot for manual inspection. Every attempted restore, including refusals, is appended to `<project>.restore.jsonl` and viewable via `yard restore-log`.

### Manual data recovery

`yard restore <project> backup:local --yes` (also `backup:offsite`) is **always refused**. A backup destination shown by Yard is descriptive, not a verified restorable archive. For database recovery, inspect the backup and destination manually, stop all writers, follow the database engine's documented restore procedure *outside Yard* with a separately verified recovery plan, validate the recovered data, then restart application services. Never equate an application image rollback with a database rollback. Yard's restore command invokes neither configured backup commands nor migrations and never restores a database or volume.

## State

Deployment state is stored as JSON under:

```text
/var/lib/yard/<project>.json
```

The state contains only deployment metadata: Git revisions, service names and image/tag references, timestamps, statuses (`active`, `superseded`, `activating`) and the previous release. Existing state files with only `revision`, `tag`, and `deployed_at_unix` remain readable; missing service metadata is populated from Compose when a deployment or restore uses that release. Writes remain atomic; new state files are mode `0600` and updates preserve the existing mode. Restore snapshots of Compose `.env` may contain application secrets: protect the state directory and its `0600` snapshots accordingly.

## Security

Recommended practices:

- keep project manifests free of credentials;
- keep application `.env` files protected (`0600` where appropriate); Yard preserves
  the existing file mode when updating a Compose `.env` file, and creates its
  temporary replacement with mode `0600` (it does not create a missing `.env`).
  Docker Compose reads `--env-file` as the same Unix user running Yard, so `0600`
  works for that deployment; if a different user must read it, configure its
  access explicitly rather than making secrets world-readable;
- do not publish database ports unless explicitly required;
- use private Docker networks for internal services;
- back up persistent data independently from application images;
- make database restoration an explicit administrative operation;
- use a dedicated Unix user or carefully scoped `sudo` permissions if Yard should not run as an unrestricted administrator.

Yard executes configured backup commands directly, without a shell. This avoids shell expansion in manifest values, but project manifests are still privileged operational configuration and should only be writable by trusted administrators.

Yard Web is designed to remain private behind Caddy Basic Auth. Its container runs unprivileged, publishes no host port, mounts Yard configuration and state read-only, uses a read-only root filesystem, drops Linux capabilities, and does not read application `.env` files.

## Remove Yard Web

```bash
sudo ./uninstall-web.sh
```

This removes the Yard Caddy block and the Yard Web Compose project. It leaves `/etc/yard/projects` and `/var/lib/yard` untouched.

## Status

Yard is in **early development**. The command surface and manifest format may change while the deployment, rollback, and status-dashboard model is being hardened.

The first target is a single-host Docker Compose homelab. More abstraction should only be added when real deployments demonstrate a need for it.

## License

MIT
