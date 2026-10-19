/** A router destination a crumb can link to (typed so `Link` accepts it). */
export type CrumbLink =
  | { to: "/" }
  | { to: "/activity" }
  | { to: "/projects/$name/overview"; params: { name: string } };

export interface Crumb {
  label: string;
  /** Absent on the current (last) crumb. */
  link?: CrumbLink;
}

const TAB_LABELS: Record<string, string> = {
  overview: "Overview",
  releases: "Releases & backups",
  logs: "Logs",
  activity: "Activity",
};

const SECTION_LABELS: Record<string, string> = { activity: "Activity", images: "Images", host: "Host" };

/** Breadcrumb trail for a URL path; the last crumb is the current page. */
export function breadcrumbs(pathname: string): Crumb[] {
  const parts = pathname.split("/").filter(Boolean).map(decodeURIComponent);
  const [first, second, third] = parts;
  if (first === "projects" && second) {
    const projects: Crumb = { label: "Projects", link: { to: "/" } };
    if (!third) return [projects, { label: second }];
    return [
      projects,
      { label: second, link: { to: "/projects/$name/overview", params: { name: second } } },
      { label: TAB_LABELS[third] ?? third },
    ];
  }
  if (first === "activity" && second) {
    return [{ label: "Activity", link: { to: "/activity" } }, { label: `Job ${second}` }];
  }
  if (first && SECTION_LABELS[first]) return [{ label: SECTION_LABELS[first] }];
  return [{ label: "Overview" }];
}
