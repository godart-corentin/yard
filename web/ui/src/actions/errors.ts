import { YardApiError } from "../api";

export interface ActionError {
  message: string;
  /** Set on a 409: the job that holds the lock, so the UI can link to it. */
  runningJobId?: string;
}

/**
 * Honest, user-facing text for a failed `POST /api/jobs`. The server only sends
 * generic messages, so the wording here comes from the status and code alone.
 */
export function describeJobError(error: unknown): ActionError {
  if (!(error instanceof YardApiError)) {
    return { message: "Could not reach Yard Web. Nothing was started; check your connection and try again." };
  }
  switch (error.status) {
    case 400:
      return {
        message:
          "Yard refused this request (unknown project, or a revision that is no longer a restore point). Refresh the page and try again.",
      };
    case 403:
      return {
        message:
          "Yard Web refused the action. The reverse proxy is not configured for actions (it must forward X-Yard-User and X-Yard-Proxy-Token, and Web needs YARD_WEB_PROXY_TOKEN), or the request failed the CSRF check.",
      };
    case 409:
      return {
        message: "Another job already holds the lock for this target. Wait for it to finish, then try again.",
        runningJobId: error.body?.running_job_id ?? undefined,
      };
    case 429:
      return { message: "Yard Web is busy right now (too many open streams). Nothing was started; try again shortly." };
    case 503:
      return { message: "The host executor is unavailable, so nothing was started. Check the yard-web-ops service on the host." };
    default:
      return { message: `Yard Web failed to start the job (status ${error.status}). Nothing is known to have started; check Activity before retrying.` };
  }
}
