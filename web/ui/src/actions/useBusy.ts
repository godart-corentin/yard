import { useRunningJobs } from "../api/jobQueries";

/**
 * True while a job holds the lock that a project action would need: a job of
 * that project, or a global job (prune, host collect), which excludes everything.
 */
export function useProjectBusy(project: string): boolean {
  const running = useRunningJobs();
  return running.data?.jobs.some((job) => job.project === project || job.project === null) ?? false;
}

/** True while any job runs: global actions exclude, and are excluded by, every job. */
export function useAnyBusy(): boolean {
  const running = useRunningJobs();
  return (running.data?.jobs.length ?? 0) > 0;
}
