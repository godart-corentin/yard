export interface PruneCandidate {
  reference: string;
  bytes: number;
}

export interface PrunePlan {
  candidates: PruneCandidate[];
  reclaimableBytes: number;
}

/**
 * Parses `yard images --prune` (dry run) output:
 *
 *   Project: name
 *     keep repo:tag
 *     candidate repo:tag (123 bytes)
 *     reclaimable: 123 bytes (estimate; ...)
 *
 * Returns null when the output does not look like that, so the caller can show it raw.
 */
export function parsePrunePlan(output: string): PrunePlan | null {
  const candidates: PruneCandidate[] = [];
  let recognized = false;
  for (const line of output.split("\n")) {
    const text = line.trim();
    if (/^Project: \S+/.test(text) || /^reclaimable: \d+ bytes/.test(text)) {
      recognized = true;
      continue;
    }
    const match = /^candidate (\S+) \((\d+) bytes\)$/.exec(text);
    if (match) {
      recognized = true;
      candidates.push({ reference: match[1] as string, bytes: Number(match[2]) });
    } else if (text.startsWith("candidate ")) {
      return null;
    }
  }
  if (!recognized) return null;
  return { candidates, reclaimableBytes: candidates.reduce((sum, c) => sum + c.bytes, 0) };
}

export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const units = ["kB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = -1;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}
