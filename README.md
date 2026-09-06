# Yard

**A small, boring deployment CLI for self-hosted Docker Compose projects.**

Yard gives a single-server homelab a consistent operational interface without introducing a full container orchestrator.

```bash
yard list
yard status hello-api
yard deploy hello-api
yard rollback hello-api
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

Secrets do **not** belong in Yard manifests. Keep them in the application's own protected environment or secret files.

## Commands

```bash
# Discover configured projects
yard list

# Inspect Git, deployment state and Compose containers
yard status hello-api

# Deploy the configured branch
yard deploy hello-api

# Roll back to the previous deployment
yard rollback hello-api

# Roll back to a specific revision whose image already exists locally
yard rollback hello-api <revision>

# Follow application logs
yard logs hello-api

# Show the last 50 lines without following
yard logs hello-api --tail 50 --no-follow

# Run the configured backup command
yard backup hello-api
```

For development and tests, the system directories can be overridden:

```bash
yard --projects-dir ./projects --state-dir ./state list
```

or with `YARD_PROJECTS_DIR` and `YARD_STATE_DIR`.

## Yard Web

Yard Web is an optional private dashboard for the projects already registered in Yard.

It reads:

- the project inventory from `/etc/yard/projects/*.toml`;
- `deployment.health_url` for the current HTTP health check;
- `/var/lib/yard/<project>.json` for the deployed release metadata.

A project without `deployment.health_url` still appears, but its health is reported as `Unknown`.

The dashboard shows projects as a responsive card grid with current health, latency, HTTP status, last check, and deployed release information when available.

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

`yard deploy <project>` currently follows this lifecycle:

1. refuse to deploy if tracked local Git changes exist;
2. switch to the configured branch;
3. fetch and fast-forward from the configured Git remote;
4. run the project's backup command when configured;
5. derive an immutable image tag from the updated Git commit SHA;
6. build the configured Compose service using that tag;
7. run the migration service when configured;
8. persist the new image tag in the project's Compose `.env` file;
9. start only the application service (`--no-deps`), leaving persistent dependencies untouched;
10. wait for the configured HTTP health check;
11. record the current and previous release under `/var/lib/yard`.

If activation or the health check fails after the new image has been selected, Yard attempts to restore the previous application image automatically.

Database rollback is deliberately separate. Yard never restores a database automatically just because an application image was rolled back.

Yard treats long-lived dependencies such as databases as already-provisioned infrastructure. A migration service may start the dependencies it needs, but release activation itself uses `docker compose up --no-deps` so a routine application deploy does not unexpectedly recreate PostgreSQL, Redis, or other persistent services.

## Rollback model

`yard rollback <project>` selects the previous release recorded by Yard, runs the project's backup command, switches the Compose image tag, starts the service and waits for the health check.

A specific Git revision can also be supplied:

```bash
yard rollback hello-api a1b2c3d4
```

Yard intentionally requires the corresponding image to already exist locally. Rebuilding arbitrary historical releases is a separate concern and avoids silently changing the source checkout during an emergency rollback.

## State

Deployment state is stored as JSON under:

```text
/var/lib/yard/<project>.json
```

The state contains only deployment metadata such as Git revisions and image tags. Application secrets remain outside Yard.

## Security

Recommended practices:

- keep project manifests free of credentials;
- keep application `.env` files protected (`0600` where appropriate);
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
