import { Link, useRouterState } from "@tanstack/react-router";
import { Fragment } from "react";

import { CommandPalette } from "../palette/CommandPalette";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { useStatus } from "../api/queries";
import { formatAge } from "../lib/format";
import { breadcrumbs } from "./breadcrumbs";
import styles from "./Topbar.module.css";
import { useNow } from "./useNow";

function Breadcrumbs() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const crumbs = breadcrumbs(pathname);
  return (
    <nav className={styles.crumbs} aria-label="Breadcrumb">
      {crumbs.map((crumb, index) => (
        <Fragment key={`${index}-${crumb.label}`}>
          {index > 0 && <span aria-hidden="true">/</span>}
          {crumb.link ? (
            <Link {...crumb.link}>{crumb.label}</Link>
          ) : (
            <strong aria-current="page">{crumb.label}</strong>
          )}
        </Fragment>
      ))}
    </nav>
  );
}

function RefreshButton() {
  const { dataUpdatedAt, isFetching, isError, refetch } = useStatus();
  const now = useNow(1000);
  const ago = dataUpdatedAt ? `${formatAge((now - dataUpdatedAt) / 1000)} ago` : "Refresh";
  return (
    <Button
      onClick={() => void refetch()}
      aria-label={isFetching ? "Refreshing status" : "Refresh status"}
      title={isError ? "The last refresh failed" : undefined}
      data-fetching={isFetching || undefined}
    >
      <span className={styles.spin} data-spinning={isFetching || undefined}>
        <Icon name="refresh" size={15} />
      </span>
      {ago}
    </Button>
  );
}

export function Topbar() {
  return (
    <header className={styles.top}>
      <Breadcrumbs />
      <div className={styles.tools}>
        <CommandPalette />
        <RefreshButton />
      </div>
    </header>
  );
}
