import { Outlet } from "@tanstack/react-router";

import styles from "./AppShell.module.css";
import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";

/** Sidebar + top bar around the routed page. */
export function AppShell() {
  return (
    <div className={styles.app}>
      <Sidebar />
      <main className={styles.main}>
        <Topbar />
        <div className={styles.content}>
          <Outlet />
        </div>
      </main>
    </div>
  );
}
