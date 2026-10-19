import * as Dialog from "@radix-ui/react-dialog";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { Command } from "cmdk";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { DeployDialog, BackupDialog, HostCollectDialog, PruneDialog } from "../actions/dialogs";
import { useAnyBusy, useProjectBusy } from "../actions/useBusy";
import type { ProjectStatus } from "../api";
import { useStatus } from "../api/queries";
import { Icon, type IconName } from "../components/Icon";
import { projectTone } from "../lib/tone";
import styles from "./CommandPalette.module.css";

const TABS = [
  { path: "overview", label: "Overview" },
  { path: "releases", label: "Releases & backups" },
  { path: "logs", label: "Logs" },
  { path: "activity", label: "Activity" },
] as const;

type Pending = { kind: "deploy" | "backup"; project: ProjectStatus } | { kind: "host-collect" | "prune" };

/** True when the key event comes from a text field (other than the palette's own input). */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.closest("[cmdk-root]")) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

function Item({
  value,
  icon,
  dot,
  hint,
  mono,
  disabled,
  onSelect,
  children,
}: {
  value: string;
  icon?: IconName;
  dot?: string;
  hint: string;
  mono?: boolean;
  disabled?: boolean;
  onSelect: () => void;
  children: ReactNode;
}) {
  return (
    <Command.Item value={value} className={styles.item} disabled={disabled} onSelect={onSelect}>
      {icon && <Icon name={icon} size={16} />}
      {dot && <span className={styles.dot} data-tone={dot} aria-hidden="true" />}
      {children}
      <span className={`${styles.hint} ${mono ? styles.mono : ""}`}>{hint}</span>
    </Command.Item>
  );
}

/** Topbar search button + ⌘K palette. Actions only open the regular confirmation dialogs. */
export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [pending, setPending] = useState<Pending | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const navigate = useNavigate();
  const status = useStatus();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const anyBusy = useAnyBusy();

  const projects = status.data?.projects ?? [];
  const routeName = /^\/projects\/([^/]+)/.exec(pathname)?.[1];
  const routeProject = routeName ? decodeURIComponent(routeName) : undefined;
  const needle = query.trim().toLowerCase();
  const subject =
    projects.find((p) => p.name === routeProject) ??
    (needle ? projects.find((p) => p.name.toLowerCase().includes(needle)) : undefined);
  const projectBusy = useProjectBusy(subject?.name ?? "");

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === "k") {
        if (isTyping(event.target)) return;
        event.preventDefault();
        setOpen((value) => !value);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
  }, []);
  const run = (action: () => void) => () => {
    close();
    action();
  };
  const go = (to: string, params?: Record<string, string>) =>
    run(() => void navigate({ to, params } as Parameters<typeof navigate>[0]));

  return (
    <>
      <button type="button" ref={trigger} className={styles.search} aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <Icon name="search" size={15} />
        Search or run a command…
        <span className={styles.kbd}>⌘K</span>
      </button>

      <Dialog.Root open={open} onOpenChange={(next) => (next ? setOpen(true) : close())}>
        <Dialog.Portal>
          <Dialog.Overlay className={styles.overlay} />
          <Dialog.Content
            className={styles.palette}
            aria-label="Command palette"
            aria-describedby={undefined}
            onCloseAutoFocus={(event) => {
              // Back to the search button, unless a confirmation dialog is taking over.
              event.preventDefault();
              if (!pending) trigger.current?.focus();
            }}
          >
            <Dialog.Title className="sr-only">Command palette</Dialog.Title>
            <Command label="Command palette" loop>
              <div className={styles.inputRow}>
                <Icon name="search" size={18} />
                <Command.Input
                  className={styles.input}
                  value={query}
                  onValueChange={setQuery}
                  placeholder="Search or run a command…"
                  aria-label="Command"
                />
                <span className={styles.kbd}>esc</span>
              </div>
              <Command.List className={styles.list}>
                <Command.Empty className={styles.empty}>No matching command.</Command.Empty>

                {subject && !subject.url_monitor && (
                  <Command.Group heading={`Actions · ${subject.name}`}>
                    <Item
                      value={`Deploy ${subject.name}`}
                      icon="deploy"
                      hint="yard deploy"
                      mono
                      disabled={projectBusy}
                      onSelect={run(() => setPending({ kind: "deploy", project: subject }))}
                    >
                      Deploy {subject.name}
                    </Item>
                    <Item
                      value={`Back up ${subject.name}`}
                      icon="backup"
                      hint="yard backup"
                      mono
                      disabled={projectBusy}
                      onSelect={run(() => setPending({ kind: "backup", project: subject }))}
                    >
                      Back up {subject.name}
                    </Item>
                    <Item
                      value={`Restore ${subject.name}…`}
                      icon="restore"
                      hint="yard restore"
                      mono
                      onSelect={go("/projects/$name/releases", { name: subject.name })}
                    >
                      Restore {subject.name}…
                    </Item>
                    <Item
                      value={`Follow ${subject.name} logs`}
                      icon="logs"
                      hint="yard logs"
                      mono
                      onSelect={go("/projects/$name/logs", { name: subject.name })}
                    >
                      Follow {subject.name} logs
                    </Item>
                  </Command.Group>
                )}

                <Command.Group heading="Go to">
                  {projects.map((p) => (
                    <Item
                      key={p.name}
                      value={`Project ${p.name}`}
                      dot={projectTone(p.status)}
                      hint={p.url_monitor ? "URL monitor" : "Project"}
                      onSelect={go("/projects/$name", { name: p.name })}
                    >
                      {p.name}
                    </Item>
                  ))}
                  {subject &&
                    !subject.url_monitor &&
                    TABS.map((tab) => (
                      <Item
                        key={tab.path}
                        value={`${subject.name} ${tab.label} tab`}
                        dot={projectTone(subject.status)}
                        hint="Tab"
                        onSelect={go(`/projects/$name/${tab.path}`, { name: subject.name })}
                      >
                        {subject.name} · {tab.label}
                      </Item>
                    ))}
                  <Item value="Overview" icon="overview" hint="Page" onSelect={go("/")}>
                    Overview
                  </Item>
                  <Item value="Activity" icon="activity" hint="Page" onSelect={go("/activity")}>
                    Activity
                  </Item>
                  <Item value="Images" icon="images" hint="Page" onSelect={go("/images")}>
                    Images
                  </Item>
                  <Item value="Host" icon="host" hint="Page" onSelect={go("/host")}>
                    Host
                  </Item>
                </Command.Group>

                <Command.Group heading="Global">
                  <Item
                    value="Collect host metrics"
                    icon="host"
                    hint="yard host"
                    mono
                    disabled={anyBusy}
                    onSelect={run(() => setPending({ kind: "host-collect" }))}
                  >
                    Collect host metrics
                  </Item>
                  <Item
                    value="Simulate image prune"
                    icon="images"
                    hint="yard images --prune"
                    mono
                    disabled={anyBusy}
                    onSelect={run(() => setPending({ kind: "prune" }))}
                  >
                    Simulate image prune
                  </Item>
                </Command.Group>
              </Command.List>
            </Command>
            <div className={styles.foot}>
              <span>
                <span className={styles.kbd}>↑↓</span> navigate
              </span>
              <span>
                <span className={styles.kbd}>↵</span> open confirmation
              </span>
              <span>Actions always ask before running</span>
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>

      {pending && <PendingDialog pending={pending} onClose={() => setPending(null)} />}
    </>
  );
}

function PendingDialog({ pending, onClose }: { pending: Pending; onClose: () => void }) {
  const onOpenChange = (next: boolean) => {
    if (!next) onClose();
  };
  switch (pending.kind) {
    case "deploy":
      return <DeployDialog open onOpenChange={onOpenChange} project={pending.project} />;
    case "backup":
      return <BackupDialog open onOpenChange={onOpenChange} project={pending.project.name} />;
    case "host-collect":
      return <HostCollectDialog open onOpenChange={onOpenChange} />;
    case "prune":
      return <PruneDialog open onOpenChange={onOpenChange} />;
  }
}
