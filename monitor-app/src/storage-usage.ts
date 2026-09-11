interface ContainerCapacity {
  container_ref: string;
  capacity_ceiling: number | null;
  capacity_free: number | null;
  capacity_in_use: number | null;
}
export function storageUsage(containers: ContainerCapacity[]) {
  if (!containers.length) return null;
  let total = 0, used = 0;
  const seen = new Set<string>();
  for (const c of containers) {
    if (!c.container_ref || seen.has(c.container_ref)) return null;
    seen.add(c.container_ref);
    const ceiling = c.capacity_ceiling;
    if (ceiling === null || !Number.isFinite(ceiling) || ceiling <= 0) return null;
    const free = c.capacity_free;
    if (free !== null && (!Number.isFinite(free) || free < 0 || free > ceiling)) return null;
    const inUse = c.capacity_in_use ?? (free === null ? null : ceiling - free);
    if (inUse === null || !Number.isFinite(inUse) || inUse < 0 || inUse > ceiling) return null;
    total += ceiling;
    used += inUse;
  }
  if (!Number.isSafeInteger(total) || !Number.isSafeInteger(used)) return null;
  return { total, used, percent: used / total * 100 };
}
