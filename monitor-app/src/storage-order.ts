/** Overview-only ordering. Keep identities and the collector's array untouched. */
export function orderOverviewDisks<T extends { name: string }>(disks: readonly T[]): T[] {
  const preferred = (disk: T) => /zhitai|致态/i.test(disk.name);
  return [...disks.filter(preferred), ...disks.filter(disk => !preferred(disk))];
}
