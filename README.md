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
```

## Philosophy

Yard aims to be:

- **small** — a CLI, not a platform;
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
- Rust/Cargo only when building Yard from source.

The resulting Yard executable is a standalone native binary; Python or another application runtime is not required.

## Installation

Build and install from source:

```bash
git clone https://github.com/godart-corentin/yard.git
cd yard
./install.sh
```

The installer builds a release binary and installs:

```text
/usr/local/bin/yard
/usr/local/share/yard/web-src/
/etc/yard/projects/
/var/lib/yard/
```

The optional private status dashboard uses a standalone Rust server in Docker and follows Kilnr's `install-web.sh` lifecycle. See [`docs/web.md`](docs/web.md).

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

## Deployment model

`yard deploy <project>` currently follows this lifecycle:

1. refuse to deploy if tracked local Git changes exist;
2. switch to the configured branch;
3. run the project's backup command when configured;
4. fetch and fast-forward from the configured Git remote;
5. derive an immutable image tag from the Git commit SHA;
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

## Status

Yard is in **early development**. The command surface and manifest format may change while the deployment and rollback model is being hardened.

The first target is a single-host Docker Compose homelab. More abstraction should only be added when real deployments demonstrate a need for it.

## License

MIT
