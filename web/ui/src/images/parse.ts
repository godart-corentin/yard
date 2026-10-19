export type ImageState = "keep" | "running" | "candidate";

export interface ImageRow {
  reference: string;
  state: ImageState;
  /** Only candidates carry a size in the CLI output. */
  sizeBytes: number | null;
}

export interface ImageProject {
  name: string;
  rows: ImageRow[];
  reclaimableBytes: number;
}

export interface ImageInventory {
  projects: ImageProject[];
  simulationOnly: boolean;
}

const REF = String.raw`(\S+)`;
const KEEP = new RegExp(`^  keep ${REF}$`);
const RUNNING = new RegExp(`^  keep ${REF} \\(running container\\)$`);
const CANDIDATE = new RegExp(`^  candidate ${REF} \\((\\d+) bytes\\)$`);
const RECLAIM = /^ {2}reclaimable: (\d+) bytes \(estimate; shared layers may reduce savings\)$/;
const PROJECT = /^Project: (\S+)$/;
const SIMULATION = "Simulation only; run yard images --prune --yes to remove candidates.";

/**
 * Parses the exact text printed by `yard images` (src/images.rs). Returns null
 * on any line it does not recognise or any inconsistent total, so the caller
 * can show the raw output instead of a guessed inventory.
 */
export function parseImages(output: string): ImageInventory | null {
  const lines = output.split("\n");
  while (lines.length && lines[lines.length - 1] === "") lines.pop();
  const projects: ImageProject[] = [];
  let current: ImageProject | null = null;
  let candidateSum = 0;
  let closed = true;
  let simulationOnly = false;

  for (const line of lines) {
    let m: RegExpMatchArray | null;
    if (line === SIMULATION) {
      if (!closed || simulationOnly) return null;
      simulationOnly = true;
    } else if (simulationOnly) {
      return null;
    } else if ((m = PROJECT.exec(line))) {
      if (!closed) return null;
      current = { name: m[1]!, rows: [], reclaimableBytes: 0 };
      projects.push(current);
      candidateSum = 0;
      closed = false;
    } else if (!current || closed) {
      return null;
    } else if ((m = RUNNING.exec(line))) {
      current.rows.push({ reference: m[1]!, state: "running", sizeBytes: null });
    } else if ((m = KEEP.exec(line))) {
      current.rows.push({ reference: m[1]!, state: "keep", sizeBytes: null });
    } else if ((m = CANDIDATE.exec(line))) {
      const size = Number(m[2]);
      if (!Number.isSafeInteger(size)) return null;
      current.rows.push({ reference: m[1]!, state: "candidate", sizeBytes: size });
      candidateSum += size;
    } else if ((m = RECLAIM.exec(line))) {
      const total = Number(m[1]);
      if (total !== candidateSum) return null;
      current.reclaimableBytes = total;
      closed = true;
    } else {
      return null;
    }
  }
  if (!closed) return null;
  // Without the simulation line (e.g. a prune run) this is not an inventory read.
  if (!simulationOnly) return null;
  return { projects, simulationOnly };
}
